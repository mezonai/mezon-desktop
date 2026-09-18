use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, ObjectFit, Render,
    SharedString, Subscription, Window, div, img, prelude::*, px,
};
use mezon_store::{
    AppConfig, ChannelId, ClanList, InviteDetails, InviteEvent, InviteState, InviteStore, Settings,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants, Icon, IconName, h_flex, v_flex};
use crate::image_cache::LruImageCache;
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;
use crate::util::imgproxy;

const MODAL_WIDTH: f32 = 440.;
const LOGO_PX: f32 = 48.;
const LOGO_PROXY_PX: u32 = 100;

pub struct JoinClanModal {
    focus_handle: FocusHandle,
    invite_id: String,
    locale: SharedString,
    joining: bool,
    error: Option<SharedString>,
    image_cache: Entity<LruImageCache>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for JoinClanModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl JoinClanModal {
    pub fn open(invite_id: String, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let locale: SharedString = Settings::try_global(cx)
            .map(|settings| settings.read(cx).language.clone())
            .unwrap_or_else(|| "en".to_string())
            .into();
        let store = InviteStore::global(cx);
        let view = cx.new(|cx| {
            let subscriptions = vec![
                cx.subscribe(&store, |_this: &mut Self, _, _event: &InviteEvent, cx| {
                    cx.notify()
                }),
            ];
            Self {
                focus_handle: cx.focus_handle(),
                invite_id: invite_id.clone(),
                locale,
                joining: false,
                error: None,
                image_cache: cx.new(|cx| {
                    LruImageCache::avatar_thumbnail_small(
                        "join-clan-modal",
                        2,
                        1024 * 1024,
                        1024 * 1024,
                        cx,
                    )
                }),
                _subscriptions: subscriptions,
            }
        });
        store.update(cx, |store, cx| store.ensure_invite(invite_id, cx));
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn details(&self, cx: &App) -> Option<InviteDetails> {
        match InviteStore::global(cx).read(cx).state() {
            InviteState::Loaded(details) => Some(details.clone()),
            _ => None,
        }
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if self.joining {
            return;
        }
        if let Some(details) = self.details(cx) {
            let joined = details.user_joined
                || ClanList::global(cx)
                    .read(cx)
                    .clan(details.clan_id)
                    .is_some();
            if joined {
                let message =
                    mezon_i18n::t(&self.locale, "invitation.acceptModal.toast.alreadyMember");
                Shell::global(cx).update(cx, |shell, cx| shell.info(message, cx));
                open_clan(details.clan_id, details.channel_id, cx);
                Self::close(cx);
                return;
            }
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
                        open_clan(accepted.clan_id, accepted.channel_id, cx);
                        Self::close(cx);
                    }
                    Err(error) => {
                        tracing::warn!("accept invite failed: {error:?}");
                        this.error =
                            Some(mezon_i18n::t(&this.locale, "common.invite.failedToJoin").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn open_clan(clan_id: mezon_store::ClanId, channel_id: ChannelId, cx: &mut App) {
    if channel_id == ChannelId(0) {
        ClanList::global(cx).update(cx, |list, cx| list.select_clan(clan_id, cx));
        navigate(cx, Route::Chat);
    } else {
        navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }
}

impl Render for JoinClanModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
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
                "invitation.acceptModal.memberCount"
            } else {
                "invitation.acceptModal.memberCount_plural"
            },
        )
        .replace("{{count}}", &member_count.to_string());
        let logo = details
            .as_ref()
            .map(|details| {
                imgproxy::proxied(
                    cx,
                    &details.clan_logo,
                    LOGO_PROXY_PX,
                    LOGO_PROXY_PX,
                    "fill-down",
                )
            })
            .filter(|logo| !logo.is_empty());
        let initial: SharedString = clan_name
            .trim()
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "M".to_string())
            .into();
        let badge = match logo {
            Some(logo) => div()
                .image_cache(self.image_cache.clone())
                .child(
                    img(logo)
                        .size(px(LOGO_PX))
                        .rounded(px(6.))
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            None => div()
                .size(px(LOGO_PX))
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

        let loading = matches!(state, InviteState::Loading);
        let accepting = self.joining;
        let accept_label: SharedString = if accepting {
            mezon_i18n::t(&locale, "invitation.acceptModal.joining").into()
        } else {
            mezon_i18n::t(&locale, "invitation.acceptModal.acceptInvite").into()
        };
        let close_hover = theme.bg_hover;
        let entity = cx.entity();

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .w(px(MODAL_WIDTH))
            .rounded(px(8.))
            .bg(theme.tokens.theme_setting_primary)
            .flex()
            .flex_col()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(&locale, "invitation.acceptModal.title")),
                    )
                    .child(
                        div()
                            .id("join-clan-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |s| s.bg(close_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(20.))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
                    ),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_3()
                    .p_6()
                    .child(badge)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(mezon_i18n::t(
                                &locale,
                                "invitation.acceptModal.invitedToJoin",
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(px(30.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.tokens.text_theme_primary)
                            .truncate()
                            .child(clan_name),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(div().size(px(8.)).rounded_full().bg(gpui::rgb(0x22c55e)))
                            .child(member_label),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(div().text_sm().text_color(theme.danger_text).child(error))
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .pt_2()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Button::new("join-clan-no-thanks")
                                        .label(mezon_i18n::t(
                                            &locale,
                                            "invitation.acceptModal.noThanks",
                                        ))
                                        .w_full()
                                        .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
                                ),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Button::new("join-clan-accept")
                                        .label(accept_label)
                                        .primary()
                                        .w_full()
                                        .disabled(accepting || loading)
                                        .on_click(move |_: &ClickEvent, _window, cx| {
                                            entity.update(cx, |this, cx| this.accept(cx));
                                        }),
                                ),
                            ),
                    ),
            )
    }
}
