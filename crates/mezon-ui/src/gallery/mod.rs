use chrono::NaiveDate;
use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    ListAlignment, ListState, MouseButton, MouseDownEvent, Render, SharedString, Subscription,
    Window, div, img, list, prelude::*, px,
};
use mezon_store::{
    AppConfig, ChannelAttachment, ChannelId, ClanId, GalleryEvent, GalleryStore, LoadDirection,
    MediaFilter, Settings, enrich_uploader, resolve_attachment_uploader,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::components::primitives::{DatePicker, DatePickerEvent, Icon, IconName};
use crate::image_cache::{GALLERY_IMAGE_CACHE_BYTES, GALLERY_IMAGE_CACHE_CAPACITY, LruImageCache};
use crate::image_viewer::{OpenViewerRequest, open_image_viewer};
use crate::theme::{ActiveTheme, Theme};

const TILE: f32 = 144.0;
const COLUMNS: usize = 3;
const LOAD_MORE_THRESHOLD: usize = 4;
const DATE_FMT: &str = "%d/%m/%Y";
const DATE_FILTER_TOP: f32 = 92.0;
const GALLERY_MIN_DATE: NaiveDate = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();

enum GalleryRow {
    Header(SharedString),
    Images(Vec<GalleryTile>),
}

#[derive(Clone)]
struct GalleryTile {
    id: i64,
    is_video: bool,
    thumb_src: SharedString,
}

impl GalleryTile {
    fn from_attachment(att: &ChannelAttachment) -> Self {
        Self {
            id: att.id,
            is_video: att.is_video,
            thumb_src: att.thumb_src.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RowsSnapshot {
    filter: MediaFilter,
    len: usize,
    head_id: Option<i64>,
    tail_id: Option<i64>,
}

pub struct GalleryModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    channel_id: ChannelId,
    channel_label: SharedString,
    settings: Entity<Settings>,
    active_filter: MediaFilter,
    from_date_picker: Entity<DatePicker>,
    to_date_picker: Entity<DatePicker>,
    date_validation_error: Option<String>,
    applied_from_date: Option<NaiveDate>,
    applied_to_date: Option<NaiveDate>,
    date_filter_open: bool,
    rows: Vec<GalleryRow>,
    list_state: ListState,
    image_cache: Entity<LruImageCache>,
    rows_snapshot: Option<RowsSnapshot>,
    _subscription: Subscription,
    _release: Subscription,
    _from_picker_sub: Subscription,
    _to_picker_sub: Subscription,
}

impl GalleryModal {
    pub(crate) fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        channel_label: SharedString,
        settings: Entity<Settings>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let image_cache = cx.new(|cx| {
            LruImageCache::gallery_thumbnail(
                "gallery",
                GALLERY_IMAGE_CACHE_CAPACITY,
                GALLERY_IMAGE_CACHE_BYTES,
                crate::image_cache::SHARED_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let gallery = GalleryStore::global(cx);
        let subscription = cx.subscribe(&gallery, |this, _, event, cx| {
            let GalleryEvent::Changed(changed_channel) = event;
            if *changed_channel == this.channel_id {
                this.sync_rows_from_store(cx);
            }
        });
        let cache_for_release = image_cache.clone();
        let release = cx.on_release(move |_, cx| {
            cache_for_release.update(cx, |cache, cx| cache.clear_app(cx));
            if let Some(store) = GalleryStore::try_global(cx) {
                store.update(cx, |store, _| {
                    store.reset_channel_attachments(channel_id);
                });
            }
            crate::image_viewer::trim_process_memory();
        });
        let from_date_picker = cx.new(DatePicker::new);
        let to_date_picker = cx.new(DatePicker::new);
        let mut this = Self {
            focus_handle,
            clan_id,
            channel_id,
            channel_label,
            settings,
            active_filter: MediaFilter::All,
            from_date_picker: from_date_picker.clone(),
            to_date_picker: to_date_picker.clone(),
            date_validation_error: None,
            applied_from_date: None,
            applied_to_date: None,
            date_filter_open: false,
            rows: Vec::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(400.)),
            image_cache,
            rows_snapshot: None,
            _subscription: subscription,
            _release: release,
            _from_picker_sub: Subscription::new(|| ()),
            _to_picker_sub: Subscription::new(|| ()),
        };
        this._from_picker_sub = cx.subscribe(&from_date_picker, |this, _, event, cx| match event {
            DatePickerEvent::Opened => {
                this.to_date_picker
                    .update(cx, |picker, cx| picker.close(cx));
            }
            DatePickerEvent::Change(from) => {
                this.to_date_picker.update(cx, |picker, cx| {
                    picker.set_min(*from, cx);
                });
                this.validate_dates(cx);
                cx.notify();
            }
        });
        this._to_picker_sub = cx.subscribe(&to_date_picker, |this, _, event, cx| match event {
            DatePickerEvent::Opened => {
                this.from_date_picker
                    .update(cx, |picker, cx| picker.close(cx));
            }
            DatePickerEvent::Change(to) => {
                this.from_date_picker.update(cx, |picker, cx| {
                    picker.set_max(*to, cx);
                });
                this.validate_dates(cx);
                cx.notify();
            }
        });
        this.install_scroll_handler(cx);
        this.rebuild_rows(cx);
        gallery.update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, cx);
        });
        this
    }

    fn install_scroll_handler(&self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let clan = self.clan_id;
        let channel = self.channel_id;
        self.list_state
            .set_scroll_handler(move |event, _window, cx| {
                let near_bottom =
                    event.visible_range.end + LOAD_MORE_THRESHOLD >= event.count && event.count > 0;
                if near_bottom && let Some(this) = weak.upgrade() {
                    this.update(cx, |_, cx| {
                        GalleryStore::global(cx).update(cx, |store, cx| {
                            store.fetch_page(clan, channel, LoadDirection::Before, cx);
                        });
                    });
                }
            });
    }

    fn rows_snapshot(&self, cx: &App) -> RowsSnapshot {
        let store = GalleryStore::global(cx);
        let atts = store.read(cx).attachments(self.channel_id);
        RowsSnapshot {
            filter: self.active_filter,
            len: atts.len(),
            head_id: atts.first().map(|a| a.id),
            tail_id: atts.last().map(|a| a.id),
        }
    }

    fn sync_rows_from_store(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.rows_snapshot(cx);
        if self.rows_snapshot == Some(snapshot) {
            return;
        }
        self.rebuild_rows(cx);
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let store = GalleryStore::global(cx);
        let filtered = store.read(cx).filtered(self.channel_id, self.active_filter);
        let mut rows: Vec<GalleryRow> = Vec::new();
        let mut current_day: Option<i64> = None;
        let mut bucket: Vec<GalleryTile> = Vec::new();
        for att in filtered {
            if current_day != Some(att.day_index) {
                flush_bucket(&mut rows, &mut bucket);
                rows.push(GalleryRow::Header(att.day_label.clone()));
                current_day = Some(att.day_index);
            }
            bucket.push(GalleryTile::from_attachment(&att));
            if bucket.len() == COLUMNS {
                rows.push(GalleryRow::Images(std::mem::take(&mut bucket)));
            }
        }
        flush_bucket(&mut rows, &mut bucket);
        self.rows = rows;
        self.rows_snapshot = Some(self.rows_snapshot(cx));
        self.list_state.reset(self.rows.len());
        cx.notify();
    }

    fn set_filter(&mut self, filter: MediaFilter, cx: &mut Context<Self>) {
        self.close_date_filter_panel(cx);
        if self.active_filter != filter {
            self.active_filter = filter;
            self.rebuild_rows(cx);
        }
    }

    fn draft_from(&self, cx: &App) -> Option<NaiveDate> {
        self.from_date_picker.read(cx).selected()
    }

    fn draft_to(&self, cx: &App) -> Option<NaiveDate> {
        self.to_date_picker.read(cx).selected()
    }

    fn validate_dates(&mut self, cx: &mut Context<Self>) -> bool {
        let from = self.draft_from(cx);
        let to = self.draft_to(cx);
        self.date_validation_error = match (from, to) {
            (Some(start), Some(end)) if start > end => Some(
                mezon_i18n::t(
                    &self.locale(cx),
                    "channelTopbar.gallery.validation.startDateBeforeEnd",
                )
                .to_string(),
            ),
            _ => None,
        };
        self.date_validation_error.is_none()
    }

    fn apply_date_filter(&mut self, cx: &mut Context<Self>) {
        if !self.validate_dates(cx) {
            cx.notify();
            return;
        }
        let from = self.draft_from(cx);
        let to = self.draft_to(cx);
        if from.is_none() && to.is_none() {
            return;
        }
        let (after, before) = calculate_timestamps(from, to);
        self.applied_from_date = from;
        self.applied_to_date = to;
        GalleryStore::global(cx).update(cx, |store, cx| {
            store.apply_date_filter(self.clan_id, self.channel_id, after, before, cx);
        });
        self.date_filter_open = false;
        cx.notify();
    }

    fn clear_date_filter(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.applied_from_date = None;
        self.applied_to_date = None;
        self.date_validation_error = None;
        self.from_date_picker.update(cx, |picker, cx| {
            picker.set_selected_silent(None, cx);
            picker.set_min(Some(GALLERY_MIN_DATE), cx);
            picker.set_max(None, cx);
            picker.close(cx);
        });
        self.to_date_picker.update(cx, |picker, cx| {
            picker.set_selected_silent(None, cx);
            picker.set_min(None, cx);
            picker.set_max(None, cx);
            picker.close(cx);
        });
        GalleryStore::global(cx).update(cx, |store, cx| {
            store.clear_date_filter(self.clan_id, self.channel_id, cx);
        });
        cx.notify();
    }

    fn toggle_date_filter(&mut self, cx: &mut Context<Self>) {
        self.date_filter_open = !self.date_filter_open;
        if self.date_filter_open {
            let locale = self.locale(cx);
            self.from_date_picker.update(cx, |picker, cx| {
                picker.set_locale(locale.clone());
                picker.set_selected_silent(self.applied_from_date, cx);
                picker.set_min(Some(GALLERY_MIN_DATE), cx);
                picker.set_max(self.applied_to_date, cx);
                picker.close(cx);
            });
            self.to_date_picker.update(cx, |picker, cx| {
                picker.set_locale(locale);
                picker.set_selected_silent(self.applied_to_date, cx);
                picker.set_min(self.applied_from_date, cx);
                picker.set_max(None, cx);
                picker.close(cx);
            });
            self.date_validation_error = None;
        }
        cx.notify();
    }

    fn close_date_filter_panel(&mut self, cx: &mut Context<Self>) {
        if self.date_filter_open {
            self.date_filter_open = false;
            cx.notify();
        }
    }

    fn close_calendar_pickers(&mut self, cx: &mut Context<Self>) {
        self.from_date_picker
            .update(cx, |picker, cx| picker.close(cx));
        self.to_date_picker
            .update(cx, |picker, cx| picker.close(cx));
    }

    fn any_calendar_open(&self, cx: &App) -> bool {
        self.from_date_picker.read(cx).is_open() || self.to_date_picker.read(cx).is_open()
    }

    fn date_range_label(&self, locale: &str) -> String {
        match (self.applied_from_date, self.applied_to_date) {
            (None, None) => mezon_i18n::t(locale, "channelTopbar.gallery.sentDate").to_string(),
            (Some(start), None) => mezon_i18n::t(locale, "channelTopbar.gallery.dateRange.from")
                .replace("{{date}}", &format_date_label(start))
                .to_string(),
            (None, Some(end)) => mezon_i18n::t(locale, "channelTopbar.gallery.dateRange.to")
                .replace("{{date}}", &format_date_label(end))
                .to_string(),
            (Some(start), Some(end)) if start == end => format_date_label(start),
            (Some(start), Some(end)) => {
                mezon_i18n::t(locale, "channelTopbar.gallery.dateRange.range")
                    .replace("{{startDate}}", &format_date_label(start))
                    .replace("{{endDate}}", &format_date_label(end))
                    .to_string()
            }
        }
    }

    fn has_date_filter(&self) -> bool {
        self.applied_from_date.is_some() || self.applied_to_date.is_some()
    }

    fn open_attachment(&mut self, attachment_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.image_cache
            .update(cx, |cache, cx| cache.shrink_to(0, window, cx));
        let store = GalleryStore::global(cx);
        let mut playlist = store.read(cx).filtered(self.channel_id, self.active_filter);
        let Some(index) = playlist.iter().position(|a| a.id == attachment_id) else {
            return;
        };
        enrich_playlist(&mut playlist, self.clan_id, self.channel_id, cx);
        open_image_viewer(
            OpenViewerRequest {
                clan_id: self.clan_id,
                channel_id: self.channel_id,
                channel_label: self.channel_label.clone(),
                settings: self.settings.clone(),
                attachments: playlist,
                selected_index: index,
                selected_url: None,
                anchor_before: None,
            },
            cx,
        );
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        self.rows.clear();
        self.list_state.reset(0);
        GalleryStore::global(cx).update(cx, |store, _| {
            store.reset_channel_attachments(self.channel_id);
        });
        crate::image_viewer::trim_process_memory();
        cx.emit(DismissEvent);
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn render_image_row_at(
        &mut self,
        row_ix: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tiles: Vec<(i64, bool, SharedString)> = match self.rows.get(row_ix) {
            Some(GalleryRow::Images(atts)) => atts
                .iter()
                .map(|att| (att.id, att.is_video, att.thumb_src.clone()))
                .collect(),
            _ => return div().into_any_element(),
        };
        let entity = cx.entity();
        let mut row = div().flex().flex_row().gap_3().pb_3();
        for (id, is_video, thumb_src) in tiles {
            let entity_click = entity.clone();
            let media = if is_video {
                div().size_full().into_any_element()
            } else {
                img(thumb_src)
                    .size_full()
                    .object_fit(gpui::ObjectFit::Cover)
                    .into_any_element()
            };
            row = row.child(
                div()
                    .id(("gallery-tile", id as usize))
                    .size(px(TILE))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(theme.bg_tertiary)
                    .cursor_pointer()
                    .relative()
                    .child(media)
                    .when(is_video, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(48.))
                                        .rounded_full()
                                        .bg(gpui::hsla(0., 0., 0., 0.6))
                                        .child(
                                            Icon::new(IconName::PlayButton)
                                                .size(px(20.))
                                                .text_color(gpui::white()),
                                        ),
                                ),
                        )
                    })
                    .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
                        entity_click.update(cx, |this, cx| {
                            this.open_attachment(id, window, cx);
                        });
                    }),
            );
        }
        row.into_any_element()
    }
}

