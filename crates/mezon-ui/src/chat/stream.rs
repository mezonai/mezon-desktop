use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, MouseDownEvent, Rgba, SharedString, Subscription,
    Window, div, linear_color_stop, linear_gradient, point, prelude::*, px,
};
#[cfg(target_os = "macos")]
use mezon_store::{AudioStore, DeviceKind, Settings};
use mezon_store::{
    AuthState, Channel, ChannelId, ChannelList, ClanId, ClanList, STREAM_QUALITIES, StreamMember,
    StreamPhase, StreamQuality, StreamStore,
};

use crate::chat::layout::ChatLayout;
use crate::chat::stream_video::StreamVideoView;
#[cfg(target_os = "macos")]
use crate::chat::voice::{
    audio_device_entries, device_option_row, device_radio, selected_device_id,
};
use crate::components::primitives::{Avatar, Icon, IconName, Slider, SliderState};
use crate::theme::{ActiveTheme, Theme};
use crate::util::theme::theme_is_light;
use ui::{Clickable, PopoverMenu, PopoverMenuHandle, Toggleable, Tooltip};

const MEMBER_AVATAR_SIZE: f32 = 40.;
const PUBLISHER_STAGE_AVATAR_SIZE: f32 = 28.;
const PUBLISHER_PREJOIN_AVATAR_SIZE: f32 = 80.;

