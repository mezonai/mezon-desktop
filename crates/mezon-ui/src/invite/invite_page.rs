use gpui::{
    Context, Entity, FontWeight, ObjectFit, SharedString, Subscription, Window, div, img,
    prelude::*, px,
};
use mezon_store::{
    AppConfig, ClanList, InviteDetails, InviteEvent, InviteState, InviteStore, Settings,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::router::{Route, Router, replace};
use crate::theme::ActiveTheme;

pub struct InvitePage {
    invite_id: String,
    settings: Entity<Settings>,
    joining: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl InvitePage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let store = InviteStore::global(cx);
        let mut subscriptions =
            vec![
                cx.subscribe(&store, |this, _, event: &InviteEvent, cx| match event {
                    InviteEvent::Loaded(details) => this.on_loaded(details, cx),
                    InviteEvent::LoadFailed => cx.notify(),
                }),
            ];
        subscriptions.push(cx.observe(&Router::global(cx), |this, _, cx| {
            this.sync_route(cx);
        }));
        let mut page = Self {
            invite_id: String::new(),
            settings,
            joining: false,
            error: None,
            _subscriptions: subscriptions,
        };
        page.sync_route(cx);
        page
    }

    fn sync_route(&mut self, cx: &mut Context<Self>) {
        let Route::Invite { invite_id } = Router::global(cx).read(cx).route() else {
            return;
        };
        if self.invite_id != invite_id {
            self.invite_id = invite_id.clone();
            self.joining = false;
            self.error = None;
            cx.notify();
        }
        InviteStore::global(cx).update(cx, |store, cx| store.ensure_invite(invite_id, cx));
    }

    fn on_loaded(&mut self, details: &InviteDetails, cx: &mut Context<Self>) {
        if details.user_joined {
            let locale = self.settings.read(cx).language.clone();
            let message = mezon_i18n::t(&locale, "common.invite.alreadyMember");
            Shell::global(cx).update(cx, |shell, cx| shell.info(message, cx));
            replace(
                cx,
                Route::Channel {
                    clan_id: details.clan_id,
                    channel_id: details.channel_id,
                },
            );
        }
        cx.notify();
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if self.joining {
            return;
        }
        let Some(domain) = AppConfig::try_global(cx).map(|config| config.domain_url.clone()) else {
            return;
        };
        let url = format!("{}/invite/{}", domain.trim_end_matches('/'), self.invite_id);
        self.joining = true;
        self.error = None;
        cx.notify();

        let task = ClanList::global(cx).update(cx, |store, cx| store.accept_invite_link(url, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.joining = false;
                match result {
                    Ok(accepted) => {
                        replace(
                            cx,
                            Route::Channel {
                                clan_id: accepted.clan_id,
                                channel_id: accepted.channel_id,
                            },
                        );
                    }
                    Err(error) => {
                        tracing::warn!("accept invite failed: {error:?}");
                        let locale = this.settings.read(cx).language.clone();
                        this.error =
                            Some(mezon_i18n::t(&locale, "common.invite.failedToJoin").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for InvitePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let state = InviteStore::global(cx).read(cx).state().clone();
        let details = match &state {
            InviteState::Loaded(details) => Some(details.clone()),
            _ => None,
        };
        let clan_name: SharedString = details
            .as_ref()
            .map(|details| details.clan_name.clone())
            .filter(|name| !name.is_empty())
            .map(SharedString::from)
            .unwrap_or_else(|| mezon_i18n::t(&locale, "common.invite.defaultClanName").into());
        let member_count = details
            .as_ref()
            .map(|details| details.member_count.max(1))
            .unwrap_or(1);
        let member_label = mezon_i18n::t(
            &locale,
            if member_count == 1 {
                "common.invite.member"
            } else {
                "common.invite.member_plural"
            },
        );
        let logo = details
            .as_ref()
            .map(|details| details.clan_logo.clone())
            .filter(|logo| !logo.is_empty());
        let initial: SharedString = clan_name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "M".to_string())
            .into();

        let badge = match logo {
            Some(logo) => img(logo)
                .size(px(48.))
                .rounded(px(6.))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .size(px(48.))
                .rounded(px(6.))
                .bg(theme.bg_secondary)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(28.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.tokens.text_theme_primary)
                .child(initial)
                .into_any_element(),
        };

        let accepting = self.joining;
        let button_label: SharedString = if accepting {
            mezon_i18n::t(&locale, "common.invite.joining").into()
        } else {
            mezon_i18n::t(&locale, "common.invite.acceptInvite").into()
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.bg_primary)
            .child(
                v_flex()
                    .w(px(440.))
                    .items_center()
                    .gap_3()
                    .p_6()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.theme_setting_primary)
                    .shadow_lg()
                    .child(badge)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(mezon_i18n::t(&locale, "common.invite.invitedToJoin")),
                    )
                    .child(
                        div()
                            .text_size(px(30.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(clan_name),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(div().size(px(8.)).rounded_full().bg(gpui::rgb(0x22c55e)))
                            .child(format!("{member_count} {member_label}")),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(div().text_sm().text_color(theme.danger_text).child(error))
                    })
                    .child(
                        Button::new("invite-accept")
                            .label(button_label)
                            .primary()
                            .w_full()
                            .disabled(accepting)
                            .on_click(cx.listener(|this, _, _window, cx| this.accept(cx))),
                    ),
            )
    }
}