fn flush_bucket(rows: &mut Vec<GalleryRow>, bucket: &mut Vec<GalleryTile>) {
    if !bucket.is_empty() {
        rows.push(GalleryRow::Images(std::mem::take(bucket)));
    }
}

fn enrich_playlist(
    playlist: &mut [ChannelAttachment],
    clan: ClanId,
    channel_id: ChannelId,
    cx: &App,
) {
    let cfg = AppConfig::try_global(cx);
    enrich_uploader(playlist, |att| {
        let info =
            resolve_attachment_uploader(clan, channel_id, att.uploader_id, att.message_id, cfg, cx);
        (!info.name.is_empty()).then_some(info)
    });
}

impl Focusable for GalleryModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for GalleryModal {}

impl Render for GalleryModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.image_cache
            .update(cx, |cache, cx| cache.sweep(window, cx));
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let viewport_h = f32::from(window.viewport_size().height);
        let panel_h = px((viewport_h * 0.8).clamp(400.0, (viewport_h - 96.0).max(400.0)));

        let entity = cx.entity();
        let entity_body = entity.clone();
        let image_cache = self.image_cache.clone();
        let theme_for_list = theme.clone();
        let body = if self.rows.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_muted)
                .child(empty_label(
                    self.active_filter,
                    self.has_date_filter(),
                    &locale,
                ))
                .into_any_element()
        } else {
            div()
                .size_full()
                .relative()
                .overflow_hidden()
                .image_cache(image_cache)
                .child(
                    list(self.list_state.clone(), move |ix, _window, cx| {
                        if let Some(label) = entity.read(cx).rows.get(ix).and_then(|row| {
                            if let GalleryRow::Header(label) = row {
                                Some(label.clone())
                            } else {
                                None
                            }
                        }) {
                            return render_header(&label, &theme_for_list);
                        }
                        let entity_row = entity.clone();
                        let theme_row = theme_for_list.clone();
                        entity_row
                            .update(cx, |this, cx| this.render_image_row_at(ix, &theme_row, cx))
                    })
                    .flex_1()
                    .size_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&self.list_state),
                    window,
                    cx,
                )
                .into_any_element()
        };

        div()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &::menu::Cancel, window, cx| {
                if this.any_calendar_open(cx) {
                    this.close_calendar_pickers(cx);
                } else if this.date_filter_open {
                    this.close_date_filter_panel(cx);
                } else {
                    this.dismiss(window, cx);
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.dismiss(window, cx);
            }))
            .occlude()
            .relative()
            .w(px(480.))
            .h(panel_h)
            .flex()
            .flex_col()
            .shadow_lg()
            .rounded(px(8.))
            .bg(theme.bg_primary)
            .border_1()
            .border_color(theme.border)
            .child(render_modal_header(
                &t("channelTopbar.gallery.title"),
                &theme,
                &cx.entity(),
            ))
            .child(render_filter_tabs(
                self.active_filter,
                &theme,
                &locale,
                self.date_range_label(&locale),
                self.date_filter_open,
                &cx.entity(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .px_3()
                    .pb_3()
                    .on_mouse_down(
                        MouseButton::Left,
                        move |_: &MouseDownEvent, _window, cx: &mut App| {
                            entity_body.update(cx, |this, cx| {
                                if this.any_calendar_open(cx) {
                                    this.close_calendar_pickers(cx);
                                } else {
                                    this.close_date_filter_panel(cx);
                                }
                            });
                        },
                    )
                    .child(body),
            )
            .when(self.date_filter_open, |el| {
                let locale = self.locale(cx);
                self.from_date_picker.update(cx, |picker, _| {
                    picker.set_locale(locale.clone());
                });
                self.to_date_picker.update(cx, |picker, _| {
                    picker.set_locale(locale.clone());
                });
                el.child(
                    div()
                        .absolute()
                        .top(px(DATE_FILTER_TOP))
                        .right(px(16.))
                        .child(render_date_filter_panel(
                            &theme,
                            &locale,
                            self.from_date_picker.clone(),
                            self.to_date_picker.clone(),
                            self.date_validation_error.clone(),
                            &cx.entity(),
                        )),
                )
            })
    }
}

