use gpui::{AnyElement, App, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelType, ClanMembersStore, Message, MessageCode,
    MessageId, MessagesStore, PinnedMessagesStore, ThreadsStore,
};

use super::content::{
    SelectableSectionCursor, SelectableTextContext, append_selectable_system_mention_spans,
    memoized_selectable_message_layout, render_system_message_content,
};
use super::context::{CONTENT_INSET, OnboardingContext, RowCtx, WelcomeContext};
use super::time::format_message_time;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::router::{Route, navigate};

const SYSTEM_ARROW_ICON: f32 = 20.;
const SYSTEM_SMALL_ICON: f32 = 18.;

pub fn render_system_message(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let primary = theme.tokens.text_theme_primary;
    let icon_fill = theme.tokens.bg_icon_theme;
    let is_custom = matches!(
        msg.code,
        MessageCode::CreatePin | MessageCode::CreateThread | MessageCode::DeleteThread
    );

    let (icon, icon_size) = match msg.code {
        MessageCode::CreatePin => (IconName::PinRight, px(SYSTEM_SMALL_ICON)),
        MessageCode::CreateThread | MessageCode::DeleteThread => {
            (IconName::ThreadIcon, px(SYSTEM_SMALL_ICON))
        }
        MessageCode::AuditLog => (IconName::AuditLogIcon, px(SYSTEM_ARROW_ICON)),
        MessageCode::UpcomingEvent => (IconName::UpcomingEventIcon, px(SYSTEM_ARROW_ICON)),
        _ => (IconName::WelcomeIcon, px(SYSTEM_ARROW_ICON)),
    };

    let text_gap = if is_custom { px(24.) } else { px(16.) };
    let mut content_line = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .max_w_full()
        .items_baseline()
        .min_w_0()
        .cursor(gpui::CursorStyle::IBeam)
        .text_sm()
        .text_color(primary);
    let selection_layout = is_custom.then(|| memoized_selectable_message_layout(msg, ctx));
    let selection_context = selection_layout.as_ref().map(|layout| {
        SelectableTextContext::new(msg.id, layout.text.clone(), ctx.selection.clone())
    });
    let mut selection_cursor = SelectableSectionCursor::new(0);
    if is_custom && let Some(selection_context) = selection_context.as_ref() {
        content_line = append_selectable_system_mention_spans(
            content_line,
            msg,
            ctx,
            &mut selection_cursor,
            selection_context,
        );
        content_line = append_system_suffix(
            msg,
            ctx,
            content_line,
            &mut selection_cursor,
            selection_context,
        );
    } else if !is_custom {
        content_line = content_line.child(render_system_message_content(msg, ctx, false));
    }

    let row = div()
        .id(("msg-sys", msg.id.0 as usize))
        .flex()
        .flex_row()
        .items_start()
        .min_h(px(32.))
        .w_full()
        .pl(px(20.))
        .pr(px(12.))
        .pt_2()
        .when(is_custom, |d| d.pb_2())
        .when(!ctx.suppress_hover, |d| {
            let bg = theme.bg_hover;
            d.hover(move |s| s.bg(bg))
        })
        .child(system_message_icon(
            msg.code, icon, icon_size, primary, icon_fill,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .max_w_full()
                .items_start()
                .min_w_0()
                .pl(text_gap)
                .child(content_line)
                .child(
                    div()
                        .ml_1()
                        .pt(px(5.))
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(primary)
                        .child(format_message_time(
                            &msg.time_hhmm,
                            msg.local_date,
                            ctx.locale,
                            ctx.now,
                        )),
                ),
        );

    let reactions = super::parts::render_reactions(msg, ctx)
        .map(|pills| div().pl(px(CONTENT_INSET)).child(pills));
    let is_welcome = matches!(msg.code, MessageCode::Welcome);

    if reactions.is_none() && !is_welcome {
        return row.into_any_element();
    }

    let mut col = div().flex().flex_col().w_full().child(row);
    if let Some(reactions) = reactions {
        col = col.child(reactions);
    }
    if is_welcome {
        col = col.child(render_wave_button(msg, ctx));
    }
    col.into_any_element()
}

const WAVE_STICKER_PATHS: &[&str] = &[
    "stickers/hellomezon.gif",
    "stickers/music_boy.gif",
    "stickers/music_girl.gif",
    "stickers/d1.gif",
    "stickers/d2.gif",
    "stickers/d3.gif",
    "stickers/d4.gif",
    "stickers/d5.gif",
    "stickers/whatsapp.gif",
    "stickers/zalo.gif",
    "stickers/mezon.gif",
    "stickers/telegram.gif",
    "stickers/mezon.gif",
    "stickers/slack.gif",
    "stickers/mezon.gif",
    "stickers/discord.gif",
    "stickers/mezon.gif",
    "landing-page-mezon/2021919345600368640.gif",
];
const WAVE_STICKER_NAME: &str = "hello";

fn render_wave_button(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let sticker_idx = if msg.create_time == 0 {
        0
    } else {
        (msg.create_time as usize) % WAVE_STICKER_PATHS.len()
    };
    let sticker_url =
        crate::util::imgproxy::cdn_asset_url(ctx.app, WAVE_STICKER_PATHS[sticker_idx]);
    let label = mezon_i18n::t(ctx.locale, "dmMessage.waveWelcome");
    let selection = ctx.selection.clone();

    div()
        .flex()
        .gap_2()
        .mt_2()
        .ml(px(CONTENT_INSET))
        .child(
            div()
                .id("wave-say-hi")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .px_3()
                .rounded_md()
                .bg(theme.bg_tertiary)
                .cursor_pointer()
                .on_click({
                    let url = sticker_url.clone();
                    move |_, _, cx| {
                        if selection.borrow().has_selection() {
                            return;
                        }
                        let badge = BadgeService::global(cx).read(cx);
                        let Some(user_id) = badge.current_user_id(cx) else {
                            return;
                        };
                        let sender_id = user_id.0.to_string();
                        MessagesStore::global(cx).update(cx, |store, cx| {
                            store.send_sticker(
                                url.clone(),
                                WAVE_STICKER_NAME.to_string(),
                                sender_id,
                                String::new(),
                                cx,
                            );
                        });
                    }
                })
                .child(
                    gpui::img(SharedString::from(sticker_url.clone()))
                        .size(px(32.))
                        .flex_none(),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_secondary)
                        .child(label),
                ),
        )
        .into_any_element()
}

