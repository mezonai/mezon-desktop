use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelWebhook, ClanWebhook, WebhookStore};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) enum WebhookDeleteTarget {
    Channel(ChannelWebhook),
    Clan(ClanWebhook),
}

impl Clone for WebhookDeleteTarget {
    fn clone(&self) -> Self {
        match self {
            Self::Channel(webhook) => Self::Channel(webhook.clone()),
            Self::Clan(webhook) => Self::Clan(webhook.clone()),
        }
    }
}

impl WebhookDeleteTarget {
    pub(super) fn webhook_name(&self) -> &str {
        match self {
            Self::Channel(webhook) => &webhook.webhook_name,
            Self::Clan(webhook) => &webhook.webhook_name,
        }
    }

    pub(super) fn delete_title_key(&self) -> &'static str {
        match self {
            Self::Channel(_) => "clanIntegrationsSetting.webhooksEdit.deleteChannelWebhookTitle",
            Self::Clan(_) => "clanIntegrationsSetting.webhooksEdit.deleteClanWebhookTitle",
        }
    }
}

pub(super) struct ConfirmDeleteWebhookModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) target: WebhookDeleteTarget,
    pub(super) locale: String,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) delete_label: SharedString,
}

impl Render for ConfirmDeleteWebhookModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let owner = cx.entity().entity_id();
        let target = self.target.clone();
        let locale = self.locale.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(440.))
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
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(self.description.clone()),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("confirm-delete-webhook-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-delete-webhook-confirm")
                            .label(self.delete_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                let name = target.webhook_name().to_string();
                                let locale_for_task = locale.clone();
                                let task =
                                    WebhookStore::global(cx).update(
                                        cx,
                                        |store, cx| match &target {
                                            WebhookDeleteTarget::Channel(webhook) => {
                                                store.delete_channel_webhook(webhook, cx)
                                            }
                                            WebhookDeleteTarget::Clan(webhook) => {
                                                store.delete_clan_webhook(webhook, cx)
                                            }
                                        },
                                    );
                                cx.spawn(async move |cx| match task.await {
                                    Ok(()) => {
                                        cx.update(|cx| {
                                            Shell::global(cx).update(cx, |shell, cx| {
                                                shell.success(
                                                    mezon_i18n::t(
                                                        &locale_for_task,
                                                        "integrations.toast.deleteSuccess",
                                                    )
                                                    .replace("{{name}}", &name),
                                                    cx,
                                                );
                                                shell.close_modal_if_current(owner, cx);
                                            });
                                        });
                                    }
                                    Err(err) => {
                                        cx.update(|cx| {
                                            Shell::global(cx).update(cx, |shell, cx| {
                                                shell.error(err, cx);
                                            });
                                        });
                                    }
                                })
                                .detach();
                            }),
                    ),
            )
    }
}
