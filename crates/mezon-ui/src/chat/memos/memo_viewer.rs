use std::time::Duration;

use chrono::Local;
use gpui::{
    App, ClickEvent, Context, FocusHandle, KeyDownEvent, ObjectFit, SharedString, Subscription,
    Task, Window, div, img, prelude::*, px,
};
use mezon_store::{
    AccountStore, ChannelId, DirectMessageStore, FriendStore, MemoEvent, MemoStore, UserId,
    build_memo_playlist, playlist_position,
};

use super::confirm_delete_memo;
use crate::app::shell::Shell;
use crate::chat::message::format_relative_time_from_seconds;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;
use crate::util::imgproxy;

const MEMO_AUTO_ADVANCE_SECS: u64 = 5;
const MEMO_REPLY_MAX_RUNES: usize = 2000;

pub struct MemoViewer {
    focus_handle: FocusHandle,
    locale: SharedString,
    creator_id: UserId,
    memo_id: i64,
    index: usize,
    advance_generation: u64,
    _auto_advance: Option<Task<()>>,
    reply_input: gpui::Entity<InputState>,
    reply_submitting: bool,
    _reply_sub: Subscription,
}

impl MemoViewer {
    pub fn open(
        creator_id: UserId,
        start_index: usize,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(group) = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(creator_id)
            .cloned()
        else {
            return;
        };
        if group.memos.is_empty() {
            return;
        }
        let index = start_index.min(group.memos.len() - 1);
        let memo_id = group.memos[index].id;
        let view = cx.new(|cx| {
            cx.subscribe(&MemoStore::global(cx), |this: &mut Self, _, event, cx| {
                if matches!(event, MemoEvent::Changed) {
                    this.sync_after_store_change(cx);
                }
            })
            .detach();
            let placeholder = mezon_i18n::t(&locale, "memos.reply.placeholder").to_string();
            let reply_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .validate(|value, _cx| value.chars().count() <= MEMO_REPLY_MAX_RUNES)
            });
            let reply_sub = cx.subscribe(&reply_input, |_this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                creator_id,
                memo_id,
                index,
                advance_generation: 0,
                _auto_advance: None,
                reply_input,
                reply_submitting: false,
                _reply_sub: reply_sub,
            }
        });
        view.update(cx, |this, cx| {
            this.mark_current_seen(cx);
            this.restart_auto_advance(cx);
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn playlist(&self, cx: &App) -> Vec<mezon_store::MemoSlide> {
        build_memo_playlist(MemoStore::global(cx).read(cx).groups())
    }

    fn current_playlist_index(&self, cx: &App) -> Option<usize> {
        playlist_position(&self.playlist(cx), self.creator_id, self.index)
    }

    fn mark_current_seen(&mut self, cx: &mut Context<Self>) {
        let mark = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(self.creator_id)
            .and_then(|group| group.memos.get(self.index))
            .map(|memo| (self.creator_id, memo.id));
        let Some((creator_id, memo_id)) = mark else {
            return;
        };
        MemoStore::global(cx).update(cx, |store, cx| {
            store.mark_seen(creator_id, memo_id, cx);
        });
    }

    fn restart_auto_advance(&mut self, cx: &mut Context<Self>) {
        self.advance_generation = self.advance_generation.wrapping_add(1);
        let generation = self.advance_generation;
        self._auto_advance = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(MEMO_AUTO_ADVANCE_SECS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.advance_generation != generation {
                    return;
                }
                this.advance(true, cx);
            });
        }));
    }

    fn advance(&mut self, auto_advance: bool, cx: &mut Context<Self>) {
        let playlist = self.playlist(cx);
        let Some(pos) = self.current_playlist_index(cx) else {
            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            return;
        };
        if pos + 1 >= playlist.len() {
            if auto_advance {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }
            return;
        }
        let next = playlist[pos + 1];
        self.creator_id = next.creator_id;
        self.index = next.memo_index;
        self.memo_id = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(self.creator_id)
            .and_then(|group| group.memos.get(self.index))
            .map(|memo| memo.id)
            .unwrap_or(self.memo_id);
        self.mark_current_seen(cx);
        self.restart_auto_advance(cx);
        cx.notify();
    }

    fn go_prev(&mut self, cx: &mut Context<Self>) {
        let playlist = self.playlist(cx);
        let Some(pos) = self.current_playlist_index(cx) else {
            return;
        };
        if pos == 0 {
            return;
        }
        let prev = playlist[pos - 1];
        self.creator_id = prev.creator_id;
        self.index = prev.memo_index;
        self.memo_id = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(self.creator_id)
            .and_then(|group| group.memos.get(self.index))
            .map(|memo| memo.id)
            .unwrap_or(self.memo_id);
        self.mark_current_seen(cx);
        self.restart_auto_advance(cx);
        cx.notify();
    }

    fn ensure_current_memo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(group) = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(self.creator_id)
        else {
            cx.defer(|cx| Self::close_viewer_deferred(cx));
            return false;
        };
        if group.memos.is_empty() {
            cx.defer(|cx| Self::close_viewer_deferred(cx));
            return false;
        }
        if let Some(index) = group.memos.iter().position(|memo| memo.id == self.memo_id) {
            if self.index != index {
                self.index = index;
                self.restart_auto_advance(cx);
            }
            return true;
        }
        self.index = self.index.min(group.memos.len() - 1);
        self.memo_id = group.memos[self.index].id;
        self.mark_current_seen(cx);
        self.restart_auto_advance(cx);
        true
    }

    fn sync_after_store_change(&mut self, cx: &mut Context<Self>) {
        if self.ensure_current_memo(cx) {
            cx.notify();
        }
    }

    fn submit_reply(&mut self, cx: &mut Context<Self>) {
        if self.reply_submitting || self.is_own_memo(cx) {
            return;
        }
        let text = self.reply_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.reply_submitting = true;
        self.advance_generation = self.advance_generation.wrapping_add(1);
        cx.notify();
        let creator_id = self.creator_id;
        let memo_id = self.memo_id;
        let task = MemoStore::global(cx).update(cx, |store, cx| {
            store.reply_memo(creator_id, memo_id, text, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.reply_submitting = false;
                cx.notify();
            });
            if let Ok(dm_channel_id) = result {
                let _ = cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                    DirectMessageStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
                    navigate(
                        cx,
                        Route::DirectMessage {
                            direct_id: ChannelId(dm_channel_id),
                            message_type: "3".into(),
                        },
                    );
                });
            }
        })
        .detach();
    }

    fn creator_label(&self, cx: &App) -> SharedString {
        if let Some(account) = AccountStore::global(cx).read(cx).account.as_ref()
            && UserId(account.user_id) == self.creator_id
        {
            let label = if account.display_name.is_empty() {
                account.username.clone()
            } else {
                account.display_name.clone()
            };
            return label.into();
        }
        FriendStore::global(cx)
            .read(cx)
            .friends()
            .iter()
            .find(|friend| friend.id == self.creator_id)
            .map(|friend| friend.label().into())
            .unwrap_or_else(|| self.creator_id.0.to_string().into())
    }

    fn creator_avatar(&self, cx: &App) -> (SharedString, SharedString) {
        if let Some(account) = AccountStore::global(cx).read(cx).account.as_ref()
            && UserId(account.user_id) == self.creator_id
        {
            let raw = account.avatar_url.as_deref().unwrap_or("");
            return (imgproxy::avatar_url(cx, raw).into(), raw.to_string().into());
        }
        if let Some(friend) = FriendStore::global(cx)
            .read(cx)
            .friends()
            .iter()
            .find(|friend| friend.id == self.creator_id)
        {
            return (
                imgproxy::avatar_url(cx, &friend.avatar_url).into(),
                friend.avatar_url.clone().into(),
            );
        }
        (SharedString::default(), SharedString::default())
    }

    fn is_own_memo(&self, cx: &App) -> bool {
        AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .is_some_and(|account| UserId(account.user_id) == self.creator_id)
    }

    fn close_viewer_deferred(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

fn viewer_icon_button(
    id: &'static str,
    icon: IconName,
    color: gpui::Rgba,
    hover_bg: gpui::Rgba,
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

impl gpui::Render for MemoViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.ensure_current_memo(cx) {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let locale = self.locale.clone();
        let Some(memo) = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(self.creator_id)
            .and_then(|group| {
                group
                    .memos
                    .iter()
                    .find(|memo| memo.id == self.memo_id)
                    .cloned()
            })
        else {
            return div().into_any_element();
        };
        let playlist = self.playlist(cx);
        let pos = self.current_playlist_index(cx).unwrap_or(0);
        let has_prev = pos > 0;
        let has_next = pos + 1 < playlist.len();
        let segment_count = MemoStore::global(cx)
            .read(cx)
            .group_for_creator(self.creator_id)
            .map(|group| group.memos.len())
            .unwrap_or(0);
        let label = self.creator_label(cx);
        let (avatar_src, avatar_raw) = self.creator_avatar(cx);
        let own_memo = self.is_own_memo(cx);
        let creator_id = self.creator_id;
        let memo_id = memo.id;
        let image_url = memo.image_url.clone();
        let caption = memo.caption.clone();
        let relative_time =
            format_relative_time_from_seconds(memo.create_time_second, &locale, Local::now());

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "left" => this.go_prev(cx),
                    "right" => this.advance(false, cx),
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
            .when(segment_count > 1, |element| {
                element.child(h_flex().gap_1().w_full().children((0..segment_count).map(
                    |segment| {
                        div()
                            .flex_1()
                            .h(px(3.))
                            .rounded_full()
                            .bg(if segment == self.index {
                                theme.brand
                            } else {
                                theme.border
                            })
                    },
                )))
            })
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child({
                        let mut avatar = Avatar::new().name(label.clone()).size_px(px(32.));
                        if !avatar_src.is_empty() {
                            avatar = avatar.src(avatar_src.clone());
                            if !avatar_raw.is_empty() && avatar_raw != avatar_src {
                                avatar = avatar.fallback_src(avatar_raw);
                            }
                        }
                        h_flex().gap_2().items_center().child(avatar).child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child(label),
                                )
                                .when(!relative_time.is_empty(), |element| {
                                    element.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_muted)
                                            .child(relative_time),
                                    )
                                }),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(own_memo, |element| {
                                element.child(
                                    div()
                                        .id("memo-delete")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(32.))
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.bg_hover))
                                        .on_click({
                                            let locale = locale.clone();
                                            move |_, window: &mut Window, cx: &mut App| {
                                                confirm_delete_memo(
                                                    creator_id, memo_id, &locale, window, cx,
                                                );
                                            }
                                        })
                                        .child(
                                            Icon::new(IconName::TrashIcon)
                                                .size(px(18.))
                                                .text_color(theme.danger),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .id("memo-viewer-close")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(32.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_click(|_, _, cx| {
                                        Shell::global(cx).update(cx, |shell, cx| {
                                            shell.close_modal(cx);
                                        });
                                    })
                                    .child(
                                        Icon::new(IconName::CloseIcon)
                                            .size(px(18.))
                                            .text_color(theme.text_secondary),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .when(has_prev, |element| {
                        element.child(viewer_icon_button(
                            "memo-viewer-prev",
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
                            .rounded_md()
                            .bg(theme.bg_secondary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                img(image_url)
                                    .w_full()
                                    .h_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .when(has_next, |element| {
                        element.child(viewer_icon_button(
                            "memo-viewer-next",
                            IconName::RightIcon,
                            theme.text_primary,
                            theme.bg_hover,
                            cx.listener(|this, _, _window, cx| this.advance(false, cx)),
                        ))
                    }),
            )
            .when(!caption.is_empty(), |element| {
                element.child(
                    div()
                        .text_sm()
                        .text_color(theme.text_primary)
                        .child(caption),
                )
            })
            .when(!own_memo, |element| {
                let can_send =
                    !self.reply_submitting && !self.reply_input.read(cx).value().trim().is_empty();
                element.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(
                            Input::new(&self.reply_input)
                                .flex_1()
                                .text_size(px(14.))
                                .text_color(theme.text_primary),
                        )
                        .child(
                            Button::new("memo-reply-send")
                                .label(mezon_i18n::t(&locale, "memos.reply.send"))
                                .primary()
                                .disabled(!can_send)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.submit_reply(cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}
