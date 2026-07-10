use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ObjectFit, RenderImage, SharedString, Task,
    Window, div, img, prelude::*, px,
};
use mezon_store::{
    ScreenShareKind, ScreenShareListError, ScreenShareOption, ScreenSharePreview,
    Settings, VoiceStore, capture_screen_share_preview, list_screen_share_options,
    peek_screen_share_options,
};

use crate::app::shell::Shell;
use crate::chat::voice::ACCENT_BLUE;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Switch, h_flex, v_flex,
};
use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PreviewKey {
    kind: ScreenShareKind,
    id: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareTab {
    Window,
    EntireScreen,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SelectedTarget {
    kind: ScreenShareKind,
    id: u32,
}

pub struct ScreenShareModal {
    focus_handle: FocusHandle,
    voice: Entity<VoiceStore>,
    settings: Entity<Settings>,
    tab: ShareTab,
    options: Vec<ScreenShareOption>,
    previews: HashMap<PreviewKey, Arc<RenderImage>>,
    preview_requests: HashSet<PreviewKey>,
    loading: bool,
    load_error: Option<ScreenShareListError>,
    selected: Option<SelectedTarget>,
    share_audio: bool,
    load_started: bool,
    list_task: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    preview_loading: bool,
}

impl Focusable for ScreenShareModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ScreenShareModal {
    pub fn new(
        voice: Entity<VoiceStore>,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        cx.on_release(|this, cx| {
            for (_, image) in this.previews.drain() {
                crate::image_cache::queue_atlas_drop(cx, image);
            }
        })
        .detach();

        let cached = peek_screen_share_options();

        Self {
            focus_handle,
            voice,
            settings,
            tab: ShareTab::Window,
            options: cached.clone().unwrap_or_default(),
            previews: HashMap::new(),
            preview_requests: HashSet::new(),
            loading: cached.is_none(),
            load_error: None,
            selected: None,
            share_audio: false,
            load_started: false,
            list_task: None,
            preview_task: None,
            preview_loading: false,
        }
    }

    fn start_loading(&mut self, cx: &mut Context<Self>) {
        if self.load_started {
            return;
        }
        self.load_started = true;

        if let Some(options) = peek_screen_share_options() {
            self.options = options;
            self.loading = false;
        }

        self.list_task = Some(cx.spawn(async move |this, cx| {
            let (tx, rx) = flume::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send(list_screen_share_options());
            });
            let result = rx.recv_async().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(options)) => {
                        this.options = options;
                        this.loading = false;
                        this.preview_task = None;
                        this.preview_loading = false;
                        for (_, image) in this.previews.drain() {
                            crate::image_cache::queue_atlas_drop(cx, image);
                        }
                        this.preview_requests.clear();
                    }
                    Ok(Err(error)) => {
                        this.load_error = Some(error);
                        this.loading = false;
                    }
                    Err(_) => {
                        this.load_error = Some(ScreenShareListError::Unavailable(
                            "failed to load share targets".into(),
                        ));
                        this.loading = false;
                    }
                }
                cx.notify();
            });
        }));
    }

    fn start_preview_loading(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.load_error.is_some() || self.preview_loading {
            return;
        }

        let targets: Vec<(ScreenShareKind, u32)> = self
            .filtered_options()
            .into_iter()
            .filter_map(|option| {
                let key = PreviewKey {
                    kind: option.kind,
                    id: option.id,
                };
                if self.previews.contains_key(&key) || self.preview_requests.contains(&key) {
                    None
                } else {
                    Some((option.kind, option.id))
                }
            })
            .collect();
        if targets.is_empty() {
            return;
        }

        self.preview_requests.extend(
            targets
                .iter()
                .copied()
                .map(|(kind, id)| PreviewKey { kind, id }),
        );

        self.preview_loading = true;
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            let (tx, rx) = flume::bounded(targets.len().max(1));
            std::thread::Builder::new()
                .name("mezon-screen-previews".into())
                .spawn(move || {
                    for (kind, id) in targets {
                        let preview = capture_screen_share_preview(kind, id);
                        if tx.send((kind, id, preview)).is_err() {
                            break;
                        }
                    }
                })
                .ok();

            while let Ok((kind, id, preview)) = rx.recv_async().await {
                let key = PreviewKey { kind, id };
                let image = preview.and_then(preview_to_render_image);
                let updated = this.update(cx, |this, cx| match image {
                    Some(image) => {
                        if let Some(previous) = this.previews.insert(key, image) {
                            cx.drop_image(previous, None);
                        }
                        cx.notify();
                    }
                    None => {
                        this.preview_requests.remove(&key);
                    }
                });
                if updated.is_err() {
                    return;
                }
            }

            let _ = this.update(cx, |this, cx| {
                this.preview_loading = false;
                cx.notify();
            });
        }));
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn set_tab(&mut self, tab: ShareTab, cx: &mut Context<Self>) {
        self.tab = tab;
        if let Some(selected) = self.selected {
            let still_valid = self
                .options
                .iter()
                .any(|option| option_kind(option) == selected.kind && option.id == selected.id);
            if !still_valid {
                self.selected = None;
            }
        }
        cx.notify();
    }

    fn confirm_share(&mut self, cx: &mut Context<Self>) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(option) = self
            .options
            .iter()
            .find(|option| option_kind(option) == selected.kind && option.id == selected.id)
        else {
            return;
        };
        let pick = option.pick.clone();
        let share_audio = self.share_audio;
        self.voice.update(cx, |store, cx| {
            store.start_screen_share(pick, share_audio, cx)
        });
        self.close(cx);
    }

    fn filtered_options(&self) -> Vec<&ScreenShareOption> {
        let kind = match self.tab {
            ShareTab::Window => ScreenShareKind::Window,
            ShareTab::EntireScreen => ScreenShareKind::Display,
        };
        self.options
            .iter()
            .filter(|option| option.is_portal() || option.kind == kind)
            .collect()
    }
}

