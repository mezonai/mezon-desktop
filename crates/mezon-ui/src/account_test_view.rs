use std::sync::Arc;

use gpui::{div, prelude::*, Context, Entity, FontWeight, Window};
use mezon_client::{
    transport::{ApiAccount, ApiChannelDesc, ApiClanDesc},
    AppApi,
};

use crate::theme::Theme;

pub struct AccountTestView {
    api: Arc<AppApi>,
    loading: bool,
    account: Option<ApiAccount>,
    clan: Option<ApiClanDesc>,
    channels: Vec<ApiChannelDesc>,
    error: Option<String>,
    started: bool,
}

impl AccountTestView {
    pub fn new(api: Arc<AppApi>) -> Self {
        Self {
            api,
            loading: false,
            account: None,
            clan: None,
            channels: Vec::new(),
            error: None,
            started: false,
        }
    }

    fn start(&mut self, entity: Entity<Self>, cx: &mut Context<Self>) {
        if self.started {
            return;
        }

        self.started = true;
        self.loading = true;
        self.error = None;

        let api = self.api.clone();
        cx.spawn(async move |_, cx: &mut gpui::AsyncApp| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;

            let result = match api.get_account().await {
                Ok(account) => match api.list_clan_descs().await {
                    Ok(clans) => {
                        let clan = clans.into_iter().next();
                        match clan.as_ref() {
                            Some(clan) => match api.list_channel_descs(&clan.clan_id).await {
                                Ok(channels) => Ok((account, Some(clan.clone()), channels)),
                                Err(e) => Err(e),
                            },
                            None => Ok((account, None, Vec::new())),
                        }
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            cx.update(|cx| {
                entity.update(cx, |this, cx| {
                    this.loading = false;
                    match result {
                        Ok((account, clan, channels)) => {
                            this.account = Some(account);
                            this.clan = clan;
                            this.channels = channels;
                            this.error = None;
                        }
                        Err(e) => {
                            this.account = None;
                            this.clan = None;
                            this.channels.clear();
                            this.error = Some(e.to_string());
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

impl Render for AccountTestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start(cx.entity().clone(), cx);

        let theme = Theme::dark();
        let mut card = div()
            .flex()
            .flex_col()
            .gap_4()
            .w(gpui::px(420.0))
            .p_8()
            .rounded_lg()
            .bg(theme.bg_secondary);

        card = card
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child("Account API Test"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child("Calls get_account and lists channels over shared TCP transport."),
            );

        if self.loading {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child("Loading account..."),
            );
        } else if let Some(error) = &self.error {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(theme.status_dnd)
                    .child(format!("get_account failed: {error}")),
            );
        } else if let Some(account) = &self.account {
            card = card
                .child(account_row(&theme, "User ID", &account.user_id))
                .child(account_row(&theme, "Username", &account.username))
                .child(account_row(
                    &theme,
                    "Email",
                    account.email.as_deref().unwrap_or("-"),
                ))
                .child(account_row(
                    &theme,
                    "Display name",
                    account.display_name.as_deref().unwrap_or("-"),
                ))
                .child(account_row(
                    &theme,
                    "Clan",
                    self.clan
                        .as_ref()
                        .map(|clan| clan.clan_name.as_str())
                        .unwrap_or("-"),
                ))
                .child(
                    div()
                        .mt_4()
                        .text_lg()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(format!("Channels ({})", self.channels.len())),
                );

            if self.channels.is_empty() {
                card = card.child(
                    div()
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child("No channels returned."),
                );
            } else {
                for channel in &self.channels {
                    card = card.child(channel_row(&theme, channel));
                }
            }
        }

        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .child(card)
    }
}

fn channel_row(theme: &Theme, channel: &ApiChannelDesc) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .bg(theme.bg_tertiary)
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(channel.channel_label.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(format!(
                    "id={} type={}",
                    channel.channel_id, channel.channel_type
                )),
        )
}

fn account_row(theme: &Theme, label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(value.to_owned()),
        )
}