fn stream_channel_bg(theme: &Theme, joined: bool) -> Rgba {
    if joined {
        gpui::rgb(0x11111b)
    } else if !theme_is_light(theme) {
        theme.tokens.theme_setting_primary
    } else {
        gpui::rgb(0xd1d5db)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_stream_channel(
    window: &mut Window,
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    stream: &Entity<StreamStore>,
    auth: &Entity<AuthState>,
    _chat: &Entity<ChatLayout>,
    volume_slider: &Entity<SliderState>,
    video_view: &Entity<StreamVideoView>,
    quality_menu: &PopoverMenuHandle<StreamQualityMenu>,
    output_menu: &PopoverMenuHandle<StreamOutputMenu>,
    window_width: f32,
    cx: &App,
) -> AnyElement {
    let store = stream.read(cx);
    let members = store.members_for_channel(channel.id);
    let publisher = store.stream_publisher(channel.id);
    let viewer_count = store.viewer_count(channel.id);
    let show_chat = store.show_chat();
    let stream_entity = stream.clone();
    let auth_entity = auth.clone();
    let channel_id = channel.id;
    let clan_id = channel.clan_id;
    let channel_label = channel.name.clone();
    let clan_name = channel.clan_name.clone();
    let error_message = store.error_message().map(str::to_owned);
    let session_here = store.is_session_channel(channel_id);
    let joined = session_here && store.is_joined();
    let joining = session_here && store.is_joining();
    let shell_bg = stream_channel_bg(theme, joined);

    let body = if let Some(message) = error_message {
        render_error(
            theme,
            locale,
            &message,
            &channel_label,
            publisher,
            &members,
            viewer_count,
            window_width,
            joining,
            stream_entity.clone(),
            auth_entity.clone(),
            clan_id,
            channel_id,
            channel_label.clone(),
            clan_name.clone(),
            cx,
        )
    } else if joined && store.fullscreen() {
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(gpui::rgb(0x11111b))
            .into_any_element()
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
            video_view,
            quality_menu,
            output_menu,
            cx,
        )
    } else {
        render_idle(
            theme,
            locale,
            &channel_label,
            publisher,
            &members,
            viewer_count,
            window_width,
            joining,
            stream_entity,
            auth_entity,
            clan_id,
            channel_id,
            channel_label.clone(),
            clan_name.clone(),
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
            joined || joining,
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
    cx: &mut App,
) {
    if clan_name.is_empty()
        && let Some(list) = ClanList::try_global(cx)
        && let Some(clan) = list.read(cx).clan(clan_id)
    {
        clan_name = clan.name.clone();
    }
    let auth = auth.read(cx).clone();
    stream.update(cx, |store, cx| {
        store.join_stream(clan_id, channel_id, &channel_name, &clan_name, &auth, cx);
    });
}

#[allow(clippy::too_many_arguments)]
pub fn render_stream_fullscreen_overlay(
    window: &mut Window,
    theme: &Theme,
    locale: &str,
    store: &StreamStore,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    video_view: &Entity<StreamVideoView>,
    quality_menu: &PopoverMenuHandle<StreamQualityMenu>,
    output_menu: &PopoverMenuHandle<StreamOutputMenu>,
    focus: &FocusHandle,
    cx: &App,
) -> Option<AnyElement> {
    if !store.fullscreen() || (!store.is_joined() && !store.is_joining()) {
        return None;
    }
    let channel = stream_session_channel(cx, store)?;
    let player = render_stream_player(
        window,
        theme,
        locale,
        &channel,
        store,
        stream.clone(),
        volume_slider,
        video_view,
        Some(quality_menu),
        Some(output_menu),
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

    let stream_leave = stream;
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

#[allow(clippy::too_many_arguments)]
fn render_error(
    theme: &Theme,
    locale: &str,
    message: &str,
    channel_label: &str,
    publisher: Option<&StreamMember>,
    members: &[StreamMember],
    viewer_count: u32,
    window_width: f32,
    joining: bool,
    stream: Entity<StreamStore>,
    auth: Entity<AuthState>,
    clan_id: ClanId,
    channel_id: ChannelId,
    join_label_name: String,
    clan_name: String,
    cx: &App,
) -> AnyElement {
    let error_text = if message.is_empty() {
        mezon_i18n::t(locale, "channelStream.videoError").to_string()
    } else {
        message.to_string()
    };
    render_pre_join(
        theme,
        locale,
        channel_label,
        publisher,
        members,
        viewer_count,
        window_width,
        joining,
        Some(error_text),
        stream,
        auth,
        clan_id,
        channel_id,
        join_label_name,
        clan_name,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_idle(
    theme: &Theme,
    locale: &str,
    channel_label: &str,
    publisher: Option<&StreamMember>,
    members: &[StreamMember],
    viewer_count: u32,
    window_width: f32,
    joining: bool,
    stream: Entity<StreamStore>,
    auth: Entity<AuthState>,
    clan_id: ClanId,
    channel_id: ChannelId,
    join_label_name: String,
    clan_name: String,
    cx: &App,
) -> AnyElement {
    render_pre_join(
        theme,
        locale,
        channel_label,
        publisher,
        members,
        viewer_count,
        window_width,
        joining,
        None,
        stream,
        auth,
        clan_id,
        channel_id,
        join_label_name,
        clan_name,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_pre_join(
    theme: &Theme,
    locale: &str,
    channel_label: &str,
    publisher: Option<&StreamMember>,
    members: &[StreamMember],
    viewer_count: u32,
    window_width: f32,
    joining: bool,
    error_text: Option<String>,
    stream: Entity<StreamStore>,
    auth: Entity<AuthState>,
    clan_id: ClanId,
    channel_id: ChannelId,
    join_label_name: String,
    clan_name: String,
    cx: &App,
) -> AnyElement {
    let publisher_display =
        publisher.map(|pub_member| resolve_member_display(cx, clan_id, pub_member));
    let has_publisher = publisher_display.is_some();
    let listed_viewers = members.get(1..).unwrap_or(&[]);
    let has_viewers_row = has_publisher && !listed_viewers.is_empty();
    let status_line = match &publisher_display {
        Some(pub_display) => mezon_i18n::t(locale, "channelStream.isLive")
            .replace("{{name}}", pub_display.name.as_ref()),
        None => mezon_i18n::t(locale, "channelStream.noOneInStream").to_string(),
    };
    let join_text = if joining {
        SharedString::from(mezon_i18n::t(locale, "common.joining"))
    } else {
        SharedString::from(mezon_i18n::t(locale, "channelStream.joinStream"))
    };
    let join_enabled = has_publisher && !joining;
    let max_members = pre_join_max_members(window_width).min(3);
    let stream_join = stream;
    let green = theme.status_online;
    let green_hover = darken(green, 0.12);
    let button_id = if error_text.is_some() {
        "stream-retry-button"
    } else {
        "stream-join-button"
    };
    let circle_bg = if theme_is_light(theme) {
        gpui::rgba(0x9ca3af66)
    } else {
        gpui::rgba(0xffffff1a)
    };
    let top_avatar = match &publisher_display {
        Some(pub_display) => stream_member_avatar_sized(pub_display, PUBLISHER_PREJOIN_AVATAR_SIZE),
        None => div()
            .size(px(PUBLISHER_PREJOIN_AVATAR_SIZE))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(circle_bg)
            .child(
                Icon::new(IconName::Stream)
                    .size(px(40.))
                    .text_color(theme.text_primary),
            )
            .into_any_element(),
    };

    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_4()
        .px_4()
        .child(top_avatar)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_2()
                .max_w(px(420.))
                .child(
                    div()
                        .truncate()
                        .text_3xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(truncate_label(channel_label, 20)),
                )
                .when(has_publisher, |el| el.child(render_live_badge(locale))),
        )
        .child(
            div()
                .text_center()
                .text_color(theme.text_primary)
                .child(status_line),
        )
        .when(has_publisher && viewer_count > 0, |el| {
            el.child(render_viewer_count(
                locale,
                viewer_count,
                theme.text_secondary.into(),
            ))
        })
        .when(has_viewers_row, |el| {
            el.child(member_row(theme, listed_viewers, clan_id, max_members, cx))
        })
        .when(has_viewers_row, |el| {
            el.child(
                div()
                    .text_center()
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(locale, "channelStream.everyoneWaiting")),
            )
        })
        .when_some(error_text, |el, err| {
            el.child(
                div()
                    .max_w(px(420.))
                    .text_center()
                    .text_color(theme.status_dnd)
                    .child(err),
            )
        })
        .child(
            div()
                .id(button_id)
                .w(px(256.))
                .max_w_full()
                .px_4()
                .py_2()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(green)
                .when(join_enabled, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(green_hover))
                        .on_click(move |_, _, cx| {
                            join_stream_action(
                                stream_join.clone(),
                                auth.clone(),
                                clan_id,
                                channel_id,
                                join_label_name.clone(),
                                clan_name.clone(),
                                cx,
                            );
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

#[allow(clippy::too_many_arguments)]
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
    video_view: &Entity<StreamVideoView>,
    quality_menu: &PopoverMenuHandle<StreamQualityMenu>,
    output_menu: &PopoverMenuHandle<StreamOutputMenu>,
    cx: &App,
) -> AnyElement {
    let show_members = store.show_members();
    let has_members = !members.is_empty();

    let stream_toggle_members = stream.clone();
    let player = render_stream_player(
        window,
        theme,
        locale,
        channel,
        store,
        stream,
        volume_slider,
        video_view,
        Some(quality_menu),
        Some(output_menu),
        cx,
        false,
    );
    let tooltip_key = if show_members {
        "channelStream.hideMembers"
    } else {
        "channelStream.showMembers"
    };
    let tooltip = SharedString::from(mezon_i18n::t(locale, tooltip_key));

    div()
        .relative()
        .flex_1()
        .flex()
        .flex_col()
        .min_h_0()
        .w_full()
        .bg(gpui::rgb(0x11111b))
        .p_4()
        .child(div().flex_1().min_h_0().w_full().child(player))
        .when(has_members, |el| {
            el.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .pt_2()
                    .child(render_members_toggle_button(
                        show_members,
                        tooltip,
                        stream_toggle_members,
                    ))
                    .when(show_members, |el| {
                        el.child(member_row(
                            theme,
                            members,
                            channel.clan_id,
                            if show_chat { 4 } else { 7 },
                            cx,
                        ))
                    }),
            )
        })
        .into_any_element()
}

fn render_members_toggle_button(
    show_members: bool,
    tooltip: SharedString,
    stream: Entity<StreamStore>,
) -> AnyElement {
    let stream_toggle = stream;
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

#[allow(clippy::too_many_arguments)]
fn render_stream_player(
    window: &mut Window,
    _theme: &Theme,
    locale: &str,
    channel: &Channel,
    store: &StreamStore,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    video_view: &Entity<StreamVideoView>,
    quality_menu: Option<&PopoverMenuHandle<StreamQualityMenu>>,
    output_menu: Option<&PopoverMenuHandle<StreamOutputMenu>>,
    cx: &App,
    is_fullscreen: bool,
) -> AnyElement {
    let quality_menu_open = quality_menu.is_some_and(PopoverMenuHandle::is_deployed);
    #[cfg(target_os = "macos")]
    let output_menu_open = output_menu.is_some_and(PopoverMenuHandle::is_deployed);
    #[cfg(not(target_os = "macos"))]
    let output_menu_open = false;
    let controls_shown = store.controls_visible() || quality_menu_open || output_menu_open;
    let stream_bump = stream.clone();
    let stream_stage_hover = stream.clone();
    let stream_double_click = stream.clone();
    let overlays = controls_shown.then(|| {
        (
            render_top_overlay(locale, channel, store, is_fullscreen, cx),
            render_controls_bar(
                window,
                locale,
                store,
                stream,
                volume_slider,
                quality_menu,
                output_menu,
                cx,
            ),
        )
    });

    div()
        .id("stream-player")
        .group("stream-player")
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(gpui::rgb(0x000000))
        .when(!is_fullscreen, |el| el.rounded_xl())
        .on_hover(move |hovered, _, cx| {
            stream_stage_hover.update(cx, |store, cx| store.set_stage_hovered(*hovered, cx));
        })
        .on_mouse_move(move |_, _, cx| {
            stream_bump.update(cx, |store, cx| store.bump_controls_visible(cx));
        })
        .child(
            div()
                .id("stream-video-box")
                .size_full()
                .overflow_hidden()
                .on_click(move |event: &ClickEvent, _, cx| {
                    if event.click_count() == 2 {
                        stream_double_click.update(cx, |store, cx| store.toggle_fullscreen(cx));
                    }
                })
                .child(video_view.clone()),
        )
        .when_some(overlays, |el, (top, bar)| el.child(top).child(bar))
        .into_any_element()
}

fn render_top_overlay(
    locale: &str,
    channel: &Channel,
    store: &StreamStore,
    is_fullscreen: bool,
    cx: &App,
) -> AnyElement {
    let publisher = store.stream_publisher(channel.id);
    let clear_traffic_lights = is_fullscreen && !crate::app::window_controls::HAS_CUSTOM_TITLE_BAR;
    let publisher_display =
        publisher.map(|pub_member| resolve_member_display(cx, channel.clan_id, pub_member));
    let viewer_count = store.viewer_count(channel.id);

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .pt_3()
        .when(clear_traffic_lights, |el| {
            el.pt(px(crate::app::window_controls::MACOS_TITLEBAR_INSET + 12.))
        })
        .pb_10()
        .bg(linear_gradient(
            180.,
            linear_color_stop(gpui::rgba(0x00000099), 0.),
            linear_color_stop(gpui::rgba(0x00000000), 1.),
        ))
        .when_some(publisher_display, |el, display| {
            el.child(stream_member_avatar_sized(
                &display,
                PUBLISHER_STAGE_AVATAR_SIZE,
            ))
            .child(
                div()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(gpui::white())
                    .child(display.name),
            )
        })
        .child(render_live_badge(locale))
        .when(viewer_count > 0, |el| {
            el.child(render_viewer_count(
                locale,
                viewer_count,
                gpui::rgba(0xffffffe6).into(),
            ))
        })
        .into_any_element()
}

fn render_live_badge(locale: &str) -> AnyElement {
    div()
        .flex_shrink_0()
        .rounded(px(4.))
        .bg(gpui::rgb(0xda373c))
        .px_1p5()
        .py_0p5()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(gpui::white())
        .child(mezon_i18n::t(locale, "channelStream.live").to_uppercase())
        .into_any_element()
}

fn render_viewer_count(locale: &str, count: u32, color: Hsla) -> AnyElement {
    let text =
        mezon_i18n::t(locale, "channelStream.viewerCount").replace("{{count}}", &count.to_string());
    div()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .text_sm()
        .text_color(color)
        .child(Icon::new(IconName::EyeOpen).size(px(16.)).text_color(color))
        .child(text)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_controls_bar(
    _window: &mut Window,
    locale: &str,
    store: &StreamStore,
    stream: Entity<StreamStore>,
    volume_slider: &Entity<SliderState>,
    quality_menu: Option<&PopoverMenuHandle<StreamQualityMenu>>,
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] output_menu: Option<
        &PopoverMenuHandle<StreamOutputMenu>,
    >,
    _cx: &App,
) -> AnyElement {
    let volume = store.effective_volume();
    let muted = store.muted();
    let fullscreen = store.fullscreen();
    let quality = store.quality();
    let silent = muted || volume <= 0.0;
    let icon = volume_icon(volume, muted);
    let stream_mute = stream.clone();
    let stream_leave = stream.clone();
    let stream_fullscreen = stream.clone();
    let stream_hover = stream.clone();
    let volume_slider_mute = volume_slider.clone();
    let icon_color = gpui::rgba(0xffffffcc);
    let mute_tooltip = SharedString::from(mezon_i18n::t(
        locale,
        if silent {
            "channelStream.unmute"
        } else {
            "channelStream.mute"
        },
    ));
    let fullscreen_tooltip = SharedString::from(mezon_i18n::t(
        locale,
        if fullscreen {
            "channelStream.exitFullscreen"
        } else {
            "channelStream.fullscreen"
        },
    ));
    let quality_picker = render_quality_picker(locale, quality, stream, quality_menu, icon_color);

    div()
        .id("stream-controls-bar")
        .occlude()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_4()
        .pb_4()
        .pt_12()
        .bg(linear_gradient(
            0.,
            linear_color_stop(gpui::rgba(0x000000cc), 0.),
            linear_color_stop(gpui::rgba(0x00000000), 1.),
        ))
        .on_hover(move |hovered, _, cx| {
            stream_hover.update(cx, |store, cx| store.set_controls_hovered(*hovered, cx));
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
                        .p_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(gpui::rgba(0xffffff1a)))
                        .tooltip(Tooltip::text(mute_tooltip))
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
                .id("stream-leave-button")
                .flex()
                .items_center()
                .justify_center()
                .p_2p5()
                .rounded_full()
                .bg(gpui::rgb(0xda373c))
                .cursor_pointer()
                .hover(|s| s.bg(gpui::rgb(0xa12829)))
                .child(
                    Icon::new(IconName::EndCall)
                        .size(px(22.))
                        .text_color(gpui::rgb(0xffffff)),
                )
                .on_click(move |_, _, cx| {
                    stream_leave.update(cx, |store, cx| store.leave_stream(cx));
                }),
        )
        .child({
            let right = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(quality_picker);
            #[cfg(target_os = "macos")]
            let right = right.child(render_output_picker(locale, output_menu, icon_color));
            right.child(
                div()
                    .id("stream-fullscreen-toggle")
                    .p_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(gpui::rgba(0xffffff1a)))
                    .tooltip(Tooltip::text(fullscreen_tooltip))
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
        })
        .into_any_element()
}

fn render_quality_picker(
    locale: &str,
    quality: StreamQuality,
    stream: Entity<StreamStore>,
    quality_menu: Option<&PopoverMenuHandle<StreamQualityMenu>>,
    text_color: Rgba,
) -> AnyElement {
    let current_label = SharedString::from(mezon_i18n::t(locale, quality.i18n_key()));
    let tooltip = SharedString::from(mezon_i18n::t(locale, "channelStream.quality"));
    let locale = locale.to_string();
    let picker = PopoverMenu::new("stream-quality-popover")
        .anchor(Anchor::BottomRight)
        .attach(Anchor::TopRight)
        .offset(point(px(0.), -px(8.)))
        .menu(move |window, cx| {
            let stream = stream.clone();
            let locale = locale.clone();
            Some(cx.new(|cx| StreamQualityMenu::new(stream, locale, window, cx)))
        })
        .trigger(StreamQualityTrigger::new(
            text_color,
            current_label,
            tooltip,
        ));
    picker
        .when_some(quality_menu, |picker, handle| {
            picker.with_handle(handle.clone())
        })
        .into_any_element()
}

#[derive(IntoElement)]
struct StreamQualityTrigger {
    open: bool,
    text_color: Rgba,
    label: SharedString,
    tooltip: SharedString,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl StreamQualityTrigger {
    fn new(text_color: Rgba, label: SharedString, tooltip: SharedString) -> Self {
        Self {
            open: false,
            text_color,
            label,
            tooltip,
            on_click: None,
        }
    }
}

impl Toggleable for StreamQualityTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for StreamQualityTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for StreamQualityTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let trigger = div()
            .id("stream-quality-toggle")
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgba(0xffffff1a)))
            .when(self.open, |el| el.bg(gpui::rgba(0xffffff26)))
            .when(!self.open, |el| el.tooltip(Tooltip::text(self.tooltip)))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.text_color)
                    .child(self.label),
            );
        match self.on_click {
            Some(on_click) => trigger.on_click(on_click),
            None => trigger,
        }
    }
}

pub struct StreamQualityMenu {
    stream: Entity<StreamStore>,
    locale: String,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl StreamQualityMenu {
    fn new(
        stream: Entity<StreamStore>,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent)),
            cx.observe(&stream, |_, _, cx| cx.notify()),
        ];
        Self {
            stream,
            locale,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }
}

impl Focusable for StreamQualityMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for StreamQualityMenu {}

impl Render for StreamQualityMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let current = self.stream.read(cx).quality();
        let text_color = tokens.text_theme_message;
        let check_color = tokens.text_secondary;
        let hover_bg = tokens.bg_item_hover;
        let rows = STREAM_QUALITIES.into_iter().map(|quality| {
            let selected = quality == current;
            let row_id = SharedString::from(format!("stream-quality-{}", quality.i18n_key()));
            let label = SharedString::from(mezon_i18n::t(&self.locale, quality.i18n_key()));
            let indicator = if selected {
                div()
                    .size(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Check)
                            .size(px(14.))
                            .text_color(check_color),
                    )
                    .into_any_element()
            } else {
                div().size(px(16.)).flex_shrink_0().into_any_element()
            };
            div()
                .id(row_id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .when(selected, move |s| s.bg(hover_bg))
                .child(indicator)
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .text_color(text_color)
                        .child(label),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.stream
                        .update(cx, |store, cx| store.set_quality(quality, cx));
                    cx.emit(DismissEvent);
                }))
        });

        div()
            .id("stream-quality-menu")
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.emit(DismissEvent)))
            .occlude()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(200.))
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tokens.text_secondary)
                    .child(mezon_i18n::t(&self.locale, "channelStream.quality")),
            )
            .children(rows)
    }
}