fn system_message_icon(
    code: MessageCode,
    icon: IconName,
    size: gpui::Pixels,
    primary: gpui::Rgba,
    icon_fill: gpui::Rgba,
) -> AnyElement {
    let color = system_icon_color(code, primary, icon_fill);
    div()
        .flex_none()
        .w(size)
        .h(size)
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(size).text_color(color))
        .into_any_element()
}

fn system_icon_color(code: MessageCode, primary: gpui::Rgba, icon_fill: gpui::Rgba) -> gpui::Rgba {
    match code {
        MessageCode::Welcome => gpui::rgb(0x16_a3_4a),
        MessageCode::AuditLog => gpui::rgb(0x18_6c_f2),
        MessageCode::UpcomingEvent => gpui::rgb(0xa31616),
        MessageCode::CreateThread | MessageCode::DeleteThread => icon_fill,
        MessageCode::CreatePin => primary,
        _ => primary,
    }
}

fn append_system_suffix(
    msg: &Message,
    ctx: &RowCtx,
    mut row: gpui::Div,
    cursor: &mut SelectableSectionCursor,
    selection_context: &SelectableTextContext,
) -> gpui::Div {
    let locale = ctx.locale;
    let primary = ctx.theme.tokens.text_theme_primary;
    match msg.code {
        MessageCode::CreatePin => {
            row = row.child(system_text(
                format!(
                    "{} ",
                    mezon_i18n::t(locale, "message.systemMessages.pinned")
                ),
                cursor,
                selection_context,
            ));
            if let Some(reference) = msg.references.first() {
                let jump_id = reference.message_ref_id;
                row = row.child(system_link(
                    primary,
                    mezon_i18n::t(locale, "message.systemMessages.aMessage"),
                    cursor,
                    selection_context,
                    move |_, _, cx| jump_to_message(jump_id, cx),
                ));
            } else {
                row = row.child(system_text(
                    mezon_i18n::t(locale, "message.systemMessages.aMessage"),
                    cursor,
                    selection_context,
                ));
            }
            row = row.child(system_text(
                format!(
                    " {} ",
                    mezon_i18n::t(locale, "message.systemMessages.toThisChannel")
                ),
                cursor,
                selection_context,
            ));
            row = row.child(system_link(
                primary,
                mezon_i18n::t(locale, "message.systemMessages.allPinned"),
                cursor,
                selection_context,
                open_pinned_popover,
            ));
            row.child(system_text(
                format!(
                    " {}",
                    mezon_i18n::t(locale, "message.systemMessages.messages")
                ),
                cursor,
                selection_context,
            ))
        }
        MessageCode::CreateThread => {
            let (thread_label, thread_id, thread_content) = parse_thread_info(&msg.content);
            if !thread_id.is_empty() {
                row = row.child(system_text(
                    format!(
                        " {} ",
                        mezon_i18n::t(locale, "message.systemMessages.startedAThread")
                    ),
                    cursor,
                    selection_context,
                ));
                let parsed = thread_id.parse::<i64>().ok().map(ChannelId);
                row = if let Some(channel_id) = parsed {
                    let thread_link_label = thread_label.clone();
                    row.child(system_link(
                        primary,
                        thread_label,
                        cursor,
                        selection_context,
                        move |_, _, cx| {
                            navigate_to_channel(channel_id, thread_link_label.clone(), cx)
                        },
                    ))
                } else {
                    row.child(system_text(thread_label, cursor, selection_context))
                };
                row.child(system_text(
                    format!(
                        ". {} ",
                        mezon_i18n::t(locale, "message.systemMessages.seeAllThreads")
                    ),
                    cursor,
                    selection_context,
                ))
                .child(system_link(
                    primary,
                    mezon_i18n::t(locale, "message.systemMessages.allThreads"),
                    cursor,
                    selection_context,
                    open_threads_popover,
                ))
                .child(system_text(".", cursor, selection_context))
            } else if !thread_content.is_empty() {
                row.child(system_text(thread_content, cursor, selection_context))
            } else {
                row
            }
        }
        MessageCode::DeleteThread => {
            let (thread_label, _thread_id, thread_content) = parse_thread_info(&msg.content);
            if !thread_label.is_empty() {
                row.child(system_text(
                    format!(
                        "{} ",
                        mezon_i18n::t(locale, "message.systemMessages.deletedAThread")
                    ),
                    cursor,
                    selection_context,
                ))
                .child(
                    div()
                        .max_w_full()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(primary)
                        .child(system_text(thread_label, cursor, selection_context)),
                )
                .child(system_text(".", cursor, selection_context))
            } else if !thread_content.is_empty() {
                row.child(system_text(thread_content, cursor, selection_context))
            } else {
                row
            }
        }
        _ => row,
    }
}

