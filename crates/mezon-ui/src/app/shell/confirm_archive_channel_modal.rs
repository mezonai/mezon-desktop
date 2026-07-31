use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelId, ChannelList, ClanId, ClanList};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmArchiveChannelModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) channel_id: ChannelId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) archive_label: SharedString,
    pub(super) toast_message: SharedString,
}

impl Render for ConfirmArchiveChannelModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let toast_message = self.toast_message.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(440.))
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                v_flex()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(20.))
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(theme.text_primary)
                            .child(self.description.clone()),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .items_center()
                    .gap_4()
                    .p(px(16.))
                    .bg(theme.bg_secondary)
                    .child(
                        Button::new("confirm-archive-channel-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-archive-channel-confirm")
                            .label(self.archive_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                let channel_list = ChannelList::global(cx);
                                let is_current = matches!(
                                    crate::router::Router::global(cx).read(cx).route(),
                                    Route::Channel {
                                        channel_id: active, ..
                                    } if active == channel_id
                                );
                                let redirect = is_current
                                    .then(|| {
                                        channel_list
                                            .read(cx)
                                            .channel(clan_id, channel_id)
                                            .and_then(|ch| ch.parent_id)
                                            .or_else(|| {
                                                ClanList::global(cx)
                                                    .read(cx)
                                                    .welcome_channel_id(clan_id)
                                            })
                                            .or_else(|| {
                                                channel_list.read(cx).default_channel_id(clan_id)
                                            })
                                    })
                                    .flatten();
                                let task = channel_list.update(cx, |list, cx| {
                                    list.archive_channel(clan_id, channel_id, cx)
                                });
                                let toast = toast_message.clone();
                                cx.spawn(async move |cx| {
                                    if task.await.is_ok() {
                                        cx.update(|cx| {
                                            Shell::global(cx).update(cx, |shell, cx| {
                                                shell.success(toast.clone(), cx);
                                            });
                                            if let Some(channel_id) = redirect {
                                                navigate(
                                                    cx,
                                                    Route::Channel {
                                                        clan_id,
                                                        channel_id,
                                                    },
                                                );
                                            }
                                        });
                                    }
                                })
                                .detach();
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    ),
            )
    }
}