#[cfg(target_os = "macos")]
fn render_output_picker(
    locale: &str,
    output_menu: Option<&PopoverMenuHandle<StreamOutputMenu>>,
    icon_color: Rgba,
) -> AnyElement {
    let label = SharedString::from(mezon_i18n::t(locale, "channelVoice.device.outputDevice"));
    let locale = locale.to_string();
    let picker = PopoverMenu::new("stream-output-popover")
        .anchor(Anchor::BottomRight)
        .attach(Anchor::TopRight)
        .offset(point(px(0.), -px(8.)))
        .menu(move |window, cx| {
            let locale = locale.clone();
            Some(cx.new(|cx| StreamOutputMenu::new(locale, window, cx)))
        })
        .trigger(StreamOutputTrigger::new(icon_color, label));
    picker
        .when_some(output_menu, |picker, handle| {
            picker.with_handle(handle.clone())
        })
        .into_any_element()
}

#[cfg(target_os = "macos")]
#[derive(IntoElement)]
struct StreamOutputTrigger {
    open: bool,
    icon_color: Rgba,
    label: SharedString,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

#[cfg(target_os = "macos")]
impl StreamOutputTrigger {
    fn new(icon_color: Rgba, label: SharedString) -> Self {
        Self {
            open: false,
            icon_color,
            label,
            on_click: None,
        }
    }
}

#[cfg(target_os = "macos")]
impl Toggleable for StreamOutputTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

#[cfg(target_os = "macos")]
impl Clickable for StreamOutputTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

#[cfg(target_os = "macos")]
impl RenderOnce for StreamOutputTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let trigger = div()
            .id("stream-output-toggle")
            .p_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgba(0xffffff1a)))
            .when(self.open, |el| el.bg(gpui::rgba(0xffffff26)))
            .when(!self.open, |el| el.tooltip(Tooltip::text(self.label)))
            .child(
                Icon::new(IconName::Settings)
                    .size(px(18.))
                    .text_color(self.icon_color),
            );
        match self.on_click {
            Some(on_click) => trigger.on_click(on_click),
            None => trigger,
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub struct StreamOutputMenu;

#[cfg(target_os = "macos")]
pub struct StreamOutputMenu {
    locale: String,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

#[cfg(target_os = "macos")]
impl StreamOutputMenu {
    fn new(locale: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let mut subscriptions =
            vec![cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent))];
        if let Some(audio) = AudioStore::try_global(cx) {
            subscriptions.push(cx.observe(&audio, |_, _, cx| cx.notify()));
            AudioStore::refresh_devices(&audio, cx);
        }
        if let Some(settings) = Settings::try_global(cx) {
            subscriptions.push(cx.observe(&settings, |_, _, cx| cx.notify()));
        }
        Self {
            locale,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }
}

#[cfg(target_os = "macos")]
impl Focusable for StreamOutputMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(target_os = "macos")]
impl EventEmitter<DismissEvent> for StreamOutputMenu {}