fn system_link(
    color: gpui::Rgba,
    label: impl Into<SharedString>,
    cursor: &mut SelectableSectionCursor,
    selection_context: &SelectableTextContext,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut App) + 'static,
) -> AnyElement {
    let label = label.into();
    let id = SharedString::from(format!("sys-link-{label}"));
    let child = system_text(label.clone(), cursor, selection_context);
    let selection = selection_context.selection();
    div()
        .id(id)
        .max_w_full()
        .min_w_0()
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .text_color(color)
        .on_click(move |event, window, cx| {
            if !selection.borrow().has_selection() {
                on_click(event, window, cx);
            }
        })
        .child(child)
        .into_any_element()
}

fn system_text(
    text: impl Into<SharedString>,
    cursor: &mut SelectableSectionCursor,
    selection_context: &SelectableTextContext,
) -> AnyElement {
    let text = text.into();
    let styled = cursor
        .inline(&text)
        .map(|range| selection_context.text_node(&text, range))
        .unwrap_or_else(|| gpui::StyledText::new(text));
    div()
        .max_w_full()
        .min_w_0()
        .child(styled)
        .into_any_element()
}

pub(crate) fn selectable_system_message_text(msg: &Message, locale: &str) -> String {
    let mut text = msg
        .spans
        .iter()
        .filter_map(|span| match span {
            mezon_store::MessageSpan::Mention { display, .. } => Some(display.as_ref()),
            _ => None,
        })
        .collect::<String>();
    match msg.code {
        MessageCode::CreatePin => text.push_str(&format!(
            "{} {} {} {} {}",
            mezon_i18n::t(locale, "message.systemMessages.pinned"),
            mezon_i18n::t(locale, "message.systemMessages.aMessage"),
            mezon_i18n::t(locale, "message.systemMessages.toThisChannel"),
            mezon_i18n::t(locale, "message.systemMessages.allPinned"),
            mezon_i18n::t(locale, "message.systemMessages.messages")
        )),
        MessageCode::CreateThread => {
            let (thread_label, thread_id, thread_content) = parse_thread_info(&msg.content);
            if !thread_id.is_empty() {
                text.push_str(&format!(
                    " {} {}. {} {}.",
                    mezon_i18n::t(locale, "message.systemMessages.startedAThread"),
                    thread_label,
                    mezon_i18n::t(locale, "message.systemMessages.seeAllThreads"),
                    mezon_i18n::t(locale, "message.systemMessages.allThreads")
                ));
            } else {
                text.push_str(&thread_content);
            }
        }
        MessageCode::DeleteThread => {
            let (thread_label, _, thread_content) = parse_thread_info(&msg.content);
            if !thread_label.is_empty() {
                text.push_str(&format!(
                    "{} {}.",
                    mezon_i18n::t(locale, "message.systemMessages.deletedAThread"),
                    thread_label
                ));
            } else {
                text.push_str(&thread_content);
            }
        }
        _ => {}
    }
    text
}

