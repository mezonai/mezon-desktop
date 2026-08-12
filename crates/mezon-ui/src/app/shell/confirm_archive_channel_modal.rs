use gpui::{App, Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ARCHIVE_ERR_PERMISSION, ChannelId, ChannelList, ClanId, ClanList};

use super::Shell;
use crate::channel_navigation::navigate_after_thread_removed;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::router::{self, Route, Router};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmArchiveChannelModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) channel_id: ChannelId,
    pub(super) parent_id: ChannelId,
    pub(super) is_thread: bool,
    pub(super) locale: String,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) archive_label: SharedString,
    pub(super) submitting: bool,
}

fn navigate_after_archive(
    cx: &mut App,
    clan_id: ClanId,
    channel_id: ChannelId,
    parent_id: ChannelId,
    is_thread: bool,
) {
    let viewing_archived = match Router::global(cx).read(cx).route() {
        Route::Channel {
            channel_id: active, ..
        } => active == channel_id,
        Route::Thread { thread_id, .. } => thread_id == channel_id,
        _ => false,
    };
    if !viewing_archived {
        return;
    }
    if is_thread && !parent_id.is_zero() {
        navigate_after_thread_removed(cx, clan_id, channel_id, parent_id);
        return;
    }
    if let Some(welcome_id) = ClanList::global(cx).read(cx).welcome_channel_id(clan_id) {
        router::navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id: welcome_id,
            },
        );
    } else {
        router::navigate(cx, Route::Chat);
    }
}

impl ConfirmArchiveChannelModal {
    fn archive(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.submitting = true;
        cx.notify();

        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let parent_id = self.parent_id;
        let is_thread = self.is_thread;
        let locale = self.locale.clone();

        cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |_, cx| {
                    ChannelList::global(cx).update(cx, |store, cx| {
                        store.archive_channel(clan_id, channel_id, cx)
                    })
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let result = task.await;

            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.close_modal(cx);
                    match &result {
                        Ok(()) => {
                            let key = if is_thread {
                                "channelMenu.toastArchiveThread"
                            } else {
                                "channelMenu.toastArchiveChannel"
                            };
                            shell.success(mezon_i18n::t(&locale, key).to_string(), cx);
                        }
                        Err(err) => {
                            tracing::error!("archive_channel failed for {channel_id}: {err}");
                            let key = if err == ARCHIVE_ERR_PERMISSION {
                                "channelMenu.toastArchivePermissionDenied"
                            } else {
                                "channelMenu.toastArchiveFailed"
                            };
                            shell.error(mezon_i18n::t(&locale, key).to_string(), cx);
                        }
                    }
                });
                if result.is_ok() {
                    navigate_after_archive(cx, clan_id, channel_id, parent_id, is_thread);
                }
            });

            let _ = this.update(cx, |this, cx| {
                this.submitting = false;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ConfirmArchiveChannelModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.submitting {
                    return;
                }
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
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if this.submitting {
                                    return;
                                }
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            })),
                    )
                    .child(
                        Button::new("confirm-archive-channel-confirm")
                            .label(self.archive_label.clone())
                            .danger()
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.archive(cx);
                            })),
                    ),
            )
    }
}
