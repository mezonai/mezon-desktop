use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate, TimeZone};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, MouseButton, ObjectFit,
    PathPromptOptions, SharedString, Subscription, Task, Window, div, img, prelude::*, px,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, DatePicker, Icon, IconName, Input, InputEvent, InputState, Spinner,
    h_flex, v_flex,
};
use crate::image_cache::{
    LruImageCache, PREVIEW_ENTRY_MAX_BYTES, PREVIEW_IMAGE_CACHE_BYTES, PREVIEW_IMAGE_CACHE_CAPACITY,
};
use crate::theme::ActiveTheme;
use mezon_store::{
    AppConfig, ChannelId, ChannelMediaStore, ChannelTimelineAttachment, ClanId, CreateTimelineInput,
};

use super::media_image::image_placeholder;

const TITLE_MAX: usize = 100;
const DESCRIPTION_MAX: usize = 250;
const CREATE_MEDIA_COLS: usize = 3;

pub struct CreateMilestoneModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    channel_id: ChannelId,
    locale: String,
    title_input: Entity<InputState>,
    description_input: Entity<InputState>,
    date_picker: Entity<DatePicker>,
    attachments: Vec<ChannelTimelineAttachment>,
    uploading: bool,
    saving: bool,
    preview_image_cache: Entity<LruImageCache>,
    _title_sub: Subscription,
    _description_sub: Subscription,
    _date_sub: Subscription,
    _save_task: Option<Task<()>>,
}

impl Focusable for CreateMilestoneModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateMilestoneModal {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_ph: SharedString = mezon_i18n::t(
            &locale,
            "channelCreator.fields.createMilestone.eventTitlePlaceholder",
        )
        .into();
        let story_ph: SharedString = mezon_i18n::t(
            &locale,
            "channelCreator.fields.createMilestone.storyPlaceholder",
        )
        .into();
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(title_ph)
                .height(px(42.))
        });
        let description_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(story_ph)
                .multi_line(true)
                .height(px(96.))
        });
        let date_picker = cx.new(DatePicker::new);
        date_picker.update(cx, |picker, cx| {
            picker.set_locale(locale.clone());
            picker.set_selected_silent(Some(Local::now().date_naive()), cx);
        });

        let _title_sub = cx.subscribe(&title_input, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter) && this.can_save(cx) {
                this.save(cx);
            } else if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let _description_sub = cx.subscribe(&description_input, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let _date_sub = cx.subscribe(&date_picker, |_, _, _, cx| cx.notify());

        title_input.update(cx, |input, cx| input.focus(window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_id,
            locale,
            title_input,
            description_input,
            date_picker,
            attachments: Vec::new(),
            uploading: false,
            saving: false,
            preview_image_cache: cx.new(|cx| {
                LruImageCache::gallery_preview(
                    "create-milestone-preview",
                    PREVIEW_IMAGE_CACHE_CAPACITY,
                    PREVIEW_IMAGE_CACHE_BYTES,
                    PREVIEW_ENTRY_MAX_BYTES,
                    cx,
                )
            }),
            _title_sub,
            _description_sub,
            _date_sub,
            _save_task: None,
        }
    }

    fn can_save(&self, cx: &App) -> bool {
        !self.saving && !self.uploading && !self.title_input.read(cx).value().trim().is_empty()
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let media_paths: Vec<PathBuf> = paths
                .into_iter()
                .filter(|path| is_allowed_media(path.as_path()))
                .collect();
            if media_paths.is_empty() {
                return;
            }
            let pending_items: Vec<(i64, PathBuf)> = media_paths
                .iter()
                .enumerate()
                .map(|(index, path)| (next_pending_id(index), path.clone()))
                .collect();

            this.update(cx, |this, cx| {
                for (pending_id, path) in &pending_items {
                    this.attachments.push(pending_attachment(*pending_id, path));
                }
                this.uploading = true;
                cx.notify();
            })
            .ok();

            for (pending_id, path) in pending_items {
                let upload_task = this
                    .update(cx, |_, cx| {
                        ChannelMediaStore::global(cx)
                            .update(cx, |store, cx| store.upload_attachment(&path, cx))
                    })
                    .ok();
                let Some(task) = upload_task else {
                    continue;
                };
                match task.await {
                    Ok(att) => {
                        this.update(cx, |this, cx| {
                            if let Some(slot) =
                                this.attachments.iter().position(|a| a.id == pending_id)
                            {
                                this.attachments[slot] = att;
                            }
                            cx.notify();
                        })
                        .ok();
                    }
                    Err(message) => {
                        this.update(cx, |this, cx| {
                            this.attachments.retain(|a| a.id != pending_id);
                            cx.notify();
                        })
                        .ok();
                        this.update(cx, |_, cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        })
                        .ok();
                    }
                }
            }

            this.update(cx, |this, cx| {
                this.uploading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            cx.notify();
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let title = truncate_to_chars(self.title_input.read(cx).value().trim(), TITLE_MAX);
        let description = truncate_to_chars(
            self.description_input.read(cx).value().trim(),
            DESCRIPTION_MAX,
        );
        let selected = self
            .date_picker
            .read(cx)
            .selected()
            .unwrap_or_else(|| Local::now().date_naive());
        let (start_time_seconds, end_time_seconds) = date_range_seconds(selected);
        let attachments: Vec<_> = self
            .attachments
            .iter()
            .filter(|a| a.is_uploaded())
            .cloned()
            .collect();

        self.saving = true;
        cx.notify();

        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let input = CreateTimelineInput {
            title,
            description,
            start_time_seconds,
            end_time_seconds,
            attachments,
        };
        let task = ChannelMediaStore::global(cx).update(cx, |store, cx| {
            store.create_timeline(clan_id, channel_id, input, cx)
        });
        self._save_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(_) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                });
            }
            Err(e) => {
                tracing::error!("create timeline failed: {e}");
                this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                })
                .ok();
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(e, cx));
                });
            }
        }));
    }
}