fn render_modal_header(
    title: &str,
    theme: &Theme,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let entity = entity.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .child(
                    Icon::new(IconName::ImageThumbnail)
                        .size(px(20.))
                        .text_color(theme.text_primary),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .id("gallery-close")
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|el| el.bg(theme.bg_hover))
                .child(
                    Icon::new(IconName::Close)
                        .size(px(16.))
                        .text_color(theme.text_secondary),
                )
                .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
                    entity.update(cx, |this, cx| this.dismiss(window, cx));
                }),
        )
}

fn render_filter_tabs(
    active: MediaFilter,
    theme: &Theme,
    locale: &str,
    date_label: String,
    date_filter_open: bool,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let entity_toggle = entity.clone();
    let mut chevron = Icon::new(IconName::ChevronDown)
        .size(px(12.))
        .text_color(theme.text_primary);
    if date_filter_open {
        chevron = chevron.with_transformation(gpui::Transformation::rotate(gpui::radians(
            std::f32::consts::PI,
        )));
    }
    let date_trigger = div()
        .id("gallery-date-filter-trigger")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .bg(theme.tokens.bg_surface)
        .text_color(theme.text_primary)
        .child(date_label)
        .child(chevron)
        .on_mouse_down(
            MouseButton::Left,
            move |_: &MouseDownEvent, _window, cx: &mut App| {
                entity_toggle.update(cx, |this, cx| this.toggle_date_filter(cx));
            },
        );

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_4()
        .py_2()
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(filter_tab(
                    MediaFilter::All,
                    active,
                    mezon_i18n::t(locale, "channelTopbar.gallery.filters.all").to_string(),
                    theme,
                    entity,
                ))
                .child(filter_tab(
                    MediaFilter::Image,
                    active,
                    mezon_i18n::t(locale, "channelTopbar.gallery.filters.images").to_string(),
                    theme,
                    entity,
                ))
                .child(filter_tab(
                    MediaFilter::Video,
                    active,
                    mezon_i18n::t(locale, "channelTopbar.gallery.filters.videos").to_string(),
                    theme,
                    entity,
                )),
        )
        .child(div().relative().child(date_trigger))
}

