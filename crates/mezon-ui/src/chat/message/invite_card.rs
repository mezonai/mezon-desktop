use crate::components::primitives::{Icon, IconName};

use gpui::{
    AnyElement, App, FontWeight, ObjectFit, SharedString, div, img, prelude::*, px, rgb, rgba,
};
use mezon_store::{AcceptInviteError, AcceptInviteResult, ClanId, ClanList, InvitePreview};

use super::content::{SelectableSectionCursor, SelectableTextContext, open_message_link};
use super::context::RowCtx;
use crate::app::shell::Shell;
use crate::router::{Route, navigate};

const COMMUNITY_GREEN: u32 = 0x22_c5_5e;
const JOIN_GREEN: u32 = 0x0a_9f_59;
const JOIN_GREEN_HOVER: u32 = 0x0b_8a_4f;
const ERROR_TEXT: u32 = 0xfc_a5_a5;
const ERROR_BORDER: u32 = 0xf8_71_71_4d;
const WHITE: u32 = 0xff_ff_ff;

pub(crate) fn selectable_invite_text(invite: &InvitePreview, locale: &str) -> String {
    if invite.is_error {
        return mezon_i18n::t(locale, "linkMessageInvite.invalidInvite.message").to_string();
    }
    let title = if invite.title.is_empty() {
        mezon_i18n::t(locale, "linkMessageInvite.unknownClan").to_string()
    } else {
        invite.title.to_string()
    };
    format!(
        "{}\n{}",
        title.to_uppercase(),
        member_count_label(locale, invite.member_count)
    )
}

const INVITE_CARD_RADIUS: f32 = 16.0;
const INVITE_AVATAR_BOX: f32 = 72.0;
const INVITE_AVATAR_RADIUS: f32 = 22.0;
const INVITE_AVATAR_INNER_RADIUS: f32 = 18.0;

pub fn render_invite_card(
    invite: &InvitePreview,
    base: Option<usize>,
    selection_context: Option<&SelectableTextContext>,
    ctx: &RowCtx,
) -> AnyElement {
    if invite.is_error {
        return render_invalid_invite(base, selection_context, ctx);
    }

    let theme = ctx.theme;
    let title = if invite.title.is_empty() {
        SharedString::from(mezon_i18n::t(ctx.locale, "linkMessageInvite.unknownClan"))
    } else {
        invite.title.clone()
    };
    let initial: SharedString = title
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "M".to_string())
        .into();
    let clan_list = ClanList::global(ctx.app);
    let clans = clan_list.read(ctx.app);
    let joined_clan = joined_clan_id(invite, clans);
    let is_joined = joined_clan.is_some();
    let joining = clans.is_joining_invite(&invite.url);
    let join_failed = clans.has_invite_join_error(&invite.url);

    let mut avatar_inner = div().size_full();
    if invite.image_proxied.is_empty() {
        avatar_inner = avatar_inner.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.tokens.text_theme_primary)
                .text_size(px(30.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(initial),
        );
    } else {
        avatar_inner = avatar_inner.child(
            img(invite.image_proxied.clone())
                .size_full()
                .rounded(px(INVITE_AVATAR_INNER_RADIUS))
                .object_fit(ObjectFit::Cover),
        );
    }

    let card_radius = px(INVITE_CARD_RADIUS);
    let mut banner = div()
        .relative()
        .w_full()
        .h(px(76.))
        .flex_shrink_0()
        .rounded_tl(card_radius)
        .rounded_tr(card_radius)
        .overflow_hidden()
        .bg(theme.tokens.theme_setting_nav)
        .image_cache(ctx.ogp_cache.clone());
    if !invite.banner_proxied.is_empty() {
        banner = banner.child(
            img(invite.banner_proxied.clone())
                .w_full()
                .h_full()
                .rounded_tl(card_radius)
                .rounded_tr(card_radius)
                .object_fit(ObjectFit::Cover),
        );
    }

    let mut cursor = base.map(SelectableSectionCursor::new);
    let uppercase_title = SharedString::from(title.to_uppercase());
    let title_element = cursor
        .as_mut()
        .and_then(|cursor| cursor.section(&uppercase_title))
        .zip(selection_context)
        .map(|(range, selection)| selection.end_truncated_text_node(&uppercase_title, range))
        .unwrap_or_else(|| gpui::StyledText::new(uppercase_title));
    let mut title_row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .pt_2()
                .min_w_0()
                .truncate()
                .text_size(px(18.))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(theme.tokens.text_theme_primary)
                .cursor(gpui::CursorStyle::IBeam)
                .child(title_element),
        );
    if invite.is_community {
        title_row = title_row.child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(20.))
                .h(px(20.))
                .rounded_full()
                .bg(rgb(COMMUNITY_GREEN))
                .child(Icon::new(IconName::CHECK_16).text_color(theme.tokens.text_theme_primary)),
        );
    }

    let member_label = member_count_label(ctx.locale, invite.member_count);
    let member_element = cursor
        .as_mut()
        .and_then(|cursor| cursor.section(&member_label))
        .zip(selection_context)
        .map(|(range, selection)| selection.text_node(&member_label, range))
        .unwrap_or_else(|| gpui::StyledText::new(member_label));
    let member_row = div()
        .mt_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .text_size(px(14.))
        .text_color(theme.tokens.text_theme_primary)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .cursor(gpui::CursorStyle::IBeam)
                .child(
                    div()
                        .w(px(8.))
                        .h(px(8.))
                        .rounded_full()
                        .bg(rgb(COMMUNITY_GREEN)),
                )
                .child(member_element),
        );

    let open_url = invite.url.clone();
    let open_selection = ctx.selection.clone();
    let header_block = div()
        .id(SharedString::from(format!("invite-open-{}", invite.url)))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            if !open_selection.borrow().has_selection() {
                open_message_link(open_url.clone(), cx);
            }
        })
        .child(title_row)
        .child(member_row);

    let button_label = if joining {
        mezon_i18n::t(ctx.locale, "linkMessageInvite.joining")
    } else if is_joined {
        mezon_i18n::t(ctx.locale, "linkMessageInvite.goToClan")
    } else {
        mezon_i18n::t(ctx.locale, "linkMessageInvite.join")
    };
    let join_url = invite.url.clone();
    let join_selection = ctx.selection.clone();
    let failed_to_join =
        SharedString::from(mezon_i18n::t(ctx.locale, "linkMessageInvite.failedToJoin"));
    let failed_to_join_for_click = failed_to_join.clone();
    let mut button = div()
        .id(SharedString::from(format!("invite-join-{}", invite.url)))
        .mt_4()
        .w_full()
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .cursor_pointer()
        .bg(rgb(JOIN_GREEN))
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(WHITE));
    if joining {
        button = button.opacity(0.6).cursor_default();
    } else {
        button = button
            .hover(|s| s.bg(rgb(JOIN_GREEN_HOVER)))
            .on_click(move |_, _, cx| {
                if !join_selection.borrow().has_selection() {
                    handle_join_or_goto(
                        joined_clan,
                        &join_url,
                        failed_to_join_for_click.clone(),
                        cx,
                    );
                }
            });
    }
    let button = button.child(button_label);

    let mut body = div().px_4().pb_4().pt_10().child(header_block);
    if join_failed {
        body = body.child(
            div()
                .mt_2()
                .text_size(px(12.))
                .text_color(rgb(ERROR_TEXT))
                .child(failed_to_join),
        );
    }
    let body = body.child(button);

    let card = div()
        .relative()
        .w_full()
        .rounded(card_radius)
        .overflow_hidden()
        .bg(theme.tokens.bg_item_theme_hover)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .child(banner)
        .child(
            div()
                .absolute()
                .top(px(40.))
                .left(px(16.))
                .w(px(INVITE_AVATAR_BOX))
                .h(px(INVITE_AVATAR_BOX))
                .overflow_hidden()
                .rounded(px(INVITE_AVATAR_RADIUS))
                .border_4()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.theme_setting_primary)
                .image_cache(ctx.avatar_cache.clone())
                .child(avatar_inner),
        )
        .child(body);

    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .w_full()
        .max_w(px(350.))
        .mt_1()
        .child(card)
        .into_any_element()
}