#[cfg(target_os = "macos")]
impl Render for StreamOutputMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let entries = audio_device_entries(DeviceKind::AudioOutput, &self.locale, cx);
        let configured = Settings::try_global(cx).and_then(|s| s.read(cx).output_device_id.clone());
        let selected_id = selected_device_id(&entries, configured.as_deref());
        let rows = entries.into_iter().map(|(id, name)| {
            let selected = id == selected_id;
            let row_id = SharedString::from(format!(
                "stream-output-{}",
                id.as_deref().unwrap_or("default")
            ));
            device_option_row(
                row_id,
                name,
                device_radio(tokens.text_secondary, selected),
                tokens.text_theme_message,
                tokens.bg_item_hover,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                mezon_store::set_output_device(id.clone(), cx);
                cx.emit(DismissEvent);
            }))
        });

        div()
            .id("stream-output-menu")
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.emit(DismissEvent)))
            .occlude()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(280.))
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tokens.text_secondary)
                    .child(mezon_i18n::t(
                        &self.locale,
                        "channelVoice.device.outputDevice",
                    )),
            )
            .children(rows)
    }
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

fn member_row(
    theme: &Theme,
    members: &[StreamMember],
    clan_id: ClanId,
    max: usize,
    cx: &App,
) -> AnyElement {
    let shown = members.len().min(max);
    let extra = members.len() - shown;
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .children(members[..shown].iter().map(|member| {
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
                    .child(format!("+{extra}")),
            )
        })
        .into_any_element()
}

fn stream_member_avatar(display: &MemberDisplay) -> AnyElement {
    stream_member_avatar_sized(display, MEMBER_AVATAR_SIZE)
}

fn stream_member_avatar_sized(display: &MemberDisplay, size_px: f32) -> AnyElement {
    let size = px(size_px);
    let mut avatar = Avatar::new().size_px(size);
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