fn preview_to_render_image(preview: ScreenSharePreview) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(preview.width, preview.height, preview.rgba)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(buffer,)
    ])))
}

fn option_kind(option: &ScreenShareOption) -> ScreenShareKind {
    option.kind
}

impl Render for ScreenShareModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_loading(cx);
        self.start_preview_loading(cx);

        let locale = self.settings.read(cx).language.clone();
        let theme = cx.theme();
        let border: gpui::Hsla = theme.border.into();
        let text_primary: gpui::Hsla = theme.text_primary.into();
        let text_muted: gpui::Hsla = theme.text_muted.into();
        let bg: gpui::Hsla = theme.bg_secondary.into();
        let accent = gpui::Hsla::from(gpui::rgb(ACCENT_BLUE));
        let tile_bg: gpui::Hsla = theme.bg_tertiary.into();
        let status_dnd: gpui::Hsla = theme.status_dnd.into();

        let title: SharedString = mezon_i18n::t(&locale, "screenShare.chooseWhatToShare").into();
        let share_audio_label: SharedString =
            mezon_i18n::t(&locale, "screenShare.alsoShareSystemAudio").into();
        let window_tab: SharedString = mezon_i18n::t(&locale, "screenShare.window").into();
        let screen_tab: SharedString = mezon_i18n::t(&locale, "screenShare.entireScreen").into();
        let cancel_label: SharedString = mezon_i18n::t(&locale, "screenShare.cancel").into();
        let share_label: SharedString = mezon_i18n::t(&locale, "screenShare.share").into();
        let loading_label: SharedString = mezon_i18n::t(&locale, "screenShare.loading").into();
        let empty_label: SharedString = mezon_i18n::t(&locale, "screenShare.selectScreen").into();
        let portal_label: SharedString =
            mezon_i18n::t(&locale, "screenShare.linuxPortalOption").into();

        let filtered = self.filtered_options();
        let can_share = self.selected.is_some();

        let tab_window_active = self.tab == ShareTab::Window;
        let tab_screen_active = self.tab == ShareTab::EntireScreen;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                this.close(cx);
            }))
            .w(px(720.))
            .h(px(560.))
            .rounded(px(12.))
            .bg(bg)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .justify_between()
                    .items_center()
                    .px(px(20.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .id("screen-share-close")
                            .w(px(24.))
                            .h(px(24.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(0.5)
                            .hover(|s| s.opacity(1.0))
                            .text_size(px(18.))
                            .text_color(text_primary)
                            .on_click(cx.listener(|this, _, _window, cx| this.close(cx)))
                            .child("×"),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(8.))
                    .px(px(20.))
                    .pt(px(12.))
                    .child(tab_button(
                        "screen-share-tab-window",
                        window_tab,
                        tab_window_active,
                        accent,
                        text_muted,
                        border,
                        cx.listener(|this, _, _window, cx| {
                            this.set_tab(ShareTab::Window, cx);
                        }),
                    ))
                    .child(tab_button(
                        "screen-share-tab-screen",
                        screen_tab,
                        tab_screen_active,
                        accent,
                        text_muted,
                        border,
                        cx.listener(|this, _, _window, cx| {
                            this.set_tab(ShareTab::EntireScreen, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .id("screen-share-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .px(px(20.))
                    .py(px(16.))
                    .overflow_y_scroll()
                    .when(self.loading, |this| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .child(div().text_sm().text_color(text_muted).child(loading_label))
                    })
                    .when(!self.loading, |this| {
                        this.when_some(self.load_error.clone(), |el, error| {
                            el.flex()
                                .items_center()
                                .justify_center()
                                .child(match error {
                                    ScreenShareListError::PermissionDenied => {
                                        permission_denied_state(&locale, cx)
                                    }
                                    ScreenShareListError::Unavailable(message) => div()
                                        .text_sm()
                                        .text_color(status_dnd)
                                        .child(message)
                                        .into_any_element(),
                                })
                        })
                        .when(self.load_error.is_none(), |el| {
                            el.when(filtered.is_empty(), |empty| {
                                empty
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .py(px(40.))
                                    .child(
                                        div().text_sm().text_color(text_muted).child(empty_label),
                                    )
                            })
                            .when(!filtered.is_empty(), |grid| {
                                grid.child(target_grid(
                                    &filtered,
                                    &self.previews,
                                    self.selected,
                                    accent,
                                    border,
                                    text_primary,
                                    text_muted,
                                    tile_bg,
                                    portal_label.clone(),
                                    cx,
                                ))
                            })
                        })
                    }),
            )
            .child(
                v_flex()
                    .w_full()
                    .flex_none()
                    .gap(px(16.))
                    .px(px(20.))
                    .py(px(16.))
                    .border_t_1()
                    .border_color(border)
                    .when(cfg!(target_os = "macos"), |footer| {
                        footer.child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(text_primary)
                                        .child(share_audio_label),
                                )
                                .child(
                                    Switch::new("screen-share-audio")
                                        .checked(self.share_audio)
                                        .on_click(cx.listener(
                                            |this, checked: &bool, _window, cx| {
                                                this.share_audio = *checked;
                                                cx.notify();
                                            },
                                        )),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("screen-share-cancel")
                                    .label(cancel_label)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _window, cx| this.close(cx))),
                            )
                            .child(
                                Button::new("screen-share-confirm")
                                    .label(share_label)
                                    .primary()
                                    .disabled(!can_share)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.confirm_share(cx);
                                    })),
                            ),
                    ),
            )
    }
}

