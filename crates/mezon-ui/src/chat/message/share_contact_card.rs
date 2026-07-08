use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, SharedString, Window, div, prelude::*, px,
    rgba,
};
use mezon_store::{DirectMessageStore, Embed, PresenceStore, UserId};

use super::context::RowCtx;
use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::router::{Route, navigate};

const CARD_WIDTH: f32 = 280.0;
const AVATAR_SIZE: f32 = 48.0;
const STATUS_DOT_SIZE: f32 = 14.0;
const AVATAR_BORDER: u32 = 0xff_ff_ff_4d;

pub fn render_share_contact_card(embed: &Embed, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let user_id = field_value(embed, "user_id");
    let username = field_value(embed, "username");
    let display_name = field_value(embed, "display_name");
    let avatar = field_value(embed, "avatar");

    if user_id.is_empty() || username.is_empty() {
        return div().into_any_element();
    }

    let name = if display_name.is_empty() {
        username
    } else {
        display_name
    };

    let online = user_id
        .parse::<i64>()
        .ok()
        .map(UserId)
        .and_then(|uid| {
            PresenceStore::try_global(ctx.app).map(|store| store.read(ctx.app).is_online(uid))
        })
        .unwrap_or(false);
    let dot_color = if online {
        theme.status_online
    } else {
        theme.text_muted
    };

    let mut avatar_view = Avatar::new()
        .name(name.to_string())
        .size_px(px(AVATAR_SIZE))
        .image_cache(ctx.avatar_cache.clone());
    if !avatar.is_empty() {
        let proxied = crate::util::imgproxy::avatar_url(ctx.app, avatar);
        avatar_view = avatar_view.src(proxied).fallback_src(avatar.to_string());
    }

    let avatar_block = div()
        .relative()
        .flex_shrink_0()
        .size(px(AVATAR_SIZE))
        .rounded_full()
        .border_2()
        .border_color(rgba(AVATAR_BORDER))
        .child(avatar_view)
        .child(
            div()
                .absolute()
                .bottom_0()
                .right(px(-4.))
                .size(px(STATUS_DOT_SIZE))
                .rounded_full()
                .border_2()
                .border_color(theme.tokens.bg_primary)
                .bg(dot_color),
        );

    let header = div().bg(theme.tokens.bg_primary).p_4().child(
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(avatar_block)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(16.))
                            .text_color(theme.tokens.text_secondary)
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.))
                            .text_color(theme.tokens.text_theme_primary)
                            .child(format!("@{username}")),
                    ),
            ),
    );

    let coming_soon = ctx.coming_soon.clone();
    let on_call = move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
        let message = coming_soon.clone();
        Shell::global(cx).update(cx, move |shell, cx| shell.info(message, cx));
    };

    let target_user = user_id.parse::<i64>().ok().map(UserId);
    let message_error: SharedString =
        mezon_i18n::t(ctx.locale, "shareContact.card.messageError").into();
    let on_message = move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
        let Some(user) = target_user else {
            return;
        };
        open_dm_with_user(user, message_error.clone(), cx);
    };

    let footer = div()
        .flex()
        .bg(theme.tokens.bg_primary)
        .child(contact_action_button(
            "share-contact-call",
            IconName::IconPhoneDM,
            mezon_i18n::t(ctx.locale, "shareContact.card.call"),
            false,
            on_call,
            ctx,
        ))
        .child(contact_action_button(
            "share-contact-message",
            IconName::Chat,
            mezon_i18n::t(ctx.locale, "shareContact.card.message"),
            true,
            on_message,
            ctx,
        ));

    div()
        .w(px(CARD_WIDTH))
        .mt_2()
        .rounded(px(10.))
        .overflow_hidden()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .shadow_lg()
        .child(header)
        .child(footer)
        .into_any_element()
}

fn contact_action_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    divided: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let mut button = div()
        .id(id)
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(40.))
        .bg(theme.tokens.bg_secondary_button_hover)
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.tokens.text_theme_primary)
        .cursor_pointer()
        .hover(|s| s.text_color(theme.tokens.text_secondary))
        .on_click(on_click);
    if divided {
        button = button
            .border_l_1()
            .border_color(theme.tokens.border_primary);
    }
    button
        .child(Icon::new(icon).size_4())
        .child(label.into())
        .into_any_element()
}

fn open_dm_with_user(user: UserId, error_message: SharedString, cx: &mut App) {
    let Some(store) = DirectMessageStore::try_global(cx) else {
        return;
    };
    let task = store.update(cx, |store, cx| {
        store.create_dm_with_user(user, String::new(), String::new(), String::new(), cx)
    });
    cx.spawn(async move |cx| match task.await {
        Ok((channel_id, channel_type)) => {
            cx.update(|cx| {
                navigate(
                    cx,
                    Route::DirectMessage {
                        direct_id: channel_id,
                        message_type: channel_type.to_string(),
                    },
                );
            });
        }
        Err(err) => {
            tracing::warn!("create DM failed: {err}");
            cx.update(|cx| {
                Shell::global(cx).update(cx, move |shell, cx| shell.error(error_message, cx));
            });
        }
    })
    .detach();
}

fn field_value<'a>(embed: &'a Embed, name: &str) -> &'a str {
    embed
        .fields
        .iter()
        .find(|field| field.name.as_ref() == name)
        .map(|field| field.value.as_ref())
        .unwrap_or("")
}
