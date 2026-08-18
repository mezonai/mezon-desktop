use gpui::{
    AnyElement, App, Entity, FocusHandle, FontWeight, ObjectFit, Rgba, SharedString, Window, div,
    img, prelude::*, px, relative,
};
use mezon_store::{
    AppConfig, AuthState, Channel, ChannelId, ChannelList, ClanId, ClanList, StreamMember,
    StreamPhase, StreamStore,
};

use crate::chat::layout::ChatLayout;
use crate::components::primitives::{Avatar, Icon, IconName, Slider, SliderState};
use crate::theme::Theme;
use crate::util::assets::STREAM_THUMBNAIL;
use ui::Tooltip;

const MEMBER_AVATAR_SIZE: f32 = 40.;

use crate::util::theme::theme_is_light;

fn stream_channel_bg(theme: &Theme, joined: bool) -> Rgba {
    if joined || !theme_is_light(theme) {
        theme.tokens.theme_setting_primary
    } else {
        gpui::rgb(0xd1d5db)
    }
}

pub fn render_stream_channel(
    window: &mut Window,
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    stream: &Entity<StreamStore>,
    auth: &Entity<AuthState>,
    _chat: &Entity<ChatLayout>,
    volume_slider: &Entity<SliderState>,
    output_device_id: Option<String>,
    window_width: f32,
    cx: &App,
) -> AnyElement {
    let store = stream.read(cx);
    let members = store.members_for_channel(channel.id);
    let show_chat = store.show_chat();
    let stream_entity = stream.clone();
    let auth_entity = auth.clone();
    let channel_id = channel.id;
    let clan_id = channel.clan_id;
    let channel_label = channel.name.clone();
    let clan_name = channel.clan_name.clone();
    let error_message = store.error_message().map(str::to_owned);
    let output_device = output_device_id.clone();
    let session_here = store.is_session_channel(channel_id);
    let joined = session_here && (store.is_joined() || store.is_joining());
    let shell_bg = stream_channel_bg(theme, joined);

    let body = if let Some(message) = error_message {
        render_error(
            theme,
            locale,
            &message,
            &channel_label,
            &members,
            window_width,
            stream_entity.clone(),
            auth_entity.clone(),
            clan_id,
            channel_id,
            channel_label.clone(),
            clan_name.clone(),
            output_device,
            cx,
        )
    } else if joined {
        render_joined(
            window,
            theme,
            locale,
            channel,
            store,
            &members,
            show_chat,
            stream_entity.clone(),
            volume_slider,
            cx,
        )
    } else {
        render_idle(
            theme,
            locale,
            &channel_label,
            &members,
            window_width,
            session_here && store.is_joining(),
            stream_entity,
            auth_entity,
            clan_id,
            channel_id,
            channel_label.clone(),
            clan_name.clone(),
            output_device,
            cx,
        )
    };

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .bg(shell_bg)
        .child(stream_header(
            theme,
            locale,
            &channel.name,
            show_chat,
            joined,
            stream.clone(),
        ))
        .child(body)
        .into_any_element()
}

fn join_stream_action(
    stream: Entity<StreamStore>,
    auth: Entity<AuthState>,
    clan_id: ClanId,
    channel_id: ChannelId,
    channel_name: String,
    mut clan_name: String,
    output_device_id: Option<String>,
    cx: &mut App,
) {
    if clan_name.is_empty()
        && let Some(list) = ClanList::try_global(cx)
        && let Some(clan) = list.read(cx).clan(clan_id)
    {
        clan_name = clan.name.clone();
    }
    let auth = auth.read(cx).clone();
    let config = AppConfig::global(cx).clone();
    stream.update(cx, |store, cx| {
        store.join_stream(
            clan_id,
            channel_id,
            &channel_name,
            &clan_name,
            &auth,
            &config,
            output_device_id,
            cx,
        );
    });
}