impl Render for CreateMilestoneModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.preview_image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        let theme = cx.theme().as_ref().clone();
        let locale = &self.locale;
        let title_len = self.title_input.read(cx).value().len();
        let story_len = self.description_input.read(cx).value().len();
        let can_save = self.can_save(cx);

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(768.))
            .max_h(px(720.))
            .rounded_xl()
            .bg(theme.bg_floating)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "channelCreator.fields.createMilestone.headerTitle",
                            )),
                    )
                    .child(
                        div()
                            .id("create-milestone-close")
                            .cursor_pointer()
                            .rounded_lg()
                            .p_1()
                            .hover(|el| el.bg(theme.bg_hover))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            })
                            .child(
                                Icon::new(IconName::CloseIcon)
                                    .size(px(20.))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .child(
                div()
                    .id("create-milestone-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .py_4()
                    .gap_5()
                    .flex()
                    .flex_col()
                    .child(render_labeled_input(
                        &theme,
                        locale,
                        "channelCreator.fields.createMilestone.eventTitleLabel",
                        title_len,
                        TITLE_MAX,
                        Input::new(&self.title_input),
                    ))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .child(mezon_i18n::t(
                                        locale,
                                        "channelCreator.fields.createMilestone.dateLabel",
                                    )),
                            )
                            .child(self.date_picker.clone()),
                    )
                    .child(render_labeled_input(
                        &theme,
                        locale,
                        "channelCreator.fields.createMilestone.storyLabel",
                        story_len,
                        DESCRIPTION_MAX,
                        Input::new(&self.description_input),
                    ))
                    .child(div().image_cache(self.preview_image_cache.clone()).child(
                        render_attachments_section(
                            &theme,
                            locale,
                            &self.attachments,
                            self.uploading,
                            cx,
                        ),
                    )),
            )
            .child(
                div()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("create-milestone-save")
                            .label(mezon_i18n::t(
                                locale,
                                "channelCreator.fields.createMilestone.saveButton",
                            ))
                            .primary()
                            .w_full()
                            .disabled(!can_save)
                            .loading(self.saving)
                            .on_click(cx.listener(|this, _, _window, cx| this.save(cx))),
                    ),
            )
    }
}

fn render_labeled_input(
    theme: &crate::theme::Theme,
    locale: &str,
    label_key: &'static str,
    len: usize,
    max: usize,
    input: Input,
) -> impl IntoElement {
    let counter_color = if len >= max {
        theme.status_dnd
    } else {
        theme.text_muted
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(mezon_i18n::t(locale, label_key)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(counter_color)
                        .child(format!("{len}/{max}")),
                ),
        )
        .child(input)
}

fn render_attachments_section(
    theme: &crate::theme::Theme,
    locale: &str,
    attachments: &[ChannelTimelineAttachment],
    uploading: bool,
    cx: &mut Context<CreateMilestoneModal>,
) -> impl IntoElement {
    let config = AppConfig::global(cx).clone();
    let hover_border = Hsla::from(theme.tokens.bg_button_primary).opacity(0.5);

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(
                    locale,
                    "channelCreator.fields.createMilestone.memoriesLabel",
                )),
        )
        .when(!attachments.is_empty(), |el| {
            el.child(render_attachment_rows(theme, attachments, &config, cx))
        })
        .child(
            div()
                .id("create-milestone-upload")
                .flex_none()
                .w_full()
                .py_8()
                .border_2()
                .border_dashed()
                .border_color(theme.border)
                .rounded_xl()
                .bg(theme.bg_primary)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .cursor_pointer()
                .hover(|el| el.border_color(hover_border))
                .when(!uploading, |el| {
                    el.on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx)))
                })
                .child(if uploading {
                    Spinner::new().into_any_element()
                } else {
                    Icon::new(IconName::PlusIcon)
                        .size(px(40.))
                        .text_color(theme.text_muted)
                        .into_any_element()
                })
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(
                            locale,
                            "channelCreator.fields.createMilestone.uploadTitle",
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(
                            locale,
                            "channelCreator.fields.createMilestone.uploadSubtitle",
                        )),
                ),
        )
}

