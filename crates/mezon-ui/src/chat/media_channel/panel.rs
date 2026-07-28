use std::sync::Arc;

use gpui::{
    App, Context, Entity, FontWeight, ListAlignment, ListOffset, ListState, Render, Subscription,
    WeakEntity, Window, div, list, prelude::*, px,
};
use mezon_store::{
    AppConfig, ChannelId, ChannelMediaStore, ChannelPermissionsStore, ClanId, Settings,
    first_event_year,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, Sizable, Size, Spinner};
use crate::image_cache::{
    LruImageCache, PREVIEW_ENTRY_MAX_BYTES, PREVIEW_IMAGE_CACHE_BYTES, PREVIEW_IMAGE_CACHE_CAPACITY,
};
use crate::theme::{ActiveTheme, Theme};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use super::create_milestone_modal::open_create_milestone_modal;
use super::event_detail_view::EventDetailView;
use super::events_view::{
    render_events_empty, render_events_header, render_events_list_card, render_events_list_header,
    render_events_loading, render_year_tabs, year_list,
};
use super::timeline_view::{render_event_card, render_timeline_header};

const MEDIA_LIST_OVERDRAW: f32 = 400.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaChannelView {
    Timeline,
    Events,
    EventDetail,
}

pub struct MediaChannelPanel {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    view: MediaChannelView,
    timeline_year: i32,
    events_year: i32,
    detail_event_id: Option<i64>,
    detail_start_time: u32,
    detail_view: Option<Entity<EventDetailView>>,
    timeline_list_state: ListState,
    events_list_state: ListState,
    _store_sub: Subscription,
    _window_activation: Option<Subscription>,
    last_fetch_error: Option<String>,
    _permissions_sub: Subscription,
    preview_image_cache: Entity<LruImageCache>,
}

impl MediaChannelPanel {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let timeline_year = current_year();
        ChannelMediaStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, timeline_year, cx);
        });
        ChannelPermissionsStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, cx);
        });
        let store = ChannelMediaStore::global(cx);
        let _store_sub = cx.observe(&store, |this, store, cx| {
            let err = store
                .read(cx)
                .error(this.channel_id, this.timeline_year)
                .map(str::to_string);
            if err != this.last_fetch_error {
                if let Some(message) = err.clone() {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                }
                this.last_fetch_error = err;
            }
            this.timeline_list_state.remeasure();
            this.events_list_state.remeasure();
            cx.notify();
        });
        let _permissions_sub =
            cx.observe(&ChannelPermissionsStore::global(cx), |_, _, cx| cx.notify());
        Self {
            clan_id,
            channel_id,
            settings,
            view: MediaChannelView::Timeline,
            timeline_year,
            events_year: timeline_year,
            detail_event_id: None,
            detail_start_time: 0,
            detail_view: None,
            timeline_list_state: ListState::new(0, ListAlignment::Top, px(MEDIA_LIST_OVERDRAW))
                .measure_all(),
            events_list_state: ListState::new(0, ListAlignment::Top, px(MEDIA_LIST_OVERDRAW))
                .measure_all(),
            _store_sub,
            _window_activation: None,
            last_fetch_error: None,
            _permissions_sub,
            preview_image_cache: cx.new(|cx| {
                LruImageCache::gallery_preview(
                    "media-channel-preview",
                    PREVIEW_IMAGE_CACHE_CAPACITY,
                    PREVIEW_IMAGE_CACHE_BYTES,
                    PREVIEW_ENTRY_MAX_BYTES,
                    cx,
                )
            }),
        }
    }

    fn can_create_events(&self, cx: &App) -> bool {
        ChannelPermissionsStore::global(cx).read(cx).has_permission(
            "send-message",
            self.clan_id,
            self.channel_id,
        )
    }

    pub fn on_enter(&mut self, cx: &mut Context<Self>) {
        self.reset_view(cx);
        scroll_list_to_start(&self.timeline_list_state);
        scroll_list_to_start(&self.events_list_state);
        ChannelMediaStore::global(cx).update(cx, |store, cx| {
            store.refresh_if_stale(self.clan_id, self.channel_id, self.timeline_year, cx);
        });
        ChannelPermissionsStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(self.clan_id, self.channel_id, cx);
        });
        cx.notify();
    }

    pub fn bind_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._window_activation.is_some() {
            return;
        }
        self._window_activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                return;
            }
            let years = match this.view {
                MediaChannelView::Events => vec![this.events_year],
                _ => vec![this.timeline_year],
            };
            ChannelMediaStore::global(cx).update(cx, |store, cx| {
                store.refresh_on_focus(this.clan_id, this.channel_id, &years, cx);
            });
        }));
    }

    pub fn sync_channel(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.clan_id == clan_id && self.channel_id == channel_id {
            return;
        }
        self.clan_id = clan_id;
        self.channel_id = channel_id;
        self.timeline_year = current_year();
        self.events_year = self.timeline_year;
        self.timeline_list_state.reset(0);
        self.events_list_state.reset(0);
        self.reset_view(cx);
        ChannelMediaStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, self.timeline_year, cx);
        });
        cx.notify();
    }

    fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.view = MediaChannelView::Timeline;
        self.detail_event_id = None;
        self.detail_start_time = 0;
        self.detail_view = None;
        cx.notify();
    }

    fn open_create_modal(&self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        open_create_milestone_modal(self.clan_id, self.channel_id, locale, window, cx);
    }

    fn navigate_events(&mut self, cx: &mut Context<Self>) {
        self.view = MediaChannelView::Events;
        self.events_year = self.timeline_year;
        scroll_list_to_start(&self.events_list_state);
        ChannelMediaStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(self.clan_id, self.channel_id, self.events_year, cx);
        });
        cx.notify();
    }

    fn navigate_timeline(&mut self, cx: &mut Context<Self>) {
        self.view = MediaChannelView::Timeline;
        self.detail_view = None;
        self.detail_event_id = None;
        cx.notify();
    }

    fn navigate_event_detail(
        &mut self,
        event_id: i64,
        start_time_seconds: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view = MediaChannelView::EventDetail;
        self.detail_event_id = Some(event_id);
        self.detail_start_time = start_time_seconds;
        let panel = cx.weak_entity();
        let on_back = Arc::new(move |_: &mut Window, cx: &mut App| {
            panel.update(cx, |this, cx| this.navigate_timeline(cx)).ok();
        });
        self.detail_view = Some(cx.new(|cx| {
            EventDetailView::new(
                self.clan_id,
                self.channel_id,
                event_id,
                start_time_seconds,
                self.settings.clone(),
                self.preview_image_cache.clone(),
                on_back,
                window,
                cx,
            )
        }));
        cx.notify();
    }

    fn select_events_year(&mut self, year: i32, cx: &mut Context<Self>) {
        self.events_year = year;
        self.events_list_state.reset(0);
        ChannelMediaStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(self.clan_id, self.channel_id, year, cx);
        });
        cx.notify();
    }
}