fn jump_to_message(message_id: MessageId, cx: &mut App) {
    MessagesStore::global(cx).update(cx, |store, cx| {
        store.jump_to_message(message_id, cx);
    });
}

fn open_threads_popover(_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App) {
    ThreadsStore::global(cx).update(cx, |store, cx| {
        store.request_open_popover(cx);
    });
}

fn open_pinned_popover(_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App) {
    PinnedMessagesStore::global(cx).update(cx, |store, cx| {
        store.request_open_popover(cx);
    });
}

fn navigate_to_channel(channel_id: ChannelId, label: String, cx: &mut App) {
    if let Some(clan_id) = ChannelList::global(cx)
        .read(cx)
        .find_channel_in_active_clan(channel_id)
        .map(|channel| channel.clan_id)
    {
        navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
        return;
    }
    if let Some(clan_id) = ChannelList::global(cx).update(cx, |channels, cx| {
        channels.ensure_thread_channel(channel_id, label, cx)
    }) {
        navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }
}

fn parse_thread_info(message_content: &str) -> (String, String, String) {
    if let Some(open) = message_content.find('(')
        && let Some(close_rel) = message_content[open + 1..].find(')')
    {
        let inside = &message_content[open + 1..open + 1 + close_rel];
        if let Some((label, id)) = inside.split_once(',') {
            return (
                label.trim().to_string(),
                id.trim().to_string(),
                String::new(),
            );
        }
    }
    let thread_content = message_content
        .strip_prefix('@')
        .and_then(|rest| rest.split_once(' ').map(|(_, tail)| tail.to_string()))
        .unwrap_or_else(|| message_content.to_string());
    (String::new(), String::new(), thread_content)
}

