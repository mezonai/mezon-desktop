use gpui::{
    App, Context, Entity, FocusHandle, Focusable, PathPromptOptions, SharedString, Subscription,
    Window, div, img, prelude::*, px,
};
use mezon_store::{MemoEvent, MemoStore};

use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants, Input, InputEvent, InputState};
use crate::theme::ActiveTheme;

const MEMO_CAPTION_MAX_RUNES: usize = 100;
const MEMO_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub struct CreateMemoModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    caption: Entity<InputState>,
    preview_path: Option<std::path::PathBuf>,
    submitting: bool,
    _caption_sub: Subscription,
    _memo_sub: Subscription,
}

impl Focusable for CreateMemoModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateMemoModal {
    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let view = cx.new(|cx| {
            let placeholder = mezon_i18n::t(&locale, "memos.caption.placeholder").to_string();
            let caption = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .validate(|value, _cx| value.chars().count() <= MEMO_CAPTION_MAX_RUNES)
            });
            let caption_sub = cx.subscribe(&caption, |_this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
            let memo_sub =
                cx.subscribe(
                    &MemoStore::global(cx),
                    |this: &mut Self, _, event, cx| match event {
                        MemoEvent::Created => {
                            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                        }
                        MemoEvent::CreateFailed | MemoEvent::CreateCapExceeded => {
                            this.submitting = false;
                            cx.notify();
                        }
                        _ => {}
                    },
                );
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                caption,
                preview_path: None,
                submitting: false,
                _caption_sub: caption_sub,
                _memo_sub: memo_sub,
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn pick_image(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        let locale = self.locale.clone();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(mezon_i18n::t(&locale, "memos.create.pickImage").into()),
        });
        cx.spawn(async move |this, cx| {
            let Some(paths) = crate::util::file_dialog::resolve(rx, cx).await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let metadata = path.metadata().ok();
            let size = metadata.map(|entry| entry.len()).unwrap_or(u64::MAX);
            if size > MEMO_IMAGE_MAX_BYTES {
                let title = mezon_i18n::t(&locale, "common.filesTooPowerful");
                let content =
                    mezon_i18n::t(&locale, "common.maxFileSize").replace("{{sizeLimit}}", "10 MB");
                cx.update(|cx| {
                    if let Some(handle) = crate::app::main_window::handle(cx) {
                        let _ = cx.update_window(handle, |_, window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| {
                                shell.show_upload_limit(title, content, window, cx);
                            });
                        });
                    }
                });
                return;
            }
            let supported = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| {
                    ext.eq_ignore_ascii_case("jpg")
                        || ext.eq_ignore_ascii_case("jpeg")
                        || ext.eq_ignore_ascii_case("png")
                })
                .unwrap_or(false);
            if !supported {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.error(mezon_i18n::t(&locale, "memos.create.unsupportedFormat"), cx);
                    });
                });
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.preview_path = Some(path);
                cx.notify();
            });
        })
        .detach();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting || self.preview_path.is_none() {
            return;
        }
        if MemoStore::global(cx).read(cx).is_creating() {
            return;
        }
        let path = self.preview_path.clone().expect("checked above");
        let caption = self.caption.read(cx).value().to_string();
        let started =
            MemoStore::global(cx).update(cx, |store, cx| store.create_from_path(path, caption, cx));
        if !started {
            return;
        }
        self.submitting = true;
        cx.notify();
    }
}

impl gpui::Render for CreateMemoModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let can_post = self.preview_path.is_some() && !self.submitting;
        let title = mezon_i18n::t(&locale, "memos.create.title");

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(420.))
            .flex()
            .flex_col()
            .gap_4()
            .p(px(20.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(title),
            )
            .child(
                div()
                    .id("memo-pick-image")
                    .w_full()
                    .h(px(220.))
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_secondary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.pick_image(window, cx)))
                    .when_some(self.preview_path.clone(), |element, path| {
                        element.child(
                            img(path.to_string_lossy().to_string())
                                .w_full()
                                .h_full()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                    })
                    .when(self.preview_path.is_none(), |element| {
                        element.child(
                            div()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child(mezon_i18n::t(&locale, "memos.create.pickImage")),
                        )
                    }),
            )
            .child(
                Input::new(&self.caption)
                    .w_full()
                    .text_size(px(14.))
                    .text_color(theme.text_primary),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("memo-create-cancel")
                            .label(mezon_i18n::t(&locale, "common.cancel"))
                            .ghost()
                            .on_click(cx.listener(|_, _, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            })),
                    )
                    .child(
                        Button::new("memo-create-post")
                            .label(mezon_i18n::t(&locale, "memos.create.post"))
                            .primary()
                            .disabled(!can_post)
                            .on_click(cx.listener(|this, _, _window, cx| this.submit(cx))),
                    ),
            )
    }
}