impl Render for MediaChannelPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.preview_image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let panel = cx.weak_entity();

        match self.view {
            MediaChannelView::EventDetail => {
                if let Some(view) = self.detail_view.clone() {
                    return div().size_full().child(view).into_any_element();
                }
            }
            MediaChannelView::Events => {
                return self
                    .render_events(&theme, &locale, panel, window, cx)
                    .into_any_element();
            }
            MediaChannelView::Timeline => {}
        }

        let store = ChannelMediaStore::global(cx).read(cx);
        let events = store.events(self.channel_id, self.timeline_year);
        let loading = store.is_loading(self.channel_id, self.timeline_year);
        let fetch_error = store
            .error(self.channel_id, self.timeline_year)
            .map(str::to_string);
        let config = AppConfig::global(cx).clone();
        let first_year = first_event_year(events);
        let can_create = self.can_create_events(cx);

        if loading && events.is_empty() {
            return centered_spinner(&theme).into_any_element();
        }

        if events.is_empty() {
            let panel_create = panel.clone();
            return render_empty_state(&theme, &locale, can_create, move |window, cx| {
                panel_create
                    .update(cx, |this, cx| this.open_create_modal(window, cx))
                    .ok();
            })
            .into_any_element();
        }

        let panel_events = panel.clone();
        let on_calendar = Arc::new(move |_: &mut Window, cx: &mut App| {
            panel_events
                .update(cx, |this, cx| this.navigate_events(cx))
                .ok();
        });
        let panel_detail = panel.clone();
        let on_event_click = Arc::new(
            move |event_id: i64, start_time_seconds: u32, window: &mut Window, cx: &mut App| {
                panel_detail
                    .update(cx, |this, cx| {
                        this.navigate_event_detail(event_id, start_time_seconds, window, cx)
                    })
                    .ok();
            },
        );
        let panel_fab = panel.clone();
        let panel_retry = panel.clone();
        let retry_error = fetch_error.map(|e| e.to_string());
        sync_media_list(&self.timeline_list_state, events.len());
        let timeline_list_state = self.timeline_list_state.clone();
        let timeline_store = ChannelMediaStore::global(cx);
        let timeline_channel_id = self.channel_id;
        let timeline_year = self.timeline_year;
        let timeline_theme = theme.clone();
        let timeline_locale = locale.clone();
        let timeline_cache = self.preview_image_cache.clone();
        let timeline_config = config.clone();
        let timeline_body = list(timeline_list_state.clone(), move |index, _window, cx| {
            let store = timeline_store.read(cx);
            let Some(event) = store.events(timeline_channel_id, timeline_year).get(index) else {
                return div().into_any_element();
            };
            div()
                .relative()
                .w_full()
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w(px(2.))
                                .h_full()
                                .bg(timeline_theme.brand)
                                .opacity(0.3),
                        ),
                )
                .child(render_event_card(
                    &timeline_theme,
                    &timeline_locale,
                    event,
                    index,
                    &timeline_config,
                    timeline_cache.clone(),
                    Some(on_event_click.clone()),
                ))
                .into_any_element()
        })
        .size_full();

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_primary)
            .overflow_hidden()
            .when_some(retry_error, |el, message| {
                el.child(render_fetch_error_banner(
                    &theme,
                    &locale,
                    message,
                    move |_: &mut Window, cx: &mut App| {
                        panel_retry
                            .update(cx, |this, cx| {
                                ChannelMediaStore::global(cx).update(cx, |store, cx| {
                                    store.fetch(
                                        this.clan_id,
                                        this.channel_id,
                                        this.timeline_year,
                                        true,
                                        cx,
                                    );
                                });
                            })
                            .ok();
                    },
                ))
            })
            .child(render_timeline_header(
                &theme,
                &locale,
                first_year.as_deref(),
                Some(on_calendar),
            ))
            .child(
                div()
                    .image_cache(self.preview_image_cache.clone())
                    .id("timeline-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .px_6()
                    .py_4()
                    .child(timeline_body)
                    .custom_scrollbars(
                        Scrollbars::always_visible(ScrollAxes::Vertical)
                            .tracked_scroll_handle(&timeline_list_state),
                        window,
                        cx,
                    ),
            )
            .when(can_create, |el| {
                el.child(
                    div()
                        .id("timeline-fab")
                        .absolute()
                        .bottom_6()
                        .right_6()
                        .w(px(48.))
                        .h(px(48.))
                        .rounded_full()
                        .bg(theme.tokens.bg_button_primary)
                        .shadow_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|el| el.opacity(0.9))
                        .on_click({
                            move |_, window, cx| {
                                panel_fab
                                    .update(cx, |this, cx| this.open_create_modal(window, cx))
                                    .ok();
                            }
                        })
                        .child(
                            Icon::new(IconName::Plus)
                                .size(px(24.))
                                .text_color(theme.text_primary),
                        ),
                )
            })
            .into_any_element()
    }
}