pub fn render_onboard_welcome(ctx: &RowCtx) -> AnyElement {
    let Some(onboarding) = ctx.onboarding.as_ref() else {
        return div().into_any_element();
    };
    let theme = ctx.theme;
    let locale = ctx.locale;
    let title_color = theme.tokens.text_theme_primary;
    let card_bg = theme.bg_tertiary;
    let hover_bg = theme.tokens.bg_item_hover;

    div()
        .id("msg-onboard")
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .p_4()
        .w_full()
        .child(onboard_item(
            card_bg,
            hover_bg,
            title_color,
            Icon::new(IconName::AddPerson)
                .size_5()
                .text_color(title_color),
            mezon_i18n::t(locale, "chatWelcome.onboard.inviteFriends"),
            onboarding.members_invited,
        ))
        .child(onboard_item(
            card_bg,
            hover_bg,
            title_color,
            Icon::new(IconName::Sent).size_5().text_color(title_color),
            mezon_i18n::t(locale, "chatWelcome.onboard.sendFirstMessage"),
            onboarding.sent_message,
        ))
        .child(onboard_item(
            card_bg,
            hover_bg,
            title_color,
            Icon::new(IconName::Download)
                .size_5()
                .text_color(title_color),
            mezon_i18n::t(locale, "chatWelcome.onboard.downloadApp"),
            onboarding.downloaded_app,
        ))
        .child(onboard_item(
            card_bg,
            hover_bg,
            title_color,
            Icon::new(IconName::Hashtag)
                .size_5()
                .text_color(title_color),
            mezon_i18n::t(locale, "chatWelcome.onboard.createChannel"),
            onboarding.created_channel,
        ))
        .into_any_element()
}

fn onboard_item(
    card_bg: gpui::Rgba,
    hover_bg: gpui::Rgba,
    title_color: gpui::Rgba,
    icon: impl IntoElement,
    title: impl Into<SharedString>,
    done: bool,
) -> AnyElement {
    let title = title.into();
    div()
        .id(SharedString::from(format!("onboard-{title}")))
        .w(px(400.))
        .h(px(72.))
        .flex()
        .flex_row()
        .items_center()
        .gap_4()
        .p_4()
        .rounded_lg()
        .bg(card_bg)
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(title_color)
        .when(!done, |d| d.cursor_pointer().hover(move |s| s.bg(hover_bg)))
        .child(icon)
        .child(div().flex_1().child(title))
        .child(if done {
            div()
                .size(px(24.))
                .flex_none()
                .rounded_full()
                .bg(gpui::rgb(0x16_a3_4a))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Tick)
                        .size(px(14.))
                        .text_color(gpui::rgb(0xff_ff_ff)),
                )
                .into_any_element()
        } else {
            Icon::new(IconName::ArrowRight)
                .size_5()
                .text_color(title_color)
                .into_any_element()
        })
        .into_any_element()
}

pub fn build_onboarding_context(is_dm: bool, cx: &gpui::App) -> Option<OnboardingContext> {
    if is_dm {
        return None;
    }
    let channels = ChannelList::global(cx).read(cx);
    let channel = channels.active_channel()?;
    if matches!(
        channel.channel_type,
        ChannelType::Thread | ChannelType::App | ChannelType::Voice
    ) {
        return None;
    }
    let clan_id = channel.clan_id;
    if clan_id.is_zero() {
        return None;
    }

    let member_count = ClanMembersStore::global(cx).read(cx).count(clan_id);
    let channel_count = channels
        .categories_for_clan(clan_id)
        .iter()
        .flat_map(|category| &category.channels)
        .filter(|ch| !matches!(ch.channel_type, ChannelType::Thread | ChannelType::Voice))
        .count();

    let store = MessagesStore::global(cx).read(cx);
    let sent_message = store
        .channel_tail_message_id()
        .and_then(|tail_id| {
            store
                .messages()
                .iter()
                .find(|message| message.id == tail_id)
                .map(|message| message.code != MessageCode::Indicator)
        })
        .unwrap_or_else(|| {
            store
                .messages()
                .iter()
                .any(|message| message.code != MessageCode::Indicator)
        });

    Some(OnboardingContext {
        members_invited: member_count > 1,
        sent_message,
        downloaded_app: true,
        created_channel: channel_count > 1,
    })
}

