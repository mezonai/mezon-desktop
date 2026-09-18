use gpui::{
    Context, Entity, FocusHandle, FontWeight, SharedString, Window, div, prelude::*, px, rgb,
};
use mezon_store::{ClanId, ClanList, UserId};

use super::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Checkbox, Icon, IconName, h_flex, v_flex,
};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const AVATAR_PX: f32 = 80.;
const OWNER_ICON_COLOR: u32 = 0xF0B132;

pub(super) struct TransferOwnerParty {
    pub(super) name: SharedString,
    pub(super) avatar: SharedString,
}

pub(super) struct TransferOwnerModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) new_owner_id: UserId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) confirmation: SharedString,
    pub(super) transfer_label: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) success_message: SharedString,
    pub(super) error_message: SharedString,
    pub(super) current_owner: TransferOwnerParty,
    pub(super) new_owner: TransferOwnerParty,
    pub(super) avatar_cache: Entity<LruImageCache>,
    pub(super) acknowledged: bool,
    pub(super) pending: bool,
}

impl TransferOwnerModal {
    fn transfer(&mut self, cx: &mut Context<Self>) {
        if !self.acknowledged || self.pending {
            return;
        }
        self.pending = true;
        cx.notify();
        let clan_id = self.clan_id;
        let new_owner_id = self.new_owner_id;
        let success_message = self.success_message.clone();
        let error_message = self.error_message.clone();
        let view_id = cx.entity_id();
        let task = ClanList::global(cx).update(cx, |store, cx| {
            store.transfer_ownership(clan_id, new_owner_id, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.pending = false;
                cx.notify();
            })
            .ok();
            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    match result {
                        Ok(()) => shell.success(success_message, cx),
                        Err(error) => {
                            tracing::error!(
                                "transfer clan {clan_id} ownership to {new_owner_id} failed: {error}"
                            );
                            shell.error(error_message, cx);
                        }
                    }
                    shell.close_modal_view(view_id, cx);
                });
            });
        })
        .detach();
    }

    fn party(
        &self,
        party: &TransferOwnerParty,
        is_new_owner: bool,
        theme: &Theme,
    ) -> impl IntoElement {
        v_flex()
            .relative()
            .w(px(160.))
            .items_center()
            .justify_center()
            .gap_3()
            .when(!is_new_owner, |el| el.opacity(0.75))
            .when(is_new_owner, |el| {
                el.child(
                    div().absolute().top(px(-14.)).child(
                        Icon::new(IconName::OwnerIcon)
                            .size(px(18.))
                            .text_color(rgb(OWNER_ICON_COLOR)),
                    ),
                )
            })
            .child(
                Avatar::new()
                    .src(party.avatar.clone())
                    .name(party.name.clone())
                    .size_px(px(AVATAR_PX))
                    .image_cache(self.avatar_cache.clone()),
            )
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(party.name.clone()),
            )
    }
}

impl Render for TransferOwnerModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let acknowledged = self.acknowledged;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.pending {
                    return;
                }
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(520.))
            .gap_4()
            .p(px(24.))
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(self.title.clone()),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap(px(24.))
                    .p(px(12.))
                    .text_color(theme.text_primary)
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_center()
                            .child(self.description.clone()),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(self.party(&self.current_owner, false, theme))
                            .child(
                                div().mt(px(24.)).child(
                                    Icon::new(IconName::TransferOwner)
                                        .size(px(60.))
                                        .text_color(theme.text_primary),
                                ),
                            )
                            .child(self.party(&self.new_owner, true, theme)),
                    )
                    .child(
                        h_flex()
                            .id("confirm-transfer-row")
                            .w_full()
                            .items_start()
                            .gap_2()
                            .cursor_pointer()
                            .text_size(px(14.))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.acknowledged = !this.acknowledged;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .pt(px(2.))
                                    .child(Checkbox::new("confirm-transfer").checked(acknowledged)),
                            )
                            .child(div().flex_1().min_w_0().child(self.confirmation.clone())),
                    ),
            )
            .child(
                h_flex()
                    .justify_center()
                    .gap_4()
                    .child(
                        Button::new("transfer-owner-confirm")
                            .label(self.transfer_label.clone())
                            .danger()
                            .disabled(!acknowledged)
                            .loading(self.pending)
                            .on_click(cx.listener(|this, _, _window, cx| this.transfer(cx))),
                    )
                    .child(
                        Button::new("transfer-owner-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .disabled(self.pending)
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    ),
            )
    }
}