impl MediaChannelPanel {
    fn render_events(
        &self,
        theme: &Arc<Theme>,
        locale: &str,
        panel: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let store = ChannelMediaStore::global(cx).read(cx);
        let events = store.events(self.channel_id, self.events_year);
        let loading = store.is_loading(self.channel_id, self.events_year);
        let config = AppConfig::global(cx).clone();
        let years = year_list();

        let panel_back = panel.clone();
        let on_back = Arc::new(move |_: &mut Window, cx: &mut App| {
            panel_back
                .update(cx, |this, cx| this.navigate_timeline(cx))
                .ok();
        });
        let panel_year = panel.clone();
        let on_year = Arc::new(move |year, _: &mut Window, cx: &mut App| {
            panel_year
                .update(cx, |this, cx| this.select_events_year(year, cx))
                .ok();
        });
        let panel_detail = panel.clone();
        let on_event_click = Arc::new(
            move |event_id: i64, start_time_seconds: u32, window: &mut Window, cx: &mut App| {
                panel_detail
                    .update(cx, |this, cx| {
                        this.navigate_event_detail(event_id, start_time_seconds, window, cx)
                    })
                    .ok();
            },
        );

        let body = if events.is_empty() {
            sync_media_list(&self.events_list_state, 0);
            div()
                .size_full()
                .child(render_events_list_header(theme, locale, self.events_year))
                .when(loading, |el| el.child(render_events_loading()))
                .when(!loading, |el| {
                    el.child(render_events_empty(theme, locale, self.events_year))
                })
                .into_any_element()
        } else {
            sync_media_list(&self.events_list_state, events.len() + 1);
            let list_state = self.events_list_state.clone();
            let event_store = ChannelMediaStore::global(cx);
            let channel_id = self.channel_id;
            let events_year = self.events_year;
            let event_theme = theme.clone();
            let event_locale = locale.to_string();
            let event_cache = self.preview_image_cache.clone();
            let event_count = events.len();
            div()
                .size_full()
                .image_cache(self.preview_image_cache.clone())
                .child(
                    list(list_state.clone(), move |index, _window, cx| {
                        if index == 0 {
                            return render_events_list_header(
                                &event_theme,
                                &event_locale,
                                events_year,
                            )
                            .into_any_element();
                        }
                        let event_index = index - 1;
                        let store = event_store.read(cx);
                        let Some(event) = store.events(channel_id, events_year).get(event_index)
                        else {
                            return div().into_any_element();
                        };
                        div()
                            .px_6()
                            .pb(if event_index + 1 == event_count {
                                px(80.)
                            } else {
                                px(16.)
                            })
                            .child(render_events_list_card(
                                &event_theme,
                                &event_locale,
                                event,
                                &config,
                                event_cache.clone(),
                                on_event_click.clone(),
                            ))
                            .into_any_element()
                    })
                    .size_full(),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_primary)
            .overflow_hidden()
            .child(render_events_header(theme, locale, on_back))
            .child(render_year_tabs(theme, &years, self.events_year, on_year))
            .child(
                div()
                    .id("events-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body)
                    .custom_scrollbars(
                        Scrollbars::always_visible(ScrollAxes::Vertical)
                            .tracked_scroll_handle(&self.events_list_state),
                        window,
                        cx,
                    ),
            )
    }
}

fn current_year() -> i32 {
    use chrono::Datelike;
    chrono::Utc::now().year()
}

fn sync_media_list(state: &ListState, item_count: usize) {
    let current = state.item_count();
    if item_count > current {
        state.splice(current..current, item_count - current);
    } else if item_count < current {
        state.reset(item_count);
    }
}

fn scroll_list_to_start(state: &ListState) {
    if state.item_count() == 0 {
        return;
    }
    state.scroll_to(ListOffset {
        item_ix: 0,
        offset_in_item: px(0.),
    });
}

fn centered_spinner(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .bg(theme.bg_primary)
        .child(Spinner::new().with_size(Size::Small))
}

fn render_fetch_error_banner(
    theme: &Theme,
    locale: &str,
    message: String,
    on_retry: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2()
        .bg(theme.bg_tertiary)
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .overflow_hidden()
                .child(message),
        )
        .child(
            div()
                .id("timeline-fetch-retry")
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_link)
                .cursor_pointer()
                .hover(|el| el.opacity(0.85))
                .on_click(move |_, window, cx| on_retry(window, cx))
                .child(mezon_i18n::t(locale, "channelVoice.retry")),
        )
}

fn render_empty_state(
    theme: &Theme,
    locale: &str,
    can_create: bool,
    on_create: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .gap_4()
        .px_8()
        .bg(theme.bg_primary)
        .child(
            div()
                .w(px(128.))
                .h(px(128.))
                .rounded_full()
                .bg(theme.bg_tertiary)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::ImageThumbnail)
                        .size(px(48.))
                        .text_color(theme.text_muted),
                ),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(
                    locale,
                    "channelCreator.fields.mediaHighlights.emptyTitle",
                )),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .text_center()
                .child(mezon_i18n::t(
                    locale,
                    "channelCreator.fields.mediaHighlights.emptyDescription",
                )),
        )
        .when(can_create, |col| {
            col.child(
                div()
                    .id("timeline-empty-create")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_6()
                    .py_3()
                    .rounded_lg()
                    .bg(theme.tokens.bg_button_primary)
                    .cursor_pointer()
                    .hover(|el| el.opacity(0.9))
                    .on_click(move |_, window, cx| on_create(window, cx))
                    .child(
                        Icon::new(IconName::PlusIcon)
                            .size(px(20.))
                            .text_color(theme.text_primary),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "channelCreator.fields.mediaHighlights.createFirstMilestone",
                            )),
                    ),
            )
        })
}
