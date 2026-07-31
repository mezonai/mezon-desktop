use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelId, ChannelList, ClanId, ClanList};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmDeleteChannelModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) channel_id: ChannelId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) delete_label: SharedString,
}

fn redirect_route_after_delete(
    clan_id: ClanId,
    channel_id: ChannelId,
    cx: &mut gpui::App,
) -> Option<Route> {
    let current_id = match crate::router::Router::global(cx).read(cx).route() {
        Route::Channel {
            channel_id: active, ..
        } => Some(active),
        _ => None,
    }?;
    let channel_list = ChannelList::global(cx);
    let current_is_child_thread = channel_list
        .read(cx)
        .channel(clan_id, current_id)
        .is_some_and(|ch| ch.parent_id == Some(channel_id));
    if current_id != channel_id && !current_is_child_thread {
        return None;
    }
    let target = ClanList::global(cx)
        .read(cx)
        .welcome_channel_id(clan_id)
        .filter(|id| *id != channel_id)
        .or_else(|| {
            channel_list
                .read(cx)
                .default_channel_id(clan_id)
                .filter(|id| *id != channel_id)
        })
        .or_else(|| {
            channel_list
                .read(cx)
                .categories_for_clan(clan_id)
                .iter()
                .flat_map(|category| category.channels.iter())
                .find(|ch| ch.id != channel_id && ch.parent_id.is_none() && ch.visible_in_sidebar())
                .map(|ch| ch.id)
        });
    Some(match target {
        Some(channel_id) => Route::Channel {
            clan_id,
            channel_id,
        },
        None => Route::ClanMembers { clan_id },
    })
}

impl Render for ConfirmDeleteChannelModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;

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
                        Button::new("confirm-delete-channel-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-delete-channel-confirm")
                            .label(self.delete_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                let redirect = redirect_route_after_delete(clan_id, channel_id, cx);
                                let task = ChannelList::global(cx).update(cx, |list, cx| {
                                    list.delete_channel(clan_id, channel_id, cx)
                                });
                                cx.spawn(async move |cx| match task.await {
                                    Ok(()) => {
                                        cx.update(|cx| {
                                            if let Some(route) = redirect {
                                                navigate(cx, route);
                                            }
                                        });
                                    }
                                    Err(err) => {
                                        cx.update(|cx| {
                                            Shell::global(cx).update(cx, |shell, cx| {
                                                shell.error(
                                                    format!("Failed to delete channel: {err}"),
                                                    cx,
                                                );
                                            });
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