fn render_date_filter_panel(
    theme: &Theme,
    locale: &str,
    from_picker: Entity<DatePicker>,
    to_picker: Entity<DatePicker>,
    validation_error: Option<String>,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let has_error = validation_error.is_some();
    let entity_clear = entity.clone();
    let entity_apply = entity.clone();

    div()
        .id("gallery-date-filter-panel")
        .occlude()
        .w(px(300.))
        .p_4()
        .flex()
        .flex_col()
        .gap_4()
        .rounded(px(8.))
        .bg(theme.tokens.bg_surface)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_secondary)
                        .child(mezon_i18n::t(locale, "channelTopbar.gallery.fromDate")),
                )
                .child(div().w_full().child(from_picker)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_secondary)
                        .child(mezon_i18n::t(locale, "channelTopbar.gallery.toDate")),
                )
                .child(div().w_full().child(to_picker)),
        )
        .when_some(validation_error, |el, error| {
            el.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.status_dnd)
                    .child(error),
            )
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(|el| el.text_color(theme.interactive_hover))
                        .child(mezon_i18n::t(
                            locale,
                            "channelTopbar.gallery.buttons.clearAll",
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_: &MouseDownEvent, window, cx: &mut App| {
                                entity_clear.update(cx, |this, cx| {
                                    this.clear_date_filter(window, cx);
                                });
                            },
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .when(!has_error, |el| {
                            el.bg(theme.brand).text_color(gpui::white())
                        })
                        .when(has_error, |el| {
                            el.bg(theme.bg_tertiary).text_color(theme.text_muted)
                        })
                        .child(mezon_i18n::t(locale, "channelTopbar.gallery.buttons.apply"))
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                entity_apply.update(cx, |this, cx| {
                                    this.apply_date_filter(cx);
                                });
                            },
                        ),
                ),
        )
}