pub fn render_welcome(_msg: &Message, ctx: &RowCtx) -> AnyElement {
    let Some(welcome) = ctx.welcome.clone() else {
        return render_welcome_fallback(ctx);
    };
    let theme = ctx.theme;
    let title_color = theme.tokens.text_secondary;
    let subtext_color = theme.tokens.text_theme_primary;
    let icon_fill = theme.tokens.bg_icon_theme;

    let mut col = div()
        .id("msg-welcome")
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .pb_2()
        .w_full();

    match welcome {
        WelcomeContext::Channel {
            name,
            private,
            is_stream,
        } => {
            col = col
                .child(welcome_icon_circle(
                    if is_stream {
                        Icon::new(IconName::Stream).size_10().text_color(icon_fill)
                    } else if private {
                        Icon::new(IconName::HashtagLocked)
                            .size_10()
                            .text_color(icon_fill)
                    } else {
                        Icon::new(IconName::Hashtag).size_10().text_color(icon_fill)
                    },
                    if is_stream {
                        None
                    } else {
                        Some(theme.tokens.bg_primary)
                    },
                ))
                .child(
                    div()
                        .text_size(px(28.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(title_color)
                        .child(fmt_i18n(
                            mezon_i18n::t(ctx.locale, "chatWelcome.welcome.welcomeToChannel"),
                            &[("channelName", name.as_ref())],
                        )),
                )
                .child(welcome_subtext(
                    subtext_color,
                    fmt_i18n(
                        mezon_i18n::t(ctx.locale, "chatWelcome.welcome.startOfChannel"),
                        &[
                            ("channelName", name.as_ref()),
                            (
                                "channelType",
                                if private {
                                    mezon_i18n::t(ctx.locale, "chatWelcome.welcome.private")
                                } else {
                                    ""
                                },
                            ),
                        ],
                    ),
                ));
        }
        WelcomeContext::Thread {
            name,
            private,
            username,
        } => {
            col = col
                .child(welcome_icon_circle(
                    if private {
                        Icon::new(IconName::ThreadIconLocker)
                            .size_10()
                            .text_color(icon_fill)
                    } else {
                        Icon::new(IconName::ThreadIcon)
                            .size_10()
                            .text_color(icon_fill)
                    },
                    Some(theme.tokens.bg_active_member_channel),
                ))
                .child(
                    div()
                        .text_size(px(28.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(title_color)
                        .child(name),
                )
                .child(welcome_subtext(
                    subtext_color,
                    fmt_i18n(
                        mezon_i18n::t(ctx.locale, "chatWelcome.welcome.startOfThread"),
                        &[("username", &username)],
                    ),
                ));
        }
        WelcomeContext::Direct {
            display_name,
            username,
            avatar,
        } => {
            col = col
                .child(welcome_avatar(&display_name, &avatar, ctx))
                .child(
                    div()
                        .text_size(px(28.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(title_color)
                        .child(display_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(username),
                )
                .child(welcome_subtext(
                    subtext_color,
                    fmt_i18n(
                        mezon_i18n::t(ctx.locale, "chatWelcome.welcome.beginningOfDM"),
                        &[("userName", display_name.as_ref())],
                    ),
                ));
        }
        WelcomeContext::Group { name, avatar } => {
            col = col
                .child(welcome_avatar(&name, &avatar, ctx))
                .child(
                    div()
                        .text_size(px(28.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(title_color)
                        .child(name.clone()),
                )
                .child(welcome_subtext(
                    subtext_color,
                    fmt_i18n(
                        mezon_i18n::t(ctx.locale, "chatWelcome.welcome.welcomeToGroup"),
                        &[("groupName", name.as_ref())],
                    ),
                ));
        }
    }

    col.into_any_element()
}

fn render_welcome_fallback(ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    div()
        .id("msg-welcome")
        .flex()
        .flex_col()
        .gap_1()
        .px_4()
        .pt_4()
        .pb_2()
        .w_full()
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.tokens.text_secondary)
                .child(mezon_i18n::t(ctx.locale, "chat.welcomeTitle")),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_muted)
                .child(mezon_i18n::t(ctx.locale, "chat.welcomeSubtitle")),
        )
        .into_any_element()
}

fn welcome_icon_circle(icon: impl IntoElement, bg: Option<gpui::Rgba>) -> AnyElement {
    div()
        .w(px(75.))
        .h(px(75.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .when_some(bg, |d, bg| d.bg(bg))
        .child(icon)
        .into_any_element()
}

fn welcome_avatar(name: &str, avatar: &str, ctx: &RowCtx) -> AnyElement {
    let mut av = Avatar::new()
        .name(name)
        .size_px(px(75.))
        .image_cache(ctx.avatar_cache.clone());
    if !avatar.is_empty() {
        av = av.src(avatar.to_string());
    }
    div().w(px(75.)).h(px(75.)).child(av).into_any_element()
}

fn welcome_subtext(color: gpui::Rgba, text: String) -> AnyElement {
    div()
        .text_sm()
        .opacity(0.6)
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn fmt_i18n(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

pub fn render_unread_break(locale: &str) -> AnyElement {
    let red = gpui::rgb(0xef4444);
    div()
        .id("unread-break")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .py_0p5()
        .w_full()
        .child(div().flex_1().h(px(1.)).bg(red))
        .child(
            div()
                .flex_none()
                .px(px(4.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(red)
                .text_size(px(8.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::rgb(0xffffff))
                .child(mezon_i18n::t(locale, "chat.newMessages")),
        )
        .into_any_element()
}

pub fn build_welcome_context(
    is_dm: bool,
    channel_id: Option<mezon_store::ChannelId>,
    cx: &gpui::App,
) -> Option<WelcomeContext> {
    if is_dm {
        let channel_id = channel_id?;
        let dm = mezon_store::DirectMessageStore::global(cx).read(cx);
        let ch = dm.find(channel_id)?;
        let name = SharedString::from(ch.label.clone());
        let avatar = SharedString::from(ch.avatar.clone());
        return Some(match ch.kind {
            mezon_store::DirectKind::Group => WelcomeContext::Group { name, avatar },
            mezon_store::DirectKind::Dm => WelcomeContext::Direct {
                display_name: name.clone(),
                username: name,
                avatar,
            },
        });
    }

    let channel_id = channel_id?;
    let channels = mezon_store::ChannelList::global(cx).read(cx);
    let channel = channels
        .active_channel()
        .filter(|ch| ch.id == channel_id)
        .or_else(|| channels.find_channel_in_active_clan(channel_id))?;

    let name = SharedString::from(channel.name.clone());
    if channel.channel_type == ChannelType::Thread {
        let creator_id = channel.creator_id;
        let clan_id = channel.clan_id;
        let username = mezon_store::ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, creator_id)
            .map(|m| SharedString::from(m.name().to_string()))
            .unwrap_or_default();
        return Some(WelcomeContext::Thread {
            name,
            private: channel.private,
            username,
        });
    }
    Some(WelcomeContext::Channel {
        name,
        private: channel.private,
        is_stream: channel.channel_type == ChannelType::Stream,
    })
}