fn render_attachment_rows(
    theme: &crate::theme::Theme,
    attachments: &[ChannelTimelineAttachment],
    config: &AppConfig,
    cx: &mut Context<CreateMilestoneModal>,
) -> impl IntoElement {
    let mut cells: Vec<gpui::AnyElement> = attachments
        .iter()
        .enumerate()
        .map(|(index, att)| {
            render_attachment_cell(theme, att, config, index, cx).into_any_element()
        })
        .collect();

    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    while !cells.is_empty() {
        let take = CREATE_MEDIA_COLS.min(cells.len());
        let row: Vec<gpui::AnyElement> = cells.drain(..take).collect();
        let filler_count = CREATE_MEDIA_COLS - row.len();
        rows.push(
            h_flex()
                .gap_2()
                .w_full()
                .flex_none()
                .children(row)
                .children((0..filler_count).map(|_| div().flex_1().min_w_0().into_any_element()))
                .into_any_element(),
        );
    }

    v_flex().gap_2().mb_3().w_full().flex_none().children(rows)
}

fn render_attachment_cell(
    theme: &crate::theme::Theme,
    att: &ChannelTimelineAttachment,
    config: &AppConfig,
    index: usize,
    cx: &mut Context<CreateMilestoneModal>,
) -> impl IntoElement {
    let bg = theme.bg_tertiary;
    let muted = theme.text_muted;
    let url = attachment_thumb(att, config);
    let uploaded = att.is_uploaded();
    let is_image = att.file_type.starts_with("image/");

    div()
        .relative()
        .flex_1()
        .aspect_square()
        .min_w_0()
        .rounded_lg()
        .overflow_hidden()
        .when(uploaded, |el| {
            el.child(
                img(url)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_loading(move || image_placeholder(bg, muted, px(160.), px(160.)))
                    .with_fallback(move || image_placeholder(bg, muted, px(160.), px(160.))),
            )
        })
        .when(!uploaded && is_image, |el| {
            el.child(
                img(PathBuf::from(att.file_url.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .bg(Hsla::from(bg)),
            )
        })
        .when(!uploaded && !is_image, |el| {
            el.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(bg)
                    .child(
                        Icon::new(IconName::PlayButton)
                            .size(px(32.))
                            .text_color(muted),
                    ),
            )
        })
        .when(!uploaded, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::hsla(0., 0., 0., 0.4))
                    .child(Spinner::new()),
            )
        })
        .child(
            div()
                .absolute()
                .top_1()
                .right_1()
                .w(px(24.))
                .h(px(24.))
                .rounded_full()
                .bg(gpui::hsla(0., 0., 0., 0.6))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|el| el.bg(gpui::hsla(0., 0., 0., 0.8)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.remove_attachment(index, cx);
                    }),
                )
                .child(
                    Icon::new(IconName::CloseIcon)
                        .size(px(12.))
                        .text_color(theme.text_primary),
                ),
        )
}

fn next_pending_id(index: usize) -> i64 {
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as i64)
        .unwrap_or(0);
    -(base + index as i64)
}

fn pending_attachment(id: i64, path: &Path) -> ChannelTimelineAttachment {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("upload")
        .to_string();
    let file_type = mime_from_path(path);
    let file_size = std::fs::metadata(path)
        .map(|meta| meta.len() as i64)
        .unwrap_or(0);
    ChannelTimelineAttachment {
        id,
        file_name,
        file_url: path.to_string_lossy().into(),
        file_type,
        file_size,
        thumbnail: String::new(),
        width: 0,
        height: 0,
        duration: 0,
        message_id: 0,
    }
}

fn mime_from_path(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "mp4" => "video/mp4".into(),
        "webm" => "video/webm".into(),
        "mov" => "video/quicktime".into(),
        _ => "application/octet-stream".into(),
    }
}

fn truncate_to_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value.chars().take(max).collect()
    }
}

fn attachment_thumb(att: &ChannelTimelineAttachment, config: &AppConfig) -> SharedString {
    let url = att.display_url();
    if att.is_uploaded() && url.starts_with("http") {
        config.gallery_thumb_proxy(url).into()
    } else {
        url.into()
    }
}

fn is_allowed_media(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "mp4" | "webm" | "mov"
    )
}

fn date_range_seconds(date: NaiveDate) -> (u32, u32) {
    let dt = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    let local = Local
        .from_local_datetime(&dt)
        .single()
        .unwrap_or_else(Local::now);
    let start = local.timestamp().max(0) as u32;
    (start, start.saturating_add(86400))
}

pub fn open_create_milestone_modal(
    clan_id: ClanId,
    channel_id: ChannelId,
    locale: String,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| CreateMilestoneModal::new(clan_id, channel_id, locale, window, cx));
    Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
}