pub fn render_stream_fullscreen_overlay(
    window: &mut Window,
    theme: &Theme,
    store: &StreamStore,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    focus: &FocusHandle,
    cx: &App,
) -> Option<AnyElement> {
    if !store.fullscreen() || (!store.is_joined() && !store.is_joining()) {
        return None;
    }
    let channel = stream_session_channel(cx, store)?;
    let is_live = matches!(store.phase(), StreamPhase::Joined { is_live: true, .. });
    let player = render_stream_player(
        window,
        theme,
        &channel,
        store,
        is_live,
        stream.clone(),
        volume_slider,
        cx,
        true,
    );
    let stream_exit = stream;
    Some(
        div()
            .id("stream-fullscreen-overlay")
            .track_focus(focus)
            .key_context("modal_backdrop")
            .on_action({
                let stream_exit = stream_exit.clone();
                move |_: &::menu::Cancel, _window, cx| {
                    stream_exit.update(cx, |store, cx| store.clear_fullscreen(cx));
                }
            })
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x000000))
            .occlude()
            .child(
                div()
                    .group("stream-fullscreen")
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .child(player),
            )
            .into_any_element(),
    )
}

fn stream_session_channel(cx: &App, store: &StreamStore) -> Option<Channel> {
    let channel_id = store.session_channel_id()?;
    let clan_id = match store.phase() {
        StreamPhase::Joining { clan_id, .. } | StreamPhase::Joined { clan_id, .. } => *clan_id,
        _ => return None,
    };
    ChannelList::try_global(cx).and_then(|list| list.read(cx).channel(clan_id, channel_id).cloned())
}

pub fn render_stream_connected_bar(
    theme: &Theme,
    store: &StreamStore,
    stream: Entity<StreamStore>,
    cx: &App,
) -> Option<AnyElement> {
    if !store.is_joined() && !store.is_joining() {
        return None;
    }
    let channel_id = store.session_channel_id()?;
    let clan_id = match store.phase() {
        StreamPhase::Joined { clan_id, .. } | StreamPhase::Joining { clan_id, .. } => *clan_id,
        _ => return None,
    };

    let channel_label = store.session_channel_label();
    let mut clan_name = store.session_clan_name().to_string();
    if clan_name.is_empty()
        && let Some(list) = ClanList::try_global(cx)
        && let Some(clan) = list.read(cx).clan(clan_id)
    {
        clan_name = clan.name.clone();
    }
    let address = if clan_name.is_empty() {
        format!("{channel_label} /")
    } else {
        format!("{channel_label} / {clan_name}")
    };
    let address = if address.chars().count() > 30 {
        format!("{}...", address.chars().take(30).collect::<String>())
    } else {
        address
    };

    let stream_leave = stream.clone();
    let jump_channel = channel_id;
    let jump_clan = clan_id;
    let hover_color = theme.text_primary;
    let leave_icon_color = if theme_is_light(theme) {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    let leave_hover_bg = if theme_is_light(theme) {
        theme.tokens.bg_secondary_button_hover
    } else {
        theme.bg_hover
    };

    Some(
        div()
            .w_full()
            .px_4()
            .py_2()
            .bg(theme.tokens.bg_active_member_channel)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_w(px(200.))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Icon::new(IconName::NetworkStatus)
                                            .size(px(16.))
                                            .text_color(theme.status_online),
                                    )
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.status_online)
                                            .child("Stream Connected"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("stream-connected-jump")
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_secondary)
                                    .hover(move |s| s.text_color(hover_color))
                                    .child(address)
                                    .on_click(move |_, _, cx| {
                                        crate::router::navigate(
                                            cx,
                                            crate::router::Route::Channel {
                                                clan_id: jump_clan,
                                                channel_id: jump_channel,
                                            },
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("stream-connected-leave")
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .opacity(80.)
                            .hover(move |s| s.bg(leave_hover_bg))
                            .child(
                                Icon::new(IconName::EndCall)
                                    .size(px(20.))
                                    .text_color(leave_icon_color),
                            )
                            .on_click(move |_, _, cx| {
                                stream_leave.update(cx, |store, cx| store.leave_stream(cx));
                            }),
                    ),
            )
            .into_any_element(),
    )
}