fn render_invalid_invite(
    base: Option<usize>,
    selection_context: Option<&SelectableTextContext>,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let message = mezon_i18n::t(ctx.locale, "linkMessageInvite.invalidInvite.message");
    let message_element = base
        .zip(selection_context)
        .map(|(base, selection)| selection.text_node(message, base..base + message.len()))
        .unwrap_or_else(|| gpui::StyledText::new(message));
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .w_full()
        .max_w(px(350.))
        .mt_1()
        .child(
            div()
                .p(px(10.))
                .rounded_lg()
                .border_1()
                .border_color(rgba(ERROR_BORDER))
                .bg(theme.tokens.theme_setting_nav)
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(rgb(ERROR_TEXT))
                        .cursor(gpui::CursorStyle::IBeam)
                        .child(message_element),
                ),
        )
        .into_any_element()
}

fn joined_clan_id(invite: &InvitePreview, clans: &ClanList) -> Option<ClanId> {
    let clan_id = invite
        .clan_id
        .as_deref()
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(ClanId)
        .filter(|id| !id.is_zero())?;
    clans.clan(clan_id).map(|_| clan_id)
}

fn handle_join_or_goto(
    joined_clan: Option<ClanId>,
    url: &str,
    failed_to_join: SharedString,
    cx: &mut App,
) {
    match joined_clan {
        Some(clan_id) => {
            ClanList::global(cx).update(cx, |list, cx| list.select_clan(clan_id, cx));
            navigate(cx, Route::Chat);
        }
        None => accept_invite_link(url, failed_to_join, cx),
    }
}

fn accept_invite_link(url: &str, failed_to_join: SharedString, cx: &mut App) {
    let Some(store) = ClanList::try_global(cx) else {
        return;
    };
    let task = store.update(cx, |store, cx| {
        store.accept_invite_link(url.to_string(), cx)
    });
    cx.spawn(async move |cx| match task.await {
        Ok(accept) => {
            cx.update(|cx| navigate_after_invite_accept(accept, cx));
        }
        Err(AcceptInviteError::AlreadyJoining) => {}
        Err(err) => {
            tracing::warn!("accept invite failed: {err:?}");
            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.error(failed_to_join, cx));
            });
        }
    })
    .detach();
}

fn navigate_after_invite_accept(accept: AcceptInviteResult, cx: &mut App) {
    let channel_id = if accept.channel_id.is_zero() {
        ClanList::global(cx)
            .read(cx)
            .welcome_channel_id(accept.clan_id)
            .filter(|id| !id.is_zero())
            .unwrap_or(accept.channel_id)
    } else {
        accept.channel_id
    };
    if channel_id.is_zero() {
        ClanList::global(cx).update(cx, |list, cx| list.select_clan(accept.clan_id, cx));
        navigate(cx, Route::Chat);
        return;
    }
    navigate(
        cx,
        Route::Channel {
            clan_id: accept.clan_id,
            channel_id,
        },
    );
}

pub(crate) fn member_count_label(locale: &str, count: i64) -> SharedString {
    let key = if count == 1 {
        "linkMessageInvite.memberCount"
    } else {
        "linkMessageInvite.memberCount_plural"
    };
    mezon_i18n::t(locale, key)
        .replace("{{count}}", &count.max(0).to_string())
        .into()
}
