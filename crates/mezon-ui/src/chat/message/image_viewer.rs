use std::sync::Arc;

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, KeyDownEvent, ObjectFit, Rgba, SharedString,
    Window, div, img, prelude::*, px,
};
use mezon_store::{PlatformStore, ViewerMedia};

use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Icon, IconName, h_flex, v_flex};
use crate::image_cache::{
    LruImageCache, VIEWER_IMAGE_CACHE_BYTES, VIEWER_IMAGE_CACHE_CAPACITY,
    VIEWER_IMAGE_ENTRY_MAX_BYTES,
};
use crate::theme::ActiveTheme;

pub struct ImageViewer {
    focus_handle: FocusHandle,
    images: Arc<[ViewerMedia]>,
    index: usize,
    uploader_name: SharedString,
    uploader_avatar: SharedString,
    image_cache: Entity<LruImageCache>,
}

impl ImageViewer {
    pub fn open(
        images: Arc<[ViewerMedia]>,
        index: usize,
        uploader_name: SharedString,
        uploader_avatar: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        if images.is_empty() {
            return;
        }
        let index = index.min(images.len() - 1);
        let view = cx.new(|cx| {
            let image_cache = cx.new(|cx| {
                LruImageCache::labeled(
                    "image-viewer",
                    VIEWER_IMAGE_CACHE_CAPACITY,
                    VIEWER_IMAGE_CACHE_BYTES,
                    VIEWER_IMAGE_ENTRY_MAX_BYTES,
                    cx,
                )
            });
            Self {
                focus_handle: cx.focus_handle(),
                images,
                index,
                uploader_name,
                uploader_avatar,
                image_cache,
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn go_prev(&mut self, cx: &mut Context<Self>) {
        let next = step_index(self.index, self.images.len(), false);
        if next != self.index {
            self.index = next;
            cx.notify();
        }
    }

    fn go_next(&mut self, cx: &mut Context<Self>) {
        let next = step_index(self.index, self.images.len(), true);
        if next != self.index {
            self.index = next;
            cx.notify();
        }
    }
}

fn step_index(index: usize, len: usize, forward: bool) -> usize {
    if forward {
        if index + 1 < len { index + 1 } else { index }
    } else if index > 0 {
        index - 1
    } else {
        index
    }
}

fn viewer_icon_button(
    id: &'static str,
    icon: IconName,
    color: Rgba,
    hover_bg: Rgba,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .on_click(on_click)
        .child(Icon::new(icon).size(px(18.)).text_color(color))
}

impl Render for ImageViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.images.len();
        let has_prev = self.index > 0;
        let has_next = self.index + 1 < count;
        let current_src = self.images[self.index].viewer_src.clone();
        let current_name = self.images[self.index].filename.clone();
        let download_url = self.images[self.index].url.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .image_cache(self.image_cache.clone())
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "left" => this.go_prev(cx),
                    "right" => this.go_next(cx),
                    _ => {}
                }
            }))
            .w(px(960.))
            .h(px(680.))
            .gap_3()
            .p(px(16.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Avatar::new()
                                    .src(self.uploader_avatar.clone())
                                    .size_px(px(28.)),
                            )
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(self.uploader_name.clone()),
                                    )
                                    .when(!current_name.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .child(current_name.clone()),
                                        )
                                    }),
                            )
                            .when(count > 1, |el| {
                                el.child(div().text_xs().text_color(theme.text_muted).child(
                                    SharedString::from(format!("{} / {}", self.index + 1, count)),
                                ))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(viewer_icon_button(
                                "viewer-download",
                                IconName::Download,
                                theme.text_secondary,
                                theme.bg_hover,
                                cx.listener(move |_, _, _window, cx| {
                                    if let Some(store) = PlatformStore::try_global(cx) {
                                        let _ = store.read(cx).open_url_external(&download_url);
                                    }
                                }),
                            ))
                            .child(viewer_icon_button(
                                "viewer-close",
                                IconName::Close,
                                theme.text_secondary,
                                theme.bg_hover,
                                cx.listener(|_, _, _window, cx| {
                                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                                }),
                            )),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .when(has_prev, |el| {
                        el.child(viewer_icon_button(
                            "viewer-prev",
                            IconName::ArrowLeft,
                            theme.text_primary,
                            theme.bg_hover,
                            cx.listener(|this, _, _window, cx| this.go_prev(cx)),
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                img(current_src)
                                    .id("viewer-image")
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .when(has_next, |el| {
                        el.child(viewer_icon_button(
                            "viewer-next",
                            IconName::RightIcon,
                            theme.text_primary,
                            theme.bg_hover,
                            cx.listener(|this, _, _window, cx| this.go_next(cx)),
                        ))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::step_index;

    #[test]
    fn step_index_clamps_at_both_ends() {
        assert_eq!(step_index(0, 3, false), 0);
        assert_eq!(step_index(0, 3, true), 1);
        assert_eq!(step_index(1, 3, true), 2);
        assert_eq!(step_index(2, 3, true), 2);
        assert_eq!(step_index(2, 3, false), 1);
        assert_eq!(step_index(0, 1, true), 0);
    }
}