fn stream_header(
    theme: &Theme,
    locale: &str,
    name: &str,
    show_chat: bool,
    streaming: bool,
    stream: Entity<StreamStore>,
) -> AnyElement {
    let chat_label = if show_chat {
        mezon_i18n::t(locale, "channelTopbar.tooltips.hideChat")
    } else {
        mezon_i18n::t(locale, "channelTopbar.tooltips.showChat")
    };
    let stream_toggle = stream;
    let stream_icon_color = if streaming {
        theme.text_primary
    } else {
        theme.text_muted
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .px_4()
        .py_2()
        .h(px(50.))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.bg_primary)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Stream)
                        .size(px(20.))
                        .text_color(stream_icon_color),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(truncate_label(name, 20)),
                ),
        )
        .child(
            div()
                .id("stream-chat-toggle")
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_tertiary))
                .when(show_chat, |el| el.bg(theme.bg_tertiary))
                .tooltip(Tooltip::text(chat_label))
                .child(crate::chat::voice::chat_toggle_icon(
                    if show_chat {
                        theme.tokens.bg_icon_theme_active
                    } else {
                        theme.tokens.bg_icon_theme
                    },
                    px(20.),
                ))
                .on_click(move |_, _, cx| {
                    stream_toggle.update(cx, |store, cx| store.toggle_chat(cx));
                }),
        )
        .into_any_element()
}

fn render_error(
    theme: &Theme,
    locale: &str,
    message: &str,
    channel_label: &str,
    members: &[StreamMember],
    window_width: f32,
    stream: Entity<StreamStore>,
    auth: Entity<AuthState>,
    clan_id: ClanId,
    channel_id: ChannelId,
    join_label_name: String,
    clan_name: String,
    output_device_id: Option<String>,
    cx: &App,
) -> AnyElement {
    let retry_label = SharedString::from(mezon_i18n::t(locale, "channelStream.joinStream"));
    let retry_enabled = !members.is_empty();
    let stream_retry = stream.clone();
    let green = theme.status_online;
    let green_hover = darken(green, 0.12);
    let error_text = if message.is_empty() {
        mezon_i18n::t(locale, "channelStream.videoError").to_string()
    } else {
        message.to_string()
    };

    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_4()
        .when(!members.is_empty(), |el| {
            el.child(member_row(
                theme,
                members,
                clan_id,
                pre_join_max_members(window_width).min(3),
                cx,
            ))
        })
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_color(theme.status_dnd)
                .child(error_text),
        )
        .child(
            div()
                .max_w(px(350.))
                .text_center()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(truncate_label(channel_label, 20)),
        )
        .child(
            div()
                .id("stream-retry-button")
                .px_4()
                .py_2()
                .rounded_full()
                .bg(green)
                .when(retry_enabled, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(green_hover))
                        .on_click({
                            let output_device_id = output_device_id.clone();
                            move |_, _, cx| {
                                join_stream_action(
                                    stream_retry.clone(),
                                    auth.clone(),
                                    clan_id,
                                    channel_id,
                                    join_label_name.clone(),
                                    clan_name.clone(),
                                    output_device_id.clone(),
                                    cx,
                                );
                            }
                        })
                })
                .when(!retry_enabled, |el| el.opacity(0.5))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(gpui::rgb(0xffffff))
                        .child(retry_label),
                ),
        )
        .into_any_element()
}

fn render_idle(
    theme: &Theme,
    locale: &str,
    channel_label: &str,
    members: &[StreamMember],
    window_width: f32,
    joining: bool,
    stream: Entity<StreamStore>,
    auth: Entity<AuthState>,
    clan_id: ClanId,
    channel_id: ChannelId,
    join_label_name: String,
    clan_name: String,
    output_device_id: Option<String>,
    cx: &App,
) -> AnyElement {
    let subtitle = if members.is_empty() {
        mezon_i18n::t(locale, "channelStream.noOneInStream")
    } else {
        mezon_i18n::t(locale, "channelStream.everyoneWaiting")
    };
    let join_text = if joining {
        SharedString::from("…")
    } else {
        SharedString::from(mezon_i18n::t(locale, "channelStream.joinStream"))
    };
    let join_enabled = !members.is_empty() && !joining;
    let max_members = pre_join_max_members(window_width);
    let stream_join = stream.clone();
    let green = theme.status_online;
    let green_hover = darken(green, 0.12);

    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_4()
        .when(!members.is_empty(), |el| {
            el.child(member_row(theme, members, clan_id, max_members.min(3), cx))
        })
        .child(
            div()
                .max_w(px(350.))
                .text_center()
                .text_3xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(truncate_label(channel_label, 20)),
        )
        .child(
            div()
                .text_color(theme.text_primary)
                .child(subtitle.to_string()),
        )
        .child(
            div()
                .id("stream-join-button")
                .px_4()
                .py_2()
                .rounded_full()
                .bg(green)
                .when(join_enabled, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(green_hover))
                        .on_click({
                            let output_device_id = output_device_id.clone();
                            move |_, _, cx| {
                                join_stream_action(
                                    stream_join.clone(),
                                    auth.clone(),
                                    clan_id,
                                    channel_id,
                                    join_label_name.clone(),
                                    clan_name.clone(),
                                    output_device_id.clone(),
                                    cx,
                                );
                            }
                        })
                })
                .when(!join_enabled, |el| el.opacity(0.5))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(gpui::rgb(0xffffff))
                        .child(join_text),
                ),
        )
        .into_any_element()
}