fn filter_tab(
    filter: MediaFilter,
    active: MediaFilter,
    label: String,
    theme: &Theme,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let is_active = filter == active;
    let entity = entity.clone();
    div()
        .id(SharedString::from(format!("gallery-tab-{label}")))
        .px_3()
        .py_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .when(is_active, |el| el.bg(theme.brand).text_color(gpui::white()))
        .when(!is_active, |el| {
            el.bg(theme.tokens.bg_surface)
                .text_color(theme.text_primary)
        })
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            move |_: &MouseDownEvent, _window, cx: &mut App| {
                entity.update(cx, |this, cx| this.set_filter(filter, cx));
            },
        )
}

fn render_header(label: &SharedString, theme: &Theme) -> gpui::AnyElement {
    div()
        .w_full()
        .pt_3()
        .pb_1()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
        .child(label.clone())
        .into_any_element()
}

fn empty_label(filter: MediaFilter, date_filtered: bool, locale: &str) -> String {
    let key = if date_filtered {
        "channelTopbar.gallery.emptyState.noMediaFilesDateRange"
    } else {
        match filter {
            MediaFilter::All => "channelTopbar.gallery.emptyState.noMediaFiles",
            MediaFilter::Image => "channelTopbar.gallery.emptyState.noImages",
            MediaFilter::Video => "channelTopbar.gallery.emptyState.noVideos",
        }
    };
    mezon_i18n::t(locale, key).to_string()
}

fn format_date_label(date: NaiveDate) -> String {
    date.format(DATE_FMT).to_string()
}

fn start_of_day_ts(date: NaiveDate) -> u32 {
    date.and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_utc().timestamp().try_into().ok())
        .unwrap_or(0)
}

fn end_of_day_ts(date: NaiveDate) -> u32 {
    date.and_hms_opt(23, 59, 59)
        .and_then(|dt| dt.and_utc().timestamp().try_into().ok())
        .unwrap_or(0)
}

fn calculate_timestamps(
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> (Option<u32>, Option<u32>) {
    match (from, to) {
        (Some(start), Some(end)) if start == end => {
            (Some(start_of_day_ts(start)), Some(end_of_day_ts(start)))
        }
        (from, to) => (from.map(start_of_day_ts), to.map(end_of_day_ts)),
    }
}