fn tab_button(
    id: &'static str,
    label: SharedString,
    active: bool,
    accent: gpui::Hsla,
    text_muted: gpui::Hsla,
    border: gpui::Hsla,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(14.))
        .py(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .when(active, |this| {
            this.bg(accent)
                .text_color(gpui::Hsla::from(gpui::rgb(0xffffff)))
        })
        .when(!active, |this| {
            this.text_color(text_muted)
                .border_1()
                .border_color(border)
                .hover(|s| s.bg(border))
        })
        .on_click(handler)
        .child(label)
}

#[allow(clippy::too_many_arguments)]
fn target_grid(
    options: &[&ScreenShareOption],
    previews: &HashMap<PreviewKey, Arc<RenderImage>>,
    selected: Option<SelectedTarget>,
    accent: gpui::Hsla,
    border: gpui::Hsla,
    text_primary: gpui::Hsla,
    text_muted: gpui::Hsla,
    tile_bg: gpui::Hsla,
    portal_label: SharedString,
    cx: &mut Context<ScreenShareModal>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(12.))
        .pb(px(4.))
        .children(options.iter().enumerate().map(|(index, option)| {
            let kind = option.kind;
            let preview_key = PreviewKey {
                kind,
                id: option.id,
            };
            let is_selected = selected
                == Some(SelectedTarget {
                    kind,
                    id: option.id,
                });
            let title = if option.is_portal() {
                portal_label.clone()
            } else {
                SharedString::from(option.title.clone())
            };
            let id = option.id;
            let tile_id = SharedString::from(format!("screen-share-tile-{index}"));
            let preview = previews.get(&preview_key).cloned();
            let has_preview = preview.is_some();

            div()
                .id(tile_id)
                .w(px(210.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.selected = Some(SelectedTarget { kind, id });
                    cx.notify();
                }))
                .child(
                    div()
                        .w_full()
                        .h(px(118.))
                        .rounded(px(8.))
                        .overflow_hidden()
                        .bg(tile_bg)
                        .border_2()
                        .when(is_selected, |this| this.border_color(accent))
                        .when(!is_selected, |this| this.border_color(border))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(preview, |tile, image| {
                            tile.child(img(image).size_full().object_fit(ObjectFit::Cover))
                        })
                        .when(!has_preview, |tile| tile.child(tile_icon(text_muted))),
                )
                .child(
                    div()
                        .mt(px(8.))
                        .text_sm()
                        .text_color(text_primary)
                        .truncate()
                        .child(title),
                )
        }))
}