fn render_joined(
    window: &mut Window,
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    store: &StreamStore,
    members: &[StreamMember],
    show_chat: bool,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    cx: &App,
) -> AnyElement {
    let is_live = matches!(store.phase(), StreamPhase::Joined { is_live: true, .. });
    let show_members = store.show_members();
    let has_members = !members.is_empty();
    let controls_visible = store.controls_visible();

    let stream_leave = stream.clone();
    let stream_toggle_members = stream.clone();
    let stream_playback = stream.clone();
    let stream_bump = stream.clone();
    let video_width = if show_chat {
        relative(1.0)
    } else {
        relative(0.7)
    };
    let player = render_stream_player(
        window,
        theme,
        channel,
        store,
        is_live,
        stream.clone(),
        volume_slider,
        cx,
        false,
    );
    let tooltip_key = if show_members {
        "channelStream.hideMembers"
    } else {
        "channelStream.showMembers"
    };
    let tooltip = SharedString::from(mezon_i18n::t(locale, tooltip_key));
    let control_opacity = if controls_visible { 100. } else { 0. };

    div()
        .relative()
        .flex_1()
        .flex()
        .flex_col()
        .min_h_0()
        .on_mouse_move(move |_, _, cx| {
            stream_bump.update(cx, |store, cx| store.bump_controls_visible(cx));
        })
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w_full()
                        .when(show_members && has_members, |el| el.mt_6())
                        .child(
                            div()
                                .relative()
                                .w(video_width)
                                .max_w_full()
                                .min_h(px(160.))
                                .aspect_ratio(16. / 9.)
                                .child(player)
                                .when(store.playback_blocked(), |el| {
                                    el.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .id("stream-enable-audio")
                                                    .px_4()
                                                    .py_2()
                                                    .rounded_md()
                                                    .bg(gpui::rgba(0x000000b3))
                                                    .text_color(gpui::rgb(0xffffff))
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(gpui::rgba(0x000000cc)))
                                                    .child("Click to enable audio")
                                                    .on_click(move |_, _, cx| {
                                                        stream_playback.update(cx, |store, cx| {
                                                            store.clear_playback_blocked(cx);
                                                        });
                                                    }),
                                            ),
                                    )
                                })
                                .when(has_members && !show_members, |el| {
                                    el.child(render_members_toggle_overlay(
                                        show_members,
                                        px(80.),
                                        control_opacity,
                                        tooltip.clone(),
                                        stream_toggle_members.clone(),
                                    ))
                                }),
                        ),
                )
                .when(show_members && has_members, |el| {
                    el.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .justify_center()
                                    .w_full()
                                    .opacity(control_opacity)
                                    .child(render_members_toggle_button(
                                        show_members,
                                        tooltip,
                                        stream_toggle_members,
                                    )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .justify_center()
                                    .p_2()
                                    .child(member_row(
                                        theme,
                                        members,
                                        channel.clan_id,
                                        if show_chat { 4 } else { 7 },
                                        cx,
                                    )),
                            )
                            .child(div().h(px(80.))),
                    )
                }),
        )
        .child(
            div()
                .absolute()
                .bottom_4()
                .left_0()
                .right_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(control_opacity)
                .child(
                    div()
                        .id("stream-leave-button")
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_3()
                        .rounded_full()
                        .bg(gpui::rgb(0xdc2626))
                        .cursor_pointer()
                        .hover(|s| s.bg(gpui::rgb(0xef4444)))
                        .child(
                            Icon::new(IconName::EndCall)
                                .size(px(24.))
                                .text_color(gpui::rgb(0xffffff)),
                        )
                        .on_click(move |_, _, cx| {
                            stream_leave.update(cx, |store, cx| store.leave_stream(cx));
                        }),
                ),
        )
        .into_any_element()
}

