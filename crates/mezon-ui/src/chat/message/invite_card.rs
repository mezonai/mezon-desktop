use gpui::{
    AnyElement, App, FontWeight, ObjectFit, SharedString, div, img, prelude::*, px, rgb, rgba,
};
use mezon_store::{ClanId, ClanList, InvitePreview};

use super::content::open_message_link;
use super::context::RowCtx;
use crate::components::primitives::{Icon, IconName};
use crate::router::{Route, navigate};

const COMMUNITY_GREEN: u32 = 0x22_c5_5e;
const JOIN_GREEN: u32 = 0x0a_9f_59;
const JOIN_GREEN_HOVER: u32 = 0x0b_8a_4f;
const ERROR_TEXT: u32 = 0xfc_a5_a5;
const ERROR_BORDER: u32 = 0xf8_71_71_4d;
const WHITE: u32 = 0xff_ff_ff;

pub fn render_invite_card(invite: &InvitePreview, ctx: &RowCtx) -> AnyElement {
    if invite.is_error {
        return render_invalid_invite(ctx);
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
    let joined_clan = joined_clan_id(invite, ctx.app);
    let is_joined = joined_clan.is_some();

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
                .object_fit(ObjectFit::Cover),
        );
    }

    let mut banner = div()
        .relative()
        .w_full()
        .h(px(76.))
        .overflow_hidden()
        .bg(theme.tokens.theme_setting_nav);
    if !invite.banner_proxied.is_empty() {
        banner = banner.child(
            img(invite.banner_proxied.clone())
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Cover),
        );
    }

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
                .child(SharedString::from(title.to_uppercase())),
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
                .child(
                    Icon::new(IconName::CheckIcon)
                        .size(px(12.))
                        .text_color(theme.tokens.text_theme_primary),
                ),
        );
    }

    let member_label = member_count_label(ctx.locale, invite.member_count);
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
                .child(
                    div()
                        .w(px(8.))
                        .h(px(8.))
                        .rounded_full()
                        .bg(rgb(COMMUNITY_GREEN)),
                )
                .child(member_label),
        );

    let open_url = invite.url.clone();
    let header_block = div()
        .id(SharedString::from(format!("invite-open-{}", invite.url)))
        .cursor_pointer()
        .on_click(move |_, _, cx| open_message_link(open_url.clone(), cx))
        .child(title_row)
        .child(member_row);

    let button_label = if is_joined {
        mezon_i18n::t(ctx.locale, "linkMessageInvite.goToClan")
    } else {
        mezon_i18n::t(ctx.locale, "linkMessageInvite.join")
    };
    let join_url = invite.url.clone();
    let button = div()
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
        .hover(|s| s.bg(rgb(JOIN_GREEN_HOVER)))
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(WHITE))
        .on_click(move |_, _, cx| {
            handle_join_or_goto(joined_clan, &join_url, cx);
        })
        .child(button_label);

    let body = div()
        .px_4()
        .pb_4()
        .pt_10()
        .child(header_block)
        .child(button);

    let card = div()
        .relative()
        .w_full()
        .overflow_hidden()
        .rounded_2xl()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(theme.tokens.bg_item_theme_hover)
        .child(banner)
        .child(
            div()
                .absolute()
                .top(px(40.))
                .left(px(16.))
                .w(px(72.))
                .h(px(72.))
                .overflow_hidden()
                .rounded(px(22.))
                .border_4()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.theme_setting_primary)
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

fn render_invalid_invite(ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let message = mezon_i18n::t(ctx.locale, "linkMessageInvite.invalidInvite.message");
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
                        .child(SharedString::from(message)),
                ),
        )
        .into_any_element()
}

fn joined_clan_id(invite: &InvitePreview, cx: &App) -> Option<ClanId> {
    let clan_id = invite
        .clan_id
        .as_deref()
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(ClanId)
        .filter(|id| !id.is_zero())?;
    ClanList::global(cx).read(cx).clan(clan_id).map(|_| clan_id)
}

fn handle_join_or_goto(joined_clan: Option<ClanId>, url: &str, cx: &mut App) {
    match joined_clan {
        Some(clan_id) => {
            ClanList::global(cx).update(cx, |list, cx| list.select_clan(clan_id, cx));
            navigate(cx, Route::Chat);
        }
        None => open_message_link(url.to_string(), cx),
    }
}

fn member_count_label(locale: &str, count: i64) -> SharedString {
    let key = if count == 1 {
        "linkMessageInvite.memberCount"
    } else {
        "linkMessageInvite.memberCount_plural"
    };
    mezon_i18n::t(locale, key)
        .replace("{{count}}", &count.max(0).to_string())
        .into()
}