fn tile_icon(color: gpui::Hsla) -> impl IntoElement {
    Icon::new(IconName::VoiceScreenShareIcon)
        .size(px(40.))
        .text_color(color)
}

const MACOS_SCREEN_CAPTURE_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

fn permission_denied_state(locale: &str, cx: &mut Context<ScreenShareModal>) -> gpui::AnyElement {
    let theme = cx.theme();
    let text_primary: gpui::Hsla = theme.text_primary.into();
    let text_muted: gpui::Hsla = theme.text_muted.into();
    let icon_bg: gpui::Hsla = theme.bg_tertiary.into();

    let title: SharedString = mezon_i18n::t(locale, "screenShare.permissionTitle").into();
    let description: SharedString =
        mezon_i18n::t(locale, "screenShare.permissionDescription").into();
    let restart_hint: SharedString =
        mezon_i18n::t(locale, "screenShare.permissionRestartHint").into();
    let open_settings: SharedString =
        mezon_i18n::t(locale, "screenShare.openSystemSettings").into();

    v_flex()
        .items_center()
        .gap(px(12.))
        .max_w(px(440.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(64.))
                .rounded_full()
                .bg(icon_bg)
                .child(
                    Icon::new(IconName::VoiceScreenShareIcon)
                        .size(px(28.))
                        .text_color(text_muted),
                ),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(text_primary)
                .text_center()
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(text_muted)
                .text_center()
                .child(description),
        )
        .when(cfg!(target_os = "macos"), |el| {
            el.child(
                div().mt(px(4.)).child(
                    Button::new("screen-share-open-settings")
                        .label(open_settings)
                        .primary()
                        .on_click(|_, _, cx| {
                            cx.open_url(MACOS_SCREEN_CAPTURE_SETTINGS_URL);
                        }),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(text_muted)
                    .opacity(0.8)
                    .text_center()
                    .child(restart_hint),
            )
        })
        .into_any_element()
}

pub fn open_screen_share_modal(
    voice: Entity<VoiceStore>,
    settings: Entity<Settings>,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| ScreenShareModal::new(voice, settings, window, cx));
    Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
}