fn render_members_toggle_overlay(
    show_members: bool,
    bottom: gpui::Pixels,
    opacity: f32,
    tooltip: SharedString,
    stream: Entity<StreamStore>,
) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .when(show_members, |el| el.top(px(-20.)))
        .when(!show_members, |el| el.bottom(bottom))
        .flex()
        .justify_center()
        .occlude()
        .opacity(opacity)
        .child(render_members_toggle_button(show_members, tooltip, stream))
        .into_any_element()
}

fn render_members_toggle_button(
    show_members: bool,
    tooltip: SharedString,
    stream: Entity<StreamStore>,
) -> AnyElement {
    let stream_toggle = stream.clone();
    let mut chevron = Icon::new(IconName::ChevronDownIcon)
        .size(px(24.))
        .text_color(gpui::rgb(0xffffff));
    if !show_members {
        chevron = chevron.with_transformation(gpui::Transformation::rotate(gpui::radians(
            std::f32::consts::PI,
        )));
    }

    div()
        .id("stream-toggle-members")
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py(px(6.))
        .rounded_full()
        .bg(gpui::rgba(0x525252ff))
        .cursor_pointer()
        .hover(|s| s.bg(gpui::rgba(0x666666ff)))
        .tooltip(Tooltip::text(tooltip))
        .child(chevron)
        .child(
            Icon::new(IconName::MemberList)
                .size(px(24.))
                .text_color(gpui::rgb(0xffffff)),
        )
        .on_click(move |_, _, cx| {
            stream_toggle.update(cx, |store, cx| store.toggle_members(cx));
        })
        .into_any_element()
}

fn render_stream_player(
    window: &mut Window,
    _theme: &Theme,
    channel: &Channel,
    store: &StreamStore,
    is_live: bool,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    cx: &App,
    always_show_controls: bool,
) -> AnyElement {
    let has_video_frame = store.has_video_frame();
    let media = if is_live && store.remote_video() && has_video_frame {
        render_stream_video(store, cx)
    } else {
        render_stream_thumbnail(channel, cx)
    };

    div()
        .id("stream-player")
        .group("stream-player")
        .relative()
        .size_full()
        .overflow_hidden()
        .when(!always_show_controls, |el| el.rounded_md())
        .child(div().size_full().overflow_hidden().child(media))
        .child(render_controls_bar(
            window,
            store,
            stream,
            volume_slider,
            cx,
            always_show_controls,
        ))
        .into_any_element()
}

fn render_controls_bar(
    _window: &mut Window,
    store: &StreamStore,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    _cx: &App,
    always_show: bool,
) -> AnyElement {
    let volume = store.effective_volume();
    let muted = store.muted();
    let fullscreen = store.fullscreen();
    let icon = volume_icon(volume, muted);
    let stream_mute = stream.clone();
    let stream_fullscreen = stream.clone();
    let volume_slider_mute = volume_slider.clone();
    let icon_color = gpui::rgba(0xaeaeaeff);

    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .p_2()
        .bg(gpui::rgba(0x00000080))
        .when(!always_show, |el| {
            el.opacity(0.)
                .group_hover("stream-player", |s| s.opacity(100.))
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .id("stream-mute-toggle")
                        .p_1()
                        .cursor_pointer()
                        .hover(move |s| s.opacity(0.8))
                        .child(Icon::new(icon).size(px(18.)).text_color(icon_color))
                        .on_click(move |_, window, cx| {
                            stream_mute.update(cx, |store, cx| {
                                let was_muted = store.muted();
                                store.toggle_mute(cx);
                                volume_slider_mute.update(cx, |slider, cx| {
                                    if was_muted {
                                        slider.set_value(store.volume(), window, cx);
                                    } else {
                                        slider.set_value(0., window, cx);
                                    }
                                });
                            });
                        }),
                )
                .child(
                    div()
                        .w(px(100.))
                        .child(Slider::new(volume_slider).horizontal()),
                ),
        )
        .child(
            div()
                .id("stream-fullscreen-toggle")
                .p_1()
                .cursor_pointer()
                .hover(move |s| s.opacity(0.8))
                .child(
                    Icon::new(if fullscreen {
                        IconName::ExitFullScreen
                    } else {
                        IconName::FullScreen
                    })
                    .size(px(18.))
                    .text_color(icon_color),
                )
                .on_click(move |_, _, cx| {
                    stream_fullscreen.update(cx, |store, cx| store.toggle_fullscreen(cx));
                }),
        )
        .into_any_element()
}

