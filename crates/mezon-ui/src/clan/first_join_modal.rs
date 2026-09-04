use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use mezon_store::{AccountStore, ClanList, Settings, invite_id_from_url};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::router::{Route, Router, navigate};
use crate::theme::ActiveTheme;

/// How long after signing up the prompt still counts as helpful rather than nagging. The web
/// client uses the same five minutes.
const NEW_ACCOUNT_WINDOW_SECS: u64 = 5 * 60;

/// Invite codes are snowflakes, so anything shorter than this cannot be one.
const MIN_INVITE_CODE_LEN: usize = 6;

pub struct FirstJoinModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    input: Entity<InputState>,
    invalid: bool,
    _input_sub: Subscription,
}

impl Focusable for FirstJoinModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Whether this account is new enough, and empty enough, to be shown the prompt — and whether
/// now is a moment to raise a modal at all.
pub fn should_prompt(cx: &App) -> bool {
    // Only over the surfaces someone lands on with no clans, the way the web only mounts the
    // popup inside the chat page. Raising it over the invite page would cover the very button
    // it is telling people to press, and a deep link lands there before the account arrives.
    if !matches!(
        Router::global(cx).read(cx).route(),
        Route::Chat | Route::Direct | Route::DirectMessage { .. } | Route::Friends
    ) {
        return false;
    }
    // A brand-new account with no clans is also exactly who the tour autostarts for; whichever
    // wins the race, the other must not paint on top of it.
    if crate::tour::is_running(cx) {
        return false;
    }
    let clans = ClanList::global(cx);
    let clans = clans.read(cx);
    if !clans.has_listed() || !clans.clans.is_empty() {
        return false;
    }
    let Some(account) = AccountStore::global(cx).read(cx).account.as_ref() else {
        return false;
    };
    let created = account.create_time_seconds;
    if created == 0 {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    now.saturating_sub(u64::from(created)) <= NEW_ACCOUNT_WINDOW_SECS
}

/// The invite id inside a full `…/invite/<id>` link, or the input itself when it already looks
/// like a bare code. The web only accepts the full link, but its own placeholder offers a bare
/// code as an example, so both are taken here.
fn invite_code(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(id) = invite_id_from_url(raw) {
        return Some(id);
    }
    let looks_like_code = raw.len() >= MIN_INVITE_CODE_LEN
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    looks_like_code.then(|| raw.to_string())
}

impl FirstJoinModal {
    pub fn open(window: &mut Window, cx: &mut App) {
        let locale: SharedString = Settings::try_global(cx)
            .map(|settings| settings.read(cx).language.clone())
            .unwrap_or_default()
            .into();
        let view = cx.new(|cx| {
            let input = cx.new(|cx| InputState::new(window, cx).height(px(42.)));
            let input_sub = cx.subscribe(&input, |this: &mut Self, _input, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.invalid = false;
                    cx.notify();
                }
            });
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                input,
                invalid: false,
                _input_sub: input_sub,
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn join(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().to_string();
        let Some(code) = invite_code(&raw) else {
            self.invalid = true;
            cx.notify();
            return;
        };
        Self::close(cx);
        navigate(cx, Route::Invite { invite_id: code });
    }
}

impl Render for FirstJoinModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let join_entity = cx.entity();
        let can_join = !self.input.read(cx).value().trim().is_empty();

        div()
            .track_focus(&self.focus_handle)
            .occlude()
            .w(px(520.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_6()
                    .child(
                        div()
                            .id("first-join-close")
                            .absolute()
                            .top(px(12.))
                            .right(px(12.))
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme.tokens.text_theme_primary)
                            .hover(|s| s.bg(theme.tokens.bg_item_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size_4()
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(t("common.firstJoinPopup.ifYouHaveInvitation")),
                            )
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.tokens.text_secondary)
                                    .child(t("common.firstJoinPopup.joinClan")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_center()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(t("common.firstJoinPopup.enterInvitationLink")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.tokens.text_theme_primary)
                                            .child(
                                                t("common.firstJoinPopup.invitationLink")
                                                    .to_uppercase(),
                                            ),
                                    )
                                    .when(self.invalid, |row| {
                                        row.child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(theme.danger_text)
                                                .child(t(
                                                    "common.firstJoinPopup.invitationInvalid",
                                                )),
                                        )
                                    }),
                            )
                            .child(Input::new(&self.input).w_full())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(t("common.firstJoinPopup.example")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_4()
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_sm()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(t("common.firstJoinPopup.or"))
                            .child(
                                div()
                                    .id("first-join-create-clan")
                                    .cursor_pointer()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_secondary)
                                    .hover(|s| s.text_color(theme.tokens.text_theme_primary))
                                    .child(t("common.firstJoinPopup.createYourOwnClan"))
                                    .on_click(|_: &ClickEvent, window, cx| {
                                        Self::close(cx);
                                        crate::clan::create_clan_modal::CreateClanModal::open(
                                            window, cx,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        Button::new("first-join-submit")
                            .primary()
                            .label(t("common.firstJoinPopup.joinClan"))
                            .disabled(!can_join)
                            .on_click(move |_: &ClickEvent, _window, cx| {
                                join_entity.update(cx, |this, cx| this.join(cx));
                            }),
                    ),
            )
    }
}