fn volume_icon(volume: f32, muted: bool) -> IconName {
    if muted || volume <= 0.0 {
        IconName::MutedVolume
    } else if volume < 0.5 {
        IconName::LowVolume
    } else {
        IconName::LoudVolume
    }
}

fn render_stream_thumbnail(channel: &Channel, _cx: &App) -> AnyElement {
    let raw = channel.avatar_url.trim();
    if !raw.is_empty() && raw != "0" {
        let raw = SharedString::from(raw.to_string());
        return img(raw)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .with_fallback(stream_thumbnail_fallback)
            .into_any_element();
    }
    stream_thumbnail_fallback()
}

fn stream_thumbnail_fallback() -> AnyElement {
    img(STREAM_THUMBNAIL)
        .size_full()
        .object_fit(ObjectFit::Cover)
        .into_any_element()
}

fn render_stream_video(store: &StreamStore, _cx: &App) -> AnyElement {
    if let Some(image) = store.render_frame() {
        return img(image)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    }
    div().size_full().bg(gpui::rgb(0x111111)).into_any_element()
}

fn member_row(
    theme: &Theme,
    members: &[StreamMember],
    clan_id: ClanId,
    max: usize,
    cx: &App,
) -> AnyElement {
    let shown = members.len().min(max);
    let extra = members.len().saturating_sub(max);
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .children(members.iter().take(shown).map(|member| {
            let display = resolve_member_display(cx, clan_id, member);
            stream_member_avatar(&display)
        }))
        .when(extra > 0, |el| {
            el.child(
                div()
                    .w(px(MEMBER_AVATAR_SIZE))
                    .h(px(MEMBER_AVATAR_SIZE))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(theme.bg_tertiary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(format!("{}+", extra.min(99))),
            )
        })
        .into_any_element()
}

fn stream_member_avatar(display: &MemberDisplay) -> AnyElement {
    let size = px(MEMBER_AVATAR_SIZE);
    let mut avatar = Avatar::new().name(display.name.clone()).size_px(size);
    if !display.avatar_src.is_empty() {
        avatar = avatar.src(display.avatar_src.clone());
        if !display.avatar_raw.is_empty() && display.avatar_raw != display.avatar_src {
            avatar = avatar.fallback_src(display.avatar_raw.clone());
        }
    } else if !display.avatar_raw.is_empty() {
        avatar = avatar.src(display.avatar_raw.clone());
    }
    avatar.into_any_element()
}

fn resolve_member_display(cx: &App, clan_id: ClanId, member: &StreamMember) -> MemberDisplay {
    let resolved = crate::util::voice_member::resolve_stream_display(cx, Some(clan_id), member);
    MemberDisplay {
        name: SharedString::from(resolved.name),
        avatar_src: SharedString::from(resolved.avatar_src),
        avatar_raw: SharedString::from(resolved.avatar_raw),
    }
}

struct MemberDisplay {
    name: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
}

pub(crate) fn truncate_label(label: &str, max: usize) -> SharedString {
    if label.chars().count() > max {
        SharedString::from(format!(
            "{}...",
            label.chars().take(max).collect::<String>()
        ))
    } else {
        SharedString::from(label.to_string())
    }
}

fn pre_join_max_members(window_width: f32) -> usize {
    if window_width < 1000. {
        2
    } else if window_width < 1200. {
        3
    } else if window_width < 1300. {
        4
    } else if window_width < 1400. {
        5
    } else if window_width < 1700. {
        6
    } else {
        7
    }
}

fn darken(color: impl Into<gpui::Hsla>, amount: f32) -> gpui::Hsla {
    let mut hsla = color.into();
    hsla.l = (hsla.l - amount).max(0.);
    hsla
}
