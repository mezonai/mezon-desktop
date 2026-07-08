use gpui::{
    Animation, AnimationExt, AnyElement, App, ClipboardItem, Context, Entity, FontWeight, Hsla,
    MouseButton, MouseDownEvent, ObjectFit, SharedString, StyledImage, Window, div, img,
    prelude::*, px, relative,
};
use mezon_store::{
    AppConfig, Channel, ChannelId, ClanId, ClanMembersStore, DisplayedReaction, NetworkQuality,
    Settings, UserId, VoiceCallStatus, VoiceConnection, VoiceMember, VoiceParticipant, VoiceStore,
};

use crate::ChatLayout;
use crate::components::primitives::{Avatar, ContextMenu, Icon, IconName, context_menu_at};
use crate::theme::Theme;
use ui::Tooltip;

/// Shared brand accent (Discord-style blurple) used across the voice UI and
/// the screen-share modal. Single source of truth — do not duplicate.
pub(crate) const ACCENT_BLUE: u32 = 0x5865f2;

const RAISE_HAND_GOLD: u32 = 0xefbc39;

#[allow(clippy::too_many_arguments)]
pub fn render_voice_channel(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    cx: &Context<ChatLayout>,
) -> AnyElement {
    let store = voice.read(cx);
    let connecting = matches!(
        store.connection(),
        VoiceConnection::Connecting { channel_id, .. } if *channel_id == channel.id.to_string()
    );

    if store.is_connected_to(&channel.id.to_string()) || connecting {
        let chat = cx.entity();
        return render_in_call(
            theme, locale, channel, voice, settings, store, connecting, &chat, cx,
        );
    }

    let error = match store.connection() {
        VoiceConnection::Failed {
            channel_id,
            message,
        } if *channel_id == channel.id.to_string() => Some(message.clone()),
        _ => None,
    };

    render_pre_join(
        theme,
        locale,
        channel,
        voice,
        input_device_id,
        output_device_id,
        error,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_mini_bar(
    theme: &Theme,
    locale: &str,
    channel_label: &str,
    clan_name: &str,
    channel_id: &str,
    clan_id: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    mic_enabled: bool,
    camera_enabled: bool,
    screen_enabled: bool,
    link_copied: bool,
    noise_control: AnyElement,
) -> AnyElement {
    let neutral_bg = theme.bg_secondary;
    let neutral_hover = darken(theme.bg_secondary, 0.1);

    let header_key = if camera_enabled {
        "channelVoice.videoConnected"
    } else {
        "channelVoice.voiceConnected"
    };

    let address = format!("{channel_label} / {clan_name}");
    let address = if address.chars().count() > 30 {
        format!("{}...", address.chars().take(30).collect::<String>())
    } else {
        address
    };

    let subtitle = {
        let channel_id = channel_id.to_string();
        let clan_id = clan_id.to_string();
        let hover_color = theme.text_primary;
        div()
            .id("voice-panel-jump")
            .cursor_pointer()
            .text_xs()
            .text_color(theme.text_secondary)
            .hover(move |s| s.text_color(hover_color))
            .child(address)
            .on_click(move |_, _, cx| {
                let (Ok(cid), Ok(clan)) =
                    (channel_id.parse::<ChannelId>(), clan_id.parse::<ClanId>())
                else {
                    return;
                };
                crate::router::navigate(
                    cx,
                    crate::router::Route::Channel {
                        clan_id: clan,
                        channel_id: cid,
                    },
                );
            })
    };

    let copy_button = {
        let channel_id = channel_id.to_string();
        let clan_id = clan_id.to_string();
        let voice = voice.clone();
        div()
            .id("voice-panel-copy")
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(neutral_hover))
            .tooltip(Tooltip::text(mezon_i18n::t(locale, "contextMenu.copyLink")))
            .child(
                Icon::new(if link_copied {
                    IconName::Check
                } else {
                    IconName::CopyIcon
                })
                .size(px(16.))
                .text_color(if link_copied {
                    theme.status_online
                } else {
                    theme.text_secondary
                }),
            )
            .on_click(move |_, _, cx| {
                let link = AppConfig::try_global(cx)
                    .map(|cfg| cfg.voice_link(&clan_id, &channel_id))
                    .unwrap_or_default();
                if !link.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(link));
                    voice.update(cx, |store, cx| store.mark_link_copied(cx));
                }
            })
    };

    let header = div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            Icon::new(IconName::Speaker)
                                .size(px(16.))
                                .text_color(theme.status_online),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.status_online)
                                .child(mezon_i18n::t(locale, header_key).to_string()),
                        ),
                )
                .child(div().flex().flex_row().child(subtitle)),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(noise_control)
                .child(copy_button),
        );

    let mic_button = {
        let voice = voice.clone();
        panel_control_button(
            "voice-panel-mic",
            if mic_enabled {
                IconName::VoiceMicIcon
            } else {
                IconName::VoiceMicDisabledIcon
            },
            neutral_bg,
            neutral_hover,
            if mic_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            },
        )
        .tooltip(Tooltip::text(mezon_i18n::t(
            locale,
            if mic_enabled {
                "channelVoice.turnOffMicrophone"
            } else {
                "channelVoice.turnOnMicrophone"
            },
        )))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_mic(cx)))
    };

    let camera_button = {
        let voice = voice.clone();
        panel_control_button(
            "voice-panel-camera",
            if camera_enabled {
                IconName::VoiceCameraIcon
            } else {
                IconName::VoiceCameraDisabledIcon
            },
            neutral_bg,
            neutral_hover,
            if camera_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            },
        )
        .tooltip(Tooltip::text(mezon_i18n::t(
            locale,
            if camera_enabled {
                "channelVoice.turnOffCamera"
            } else {
                "channelVoice.turnOnCamera"
            },
        )))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_camera(cx)))
    };

    let screen_button = {
        let voice = voice.clone();
        let settings = settings.clone();
        let (bg, hover, color): (Hsla, Hsla, Hsla) = if screen_enabled {
            (
                gpui::rgb(ACCENT_BLUE).into(),
                darken(gpui::rgb(ACCENT_BLUE), 0.1),
                gpui::rgb(0xffffff).into(),
            )
        } else {
            (neutral_bg.into(), neutral_hover, theme.text_primary.into())
        };
        panel_control_button(
            "voice-panel-screen",
            if screen_enabled {
                IconName::VoiceScreenShareStopIcon
            } else {
                IconName::VoiceScreenShareIcon
            },
            bg,
            hover,
            color,
        )
        .tooltip(Tooltip::text(mezon_i18n::t(
            locale,
            if screen_enabled {
                "channelVoice.stopScreenShare"
            } else {
                "channelVoice.shareYourScreen"
            },
        )))
        .on_click(move |_, window, cx| {
            if screen_enabled {
                voice.update(cx, |store, cx| store.stop_screen_share(cx));
            } else {
                crate::chat::screen_share_modal::open_screen_share_modal(
                    voice.clone(),
                    settings.clone(),
                    window,
                    cx,
                );
            }
        })
    };

    let leave_button = {
        let voice = voice.clone();
        panel_control_button(
            "voice-panel-leave",
            IconName::CancelCall,
            theme.status_dnd,
            darken(theme.status_dnd, 0.12),
            gpui::rgb(0xffffff),
        )
        .tooltip(Tooltip::text(mezon_i18n::t(
            locale,
            "channelVoice.disconnect",
        )))
        .on_click(move |_, window, cx| voice.update(cx, |store, cx| store.leave(window, cx)))
    };

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .child(header)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(mic_button)
                .child(camera_button)
                .child(screen_button)
                .child(leave_button),
        )
        .into_any_element()
}

fn panel_control_button(
    id: &'static str,
    icon: IconName,
    bg: impl Into<Hsla>,
    bg_hover: impl Into<Hsla>,
    icon_color: impl Into<Hsla>,
) -> gpui::Stateful<gpui::Div> {
    let bg = bg.into();
    let bg_hover = bg_hover.into();
    let icon_color = icon_color.into();
    div()
        .id(id)
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .h(px(36.))
        .rounded_md()
        .bg(bg)
        .cursor_pointer()
        .hover(move |s| s.bg(bg_hover))
        .child(Icon::new(icon).size(px(20.)).text_color(icon_color))
}

fn voice_header(
    theme: &Theme,
    name: &str,
    in_call: bool,
    status_badge: Option<(SharedString, Hsla, Hsla)>,
) -> AnyElement {
    let right = in_call.then(|| {
        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(decorative_icon(theme, IconName::Chat))
            .child(decorative_icon(theme, IconName::VoiceGridIcon))
            .child(decorative_icon(theme, IconName::VoiceFocusIcon));
        bar.when_some(status_badge, |this, (label, text_color, bg_color)| {
            this.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded_full()
                    .bg(bg_color)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_color)
                    .child(label),
            )
        })
    });

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
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
                    Icon::new(IconName::Speaker)
                        .size(px(20.0))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(name.to_string()),
                ),
        )
        .children(right)
        .into_any_element()
}

fn decorative_icon(theme: &Theme, icon: IconName) -> AnyElement {
    Icon::new(icon)
        .size(px(18.))
        .text_color(theme.text_muted)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_pre_join(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    error: Option<String>,
    cx: &App,
) -> AnyElement {
    let members = &channel.voice_members;
    let subtitle = if members.is_empty() {
        mezon_i18n::t(locale, "channelVoice.noOneInRoom")
    } else {
        mezon_i18n::t(locale, "channelVoice.everyoneWaiting")
    };

    let avatars = (!members.is_empty()).then(|| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .children(members.iter().take(12).map(|m| {
                let (name, avatar_url) = resolve_voice_member(cx, channel.clan_id, m);
                let mut avatar = Avatar::new().name(name).size_px(px(56.));
                if !avatar_url.is_empty() {
                    avatar = avatar.src(avatar_url);
                }
                avatar
            }))
    });

    // A single shared closure builds a join action so both the primary "Join"
    // button and the error-state "Retry" button trigger the same (re)join.
    let make_join_action = {
        let voice = voice.clone();
        let channel_id = channel.id.to_string();
        let clan_id = channel.clan_id.to_string();
        let channel_label = channel.name.clone();
        let input_device_id = input_device_id.clone();
        let output_device_id = output_device_id.clone();
        move || {
            let voice = voice.clone();
            let channel_id = channel_id.clone();
            let clan_id = clan_id.clone();
            let channel_label = channel_label.clone();
            let input_device_id = input_device_id.clone();
            let output_device_id = output_device_id.clone();
            move |_: &gpui::ClickEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
                voice.update(cx, |store, cx| {
                    store.join(
                        channel_id.clone(),
                        clan_id.clone(),
                        channel_label.clone(),
                        input_device_id.clone(),
                        output_device_id.clone(),
                        window,
                        cx,
                    );
                });
            }
        }
    };

    let join = {
        let green = theme.status_online;
        let green_hover = darken(theme.status_online, 0.12);
        div()
            .id("voice-join-btn")
            .flex()
            .items_center()
            .justify_center()
            .px_5()
            .py(px(10.))
            .rounded_full()
            .bg(green)
            .cursor_pointer()
            .hover(move |s| s.bg(green_hover))
            .text_color(gpui::rgb(0xffffff))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .child(mezon_i18n::t(locale, "channelVoice.joinChannelVoiceBS.joinVoice").to_string())
            .on_click(make_join_action())
    };

    let body = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_4()
        .bg(theme.bg_tertiary)
        .children(avatars)
        .child(
            div()
                .text_color(theme.text_primary)
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .child(channel.name.clone()),
        )
        .child(
            div()
                .text_color(theme.text_secondary)
                .text_sm()
                .child(subtitle.to_string()),
        )
        .when_some(error, |this, message| {
            this.child(div().text_color(theme.status_dnd).text_sm().child(message))
        })
        .child(join);

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(voice_header(theme, &channel.name, false, None))
        .child(body)
        .into_any_element()
}

struct VideoCell {
    id: String,
    identity: String,
    name: String,
    avatar_url: String,
    key: Option<u64>,
    is_screen: bool,
    speaking: bool,
    muted: bool,
    quality: NetworkQuality,
}

impl VideoCell {
    fn camera(p: &VoiceParticipant, name: String, avatar_url: String) -> Self {
        Self {
            id: mezon_store::camera_tile_id(&p.identity),
            identity: p.identity.clone(),
            name,
            avatar_url,
            key: p.camera,
            is_screen: false,
            speaking: p.speaking,
            muted: p.muted,
            quality: p.quality,
        }
    }

    fn screen(p: &VoiceParticipant, name: String, avatar_url: String) -> Self {
        Self {
            id: mezon_store::screen_tile_id(&p.identity),
            identity: p.identity.clone(),
            name,
            avatar_url,
            key: p.screenshare,
            is_screen: true,
            speaking: p.speaking,
            muted: p.muted,
            quality: p.quality,
        }
    }
}

fn resolve_voice_identity(
    cx: &App,
    clan_id: ClanId,
    identity: &str,
    fallback_name: &str,
) -> (String, String) {
    if let Ok(uid) = identity.parse::<UserId>()
        && let Some(store) = ClanMembersStore::try_global(cx)
        && let Some(member) = store.read(cx).member(clan_id, uid)
    {
        let name = if member.name().is_empty() {
            fallback_name.to_string()
        } else {
            member.name().to_string()
        };
        let avatar = member.avatar();
        let avatar_url = if avatar.is_empty() {
            String::new()
        } else {
            crate::util::imgproxy::proxied(cx, avatar, 320, 320, "fit")
        };
        return (name, avatar_url);
    }
    (fallback_name.to_string(), String::new())
}

fn resolve_voice_member(cx: &App, clan_id: ClanId, m: &VoiceMember) -> (String, String) {
    if let Some(store) = ClanMembersStore::try_global(cx)
        && let Some(member) = store.read(cx).member(clan_id, m.user_id)
    {
        let name = if member.name().is_empty() {
            m.display_name.clone()
        } else {
            member.name().to_string()
        };
        let avatar = member.avatar();
        let avatar_url = if avatar.is_empty() {
            m.avatar_url.clone()
        } else {
            crate::util::imgproxy::avatar_url(cx, avatar)
        };
        return (name, avatar_url);
    }
    (m.display_name.clone(), m.avatar_url.clone())
}

fn raised_hands_overlay(cx: &App, clan_id: ClanId, store: &VoiceStore) -> Option<AnyElement> {
    let hands = store.raised_hands();
    if hands.is_empty() {
        return None;
    }
    Some(
        div()
            .absolute()
            .top(px(68.))
            .right(px(8.))
            .w(px(320.))
            .flex()
            .flex_col()
            .gap_1()
            .items_end()
            .children(hands.iter().map(|user_id| {
                let (name, avatar_url) = resolve_voice_identity(cx, clan_id, user_id, "");
                let name = SharedString::from(name);
                let mut avatar = Avatar::new().name(name.clone()).size_px(px(32.));
                if !avatar_url.is_empty() {
                    avatar = avatar.src(avatar_url);
                }
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .p_1()
                    .w(px(160.))
                    .h(px(36.))
                    .rounded_full()
                    .bg(gpui::rgb(0xffffff))
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(gpui::rgb(0x000000))
                            .truncate()
                            .child(name),
                    )
                    .child(
                        Icon::new(IconName::VoiceRaiseHandIcon)
                            .size(px(32.))
                            .text_color(gpui::rgb(RAISE_HAND_GOLD)),
                    )
            }))
            .into_any_element(),
    )
}

fn reaction_opacity(delta: f32) -> f32 {
    if delta < 0.1 {
        delta / 0.1
    } else if delta > 0.75 {
        ((1.0 - delta) / 0.25).max(0.0)
    } else {
        1.0
    }
}

fn reaction_float(r: &DisplayedReaction) -> AnyElement {
    let name = r.display_name.as_str();
    let left = r.left;
    let drift = r.drift;
    let seq = r.seq as usize;

    div()
        .absolute()
        .bottom(relative(0.15))
        .left(relative(left))
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .child(
            div()
                .w(px(56.))
                .h(px(56.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(r.emoji_src.clone())
                        .size(px(40.))
                        .object_fit(ObjectFit::Contain)
                        .with_animation(
                            ("voice-reaction-scale", seq),
                            Animation::new(r.duration),
                            |el, delta| el.size(px(16.0 + delta * 40.0)),
                        ),
                ),
        )
        .when(!name.is_empty(), |this| {
            this.child(
                div()
                    .px_2()
                    .py(px(1.))
                    .rounded_full()
                    .bg(gpui::rgba(0x000000b0))
                    .text_size(px(10.))
                    .text_color(gpui::rgb(0xffffff))
                    .child(SharedString::from(name.to_string())),
            )
        })
        .with_animation(
            ("voice-reaction-float", seq),
            Animation::new(r.duration),
            move |el, delta| {
                el.bottom(relative(0.15 + delta))
                    .left(relative(left + drift * delta))
                    .opacity(reaction_opacity(delta))
            },
        )
        .into_any_element()
}

fn reactions_overlay(store: &VoiceStore) -> Option<AnyElement> {
    let reactions = store.displayed_reactions();
    if reactions.is_empty() {
        return None;
    }
    Some(
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .children(reactions.iter().map(reaction_float))
            .into_any_element(),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_in_call(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    connecting: bool,
    chat: &Entity<ChatLayout>,
    cx: &App,
) -> AnyElement {
    let fullscreen_active = store.fullscreen_screen().is_some();

    let body = (!fullscreen_active).then(|| {
        let participants = store.participants();
        let focused = store.focused_tile();

        let mut cells: Vec<VideoCell> = Vec::new();
        for p in participants {
            if p.screenshare.is_some() {
                let (name, avatar) =
                    resolve_voice_identity(cx, channel.clan_id, &p.identity, &p.name);
                cells.push(VideoCell::screen(p, name, avatar));
            }
        }
        for p in participants {
            let (name, avatar) = resolve_voice_identity(cx, channel.clan_id, &p.identity, &p.name);
            cells.push(VideoCell::camera(p, name, avatar));
        }

        let focused_idx = focused.and_then(|id| cells.iter().position(|c| c.id == id));

        match focused_idx {
            Some(idx) => render_focus_layout(theme, locale, store, voice, &cells, idx),
            None => render_grid(
                theme,
                locale,
                store,
                voice,
                &cells,
                &channel.voice_members,
                connecting,
                channel.clan_id,
                cx,
            ),
        }
    });

    let status_badge = if connecting {
        Some((
            SharedString::from(mezon_i18n::t(locale, "channelVoice.connecting").to_string()),
            theme.text_primary.into(),
            theme.bg_hover.into(),
        ))
    } else {
        match store.call_status() {
            VoiceCallStatus::Reconnecting => Some((
                SharedString::from(mezon_i18n::t(locale, "channelVoice.reconnecting").to_string()),
                theme.status_idle.into(),
                theme.bg_hover.into(),
            )),
            VoiceCallStatus::WeakNetwork => Some((
                SharedString::from(mezon_i18n::t(locale, "channelVoice.weakNetwork").to_string()),
                theme.status_idle.into(),
                theme.bg_hover.into(),
            )),
            VoiceCallStatus::Stable => None,
        }
    };

    let mic_modal = store
        .mic_permission_denied()
        .then(|| mic_permission_modal(theme, locale, voice));

    let participant_menu = store.participant_menu().and_then(|(identity, position)| {
        let participant = store
            .participants()
            .iter()
            .find(|p| p.identity == identity)?;
        let (name, _) = resolve_voice_identity(cx, channel.clan_id, identity, &participant.name);
        let menu = build_participant_menu(
            voice,
            identity.to_string(),
            name,
            participant.is_local,
            participant.muted,
            locale,
        );
        Some(context_menu_at(position, menu).into_any_element())
    });

    let kick_modal = store
        .pending_kick()
        .map(|(_, name)| kick_confirm_modal(theme, locale, voice, name));

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(voice_header(theme, &channel.name, true, status_badge))
        .children(body)
        .when(!fullscreen_active, |this| {
            this.child(control_bar(theme, locale, voice, settings, store, chat))
        })
        .children(reactions_overlay(store))
        .children(raised_hands_overlay(cx, channel.clan_id, store))
        .children(mic_modal)
        .children(participant_menu)
        .children(kick_modal)
        .into_any_element()
}

pub(crate) fn render_screen_fullscreen_overlay(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    chat: &Entity<ChatLayout>,
) -> Option<AnyElement> {
    let key = store.fullscreen_screen()?;
    let exit_voice = voice.clone();
    let bg_voice = voice.clone();

    let media = match store.render_image(key) {
        Some(image) => img(image)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => Icon::new(IconName::VoiceScreenShareIcon)
            .size(px(64.))
            .text_color(theme.text_muted)
            .into_any_element(),
    };

    let exit_btn = div()
        .id("screen-fs-exit")
        .absolute()
        .top_4()
        .right_4()
        .flex()
        .items_center()
        .justify_center()
        .w(px(40.))
        .h(px(40.))
        .rounded_full()
        .bg(gpui::rgba(0x000000a6))
        .cursor_pointer()
        .hover(|s| s.bg(gpui::rgba(0x000000d9)))
        .child(
            Icon::new(IconName::ExitFullScreen)
                .size(px(20.))
                .text_color(gpui::rgb(0xffffff)),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            exit_voice.update(cx, |store, cx| store.clear_fullscreen_screen(cx));
        });

    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x000000))
            .child(
                div()
                    .id("screen-fs-video")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(media)
                    .child(exit_btn)
                    .on_click(move |_, _, cx| {
                        bg_voice.update(cx, |store, cx| store.clear_fullscreen_screen(cx));
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_4()
                    .child(control_bar(theme, locale, voice, settings, store, chat)),
            )
            .into_any_element(),
    )
}

fn open_microphone_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }
}

fn mic_permission_modal(theme: &Theme, locale: &str, voice: &Entity<VoiceStore>) -> AnyElement {
    let title =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.micPermissionTitle").to_string());
    let body =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.micPermissionBody").to_string());
    let open_label =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.openSettings").to_string());
    let later_label = SharedString::from(mezon_i18n::t(locale, "channelVoice.later").to_string());

    let later_hover = darken(theme.bg_tertiary, 0.03);
    let primary_hover = theme.brand_hover;
    let voice_later = voice.clone();

    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x000000b3))
        .occlude()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .w(px(380.))
                .p_6()
                .rounded_xl()
                .bg(theme.bg_floating)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(56.))
                        .h(px(56.))
                        .rounded_full()
                        .bg(theme.bg_hover)
                        .child(
                            Icon::new(IconName::VoiceMicDisabledIcon)
                                .size(px(26.))
                                .text_color(theme.status_dnd),
                        ),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.text_muted)
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .w_full()
                        .child(
                            div()
                                .id("mic-perm-later")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.bg_tertiary)
                                .text_color(theme.text_primary)
                                .hover(move |s| s.bg(later_hover))
                                .on_click(move |_, _, cx| {
                                    voice_later.update(cx, |store, cx| {
                                        store.dismiss_mic_permission_prompt(cx)
                                    })
                                })
                                .child(later_label),
                        )
                        .child(
                            div()
                                .id("mic-perm-open")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.brand)
                                .text_color(gpui::rgb(0xffffff))
                                .hover(move |s| s.bg(primary_hover))
                                .on_click(|_, _, _| open_microphone_settings())
                                .child(open_label),
                        ),
                ),
        )
        .into_any_element()
}

fn kick_confirm_modal(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    name: &str,
) -> AnyElement {
    let title =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.kickModal.title").to_string());
    let body = SharedString::from(
        mezon_i18n::t(locale, "channelVoice.kickModal.content").replace("{{userName}}", name),
    );
    let cancel_label = SharedString::from(mezon_i18n::t(locale, "common.cancel").to_string());
    let kick_label =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.kickModal.kick").to_string());

    let cancel_hover = darken(theme.bg_tertiary, 0.03);
    let kick_hover = darken(theme.status_dnd, 0.12);
    let voice_cancel = voice.clone();
    let voice_confirm = voice.clone();

    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x000000b3))
        .occlude()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .w(px(380.))
                .p_6()
                .rounded_xl()
                .bg(theme.bg_floating)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(56.))
                        .h(px(56.))
                        .rounded_full()
                        .bg(theme.bg_hover)
                        .child(
                            Icon::new(IconName::CloseIcon)
                                .size(px(26.))
                                .text_color(theme.status_dnd),
                        ),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.text_muted)
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .w_full()
                        .child(
                            div()
                                .id("kick-cancel")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.bg_tertiary)
                                .text_color(theme.text_primary)
                                .hover(move |s| s.bg(cancel_hover))
                                .on_click(move |_, _, cx| {
                                    voice_cancel.update(cx, |store, cx| store.cancel_kick(cx))
                                })
                                .child(cancel_label),
                        )
                        .child(
                            div()
                                .id("kick-confirm")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.status_dnd)
                                .text_color(gpui::rgb(0xffffff))
                                .hover(move |s| s.bg(kick_hover))
                                .on_click(move |_, _, cx| {
                                    voice_confirm.update(cx, |store, cx| store.confirm_kick(cx))
                                })
                                .child(kick_label),
                        ),
                ),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_grid(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cells: &[VideoCell],
    room_members: &[VoiceMember],
    connecting: bool,
    clan_id: ClanId,
    cx: &App,
) -> AnyElement {
    if cells.is_empty() {
        if !room_members.is_empty() {
            return div()
                .flex()
                .flex_row()
                .flex_wrap()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .p_3()
                .bg(theme.bg_tertiary)
                .children(room_members.iter().map(|member| {
                    let (name, avatar_url) = resolve_voice_member(cx, clan_id, member);
                    let mut avatar = Avatar::new().name(name.clone()).size_px(px(80.));
                    if !avatar_url.is_empty() {
                        avatar = avatar.src(avatar_url);
                    }
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .w(px(180.))
                        .h(px(180.))
                        .rounded_lg()
                        .bg(theme.bg_secondary)
                        .child(avatar)
                        .child(div().text_sm().text_color(theme.text_primary).child(name))
                }))
                .into_any_element();
        }

        let key = if connecting {
            "channelVoice.connecting"
        } else {
            "channelVoice.noOneInRoom"
        };
        return div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .bg(theme.bg_tertiary)
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(locale, key).to_string()),
            )
            .into_any_element();
    }

    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_3()
        .p_3()
        .bg(theme.bg_tertiary)
        .children(
            cells
                .iter()
                .map(|c| video_tile(theme, locale, store, voice, c, px(360.), px(220.))),
        )
        .into_any_element()
}

fn render_focus_layout(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cells: &[VideoCell],
    focused_idx: usize,
) -> AnyElement {
    let focused = &cells[focused_idx];

    let main = div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .p_3()
        .child(focus_main_tile(theme, locale, store, voice, focused));

    let strip = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .px_3()
        .pb_3()
        .overflow_hidden()
        .children(
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| strip_tile(theme, locale, store, voice, c, i == focused_idx)),
        );

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .bg(theme.bg_tertiary)
        .child(main)
        .child(strip)
        .into_any_element()
}

fn participant_menu_trigger(
    voice: &Entity<VoiceStore>,
    identity: String,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let voice = voice.clone();
    move |event: &MouseDownEvent, _window: &mut Window, cx: &mut App| {
        let position = event.position;
        let identity = identity.clone();
        voice.update(cx, |store, cx| {
            store.open_participant_menu(identity, position, cx);
        });
    }
}

fn build_participant_menu(
    voice: &Entity<VoiceStore>,
    identity: String,
    name: String,
    is_local: bool,
    muted: bool,
    locale: &str,
) -> ContextMenu {
    let dismiss = {
        let voice = voice.clone();
        move |_window: &mut Window, cx: &mut App| {
            voice.update(cx, |store, cx| store.close_participant_menu(cx));
        }
    };

    let mut menu = ContextMenu::new().on_dismiss(dismiss);

    if !is_local {
        if !muted {
            let voice = voice.clone();
            let identity = identity.clone();
            menu = menu.danger_item_icon(
                mezon_i18n::t(locale, "contextMenu.muteMic").to_string(),
                IconName::VoiceMicDisabledIcon,
                move |_window, cx| {
                    let identity = identity.clone();
                    voice.update(cx, |store, cx| store.mute_participant(identity, cx));
                },
            );
        }
        let voice = voice.clone();
        let identity = identity.clone();
        menu = menu
            .danger_item_icon(
                mezon_i18n::t(locale, "contextMenu.member.kick").to_string(),
                IconName::CloseIcon,
                move |_window, cx| {
                    let identity = identity.clone();
                    let name = name.clone();
                    voice.update(cx, |store, cx| store.request_kick(identity, name, cx));
                },
            )
            .separator();
    }

    menu.item_icon(
        mezon_i18n::t(locale, "contextMenu.copyUserId").to_string(),
        IconName::CopyIcon,
        move |_window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(identity.clone()));
        },
    )
}

fn focus_main_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
) -> AnyElement {
    let voice = voice.clone();
    let inner = tile_inner(theme, store, cell, true);

    div()
        .id(SharedString::from(format!("focus-main-{}", cell.id)))
        .relative()
        .flex()
        .flex_1()
        .size_full()
        .min_h_0()
        .items_center()
        .justify_center()
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .child(inner)
        .child(tile_label(theme, locale, cell))
        .child(tile_quality(cell))
        .children(tile_sound_overlay(store, cell))
        .on_mouse_down(
            MouseButton::Right,
            participant_menu_trigger(&voice, cell.identity.clone()),
        )
        .on_click(move |_, _, cx| {
            voice.update(cx, |store, cx| store.clear_focus(cx));
        })
        .into_any_element()
}

fn strip_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
    is_focused: bool,
) -> AnyElement {
    let voice = voice.clone();
    let id = cell.id.clone();
    let inner = tile_inner(theme, store, cell, false);
    let border_color: Hsla = if is_focused {
        gpui::rgb(ACCENT_BLUE).into()
    } else if cell.speaking && !cell.is_screen {
        theme.status_online.into()
    } else {
        gpui::transparent_black()
    };

    div()
        .id(SharedString::from(format!("strip-{}", cell.id)))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(214.))
        .h(px(120.))
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .border_2()
        .border_color(border_color)
        .child(inner)
        .child(tile_label(theme, locale, cell))
        .child(tile_quality(cell))
        .children(tile_sound_overlay(store, cell))
        .on_mouse_down(
            MouseButton::Right,
            participant_menu_trigger(&voice, cell.identity.clone()),
        )
        .on_click(move |_, _, cx| {
            voice.update(cx, |store, cx| store.set_focus(id.clone(), cx));
        })
        .into_any_element()
}

fn video_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
    width: gpui::Pixels,
    height: gpui::Pixels,
) -> AnyElement {
    let voice = voice.clone();
    let id = cell.id.clone();
    let inner = tile_inner(theme, store, cell, false);
    let border_color: Hsla = if cell.speaking && !cell.is_screen {
        theme.status_online.into()
    } else {
        gpui::transparent_black()
    };

    div()
        .id(SharedString::from(format!("tile-{}", cell.id)))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(width)
        .h(height)
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .border_2()
        .border_color(border_color)
        .child(inner)
        .child(tile_label(theme, locale, cell))
        .child(tile_quality(cell))
        .children(tile_sound_overlay(store, cell))
        .on_mouse_down(
            MouseButton::Right,
            participant_menu_trigger(&voice, cell.identity.clone()),
        )
        .on_click(move |_, _, cx| {
            voice.update(cx, |store, cx| store.toggle_focus(id.clone(), cx));
        })
        .into_any_element()
}

fn tile_inner(theme: &Theme, store: &VoiceStore, cell: &VideoCell, large: bool) -> AnyElement {
    if let Some(key) = cell.key
        && let Some(image) = store.render_image(key)
    {
        let fit = if cell.is_screen {
            ObjectFit::Contain
        } else {
            ObjectFit::Cover
        };
        return img(image).size_full().object_fit(fit).into_any_element();
    }

    if cell.is_screen {
        return Icon::new(IconName::VoiceScreenShareIcon)
            .size(px(48.))
            .text_color(theme.text_muted)
            .into_any_element();
    }

    let mut avatar =
        Avatar::new()
            .name(cell.name.clone())
            .size_px(if large { px(120.) } else { px(80.) });
    if !cell.avatar_url.is_empty() {
        avatar = avatar.src(cell.avatar_url.clone());
    }
    avatar.into_any_element()
}

fn tile_label(theme: &Theme, locale: &str, cell: &VideoCell) -> AnyElement {
    let label = if cell.is_screen {
        mezon_i18n::t(locale, "channelVoice.usernameScreen").replace("{{username}}", &cell.name)
    } else {
        cell.name.clone()
    };

    div()
        .absolute()
        .left_2()
        .bottom_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px(px(6.))
        .py(px(2.))
        .rounded_md()
        .bg(gpui::rgba(0x000000b0))
        .when(cell.muted && !cell.is_screen, |this| {
            this.child(
                Icon::new(IconName::VoiceMicDisabledIcon)
                    .size(px(14.))
                    .text_color(theme.status_dnd),
            )
        })
        .when(cell.is_screen, |this| {
            this.child(
                Icon::new(IconName::VoiceScreenShareIcon)
                    .size(px(14.))
                    .text_color(gpui::rgb(0xffffff)),
            )
        })
        .child(div().text_xs().text_color(gpui::rgb(0xffffff)).child(label))
        .into_any_element()
}

fn tile_quality(cell: &VideoCell) -> AnyElement {
    let icon = match cell.quality {
        NetworkQuality::Excellent => IconName::SvgQualityExcellentIcon,
        NetworkQuality::Good => IconName::SvgQualityGoodIcon,
        NetworkQuality::Poor => IconName::SvgQualityPoorIcon,
        NetworkQuality::Unknown => IconName::SvgQualityUnknownIcon,
    };

    div()
        .absolute()
        .right_2()
        .bottom_2()
        .flex()
        .items_center()
        .justify_center()
        .px(px(6.))
        .py(px(2.))
        .rounded_md()
        .bg(gpui::rgba(0x000000b0))
        .child(
            Icon::new(icon)
                .size(px(16.))
                .text_color(gpui::rgb(0xffffff)),
        )
        .into_any_element()
}

fn tile_sound_overlay(store: &VoiceStore, cell: &VideoCell) -> Option<AnyElement> {
    if !store.is_sound_active(&cell.identity) {
        return None;
    }
    Some(
        div()
            .absolute()
            .top_2()
            .right_2()
            .flex()
            .items_center()
            .justify_center()
            .p(px(6.))
            .rounded_full()
            .bg(gpui::rgb(ACCENT_BLUE))
            .border_1()
            .border_color(gpui::rgba(0xffffff33))
            .shadow_lg()
            .child(
                Icon::new(IconName::VoiceSoundControlIcon)
                    .size(px(16.))
                    .text_color(gpui::rgb(0xffffff)),
            )
            .into_any_element(),
    )
}

fn control_bar(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    chat: &Entity<ChatLayout>,
) -> AnyElement {
    let mic_enabled = store.mic_enabled();
    let camera_enabled = store.camera_enabled();
    let screen_enabled = store.screen_share_enabled();

    let neutral_bg = theme.bg_secondary;
    let neutral_hover = darken(theme.bg_secondary, 0.1);

    let mic_tooltip = mezon_i18n::t(
        locale,
        if mic_enabled {
            "channelVoice.turnOffMicrophone"
        } else {
            "channelVoice.turnOnMicrophone"
        },
    );
    let camera_tooltip = mezon_i18n::t(
        locale,
        if camera_enabled {
            "channelVoice.turnOffCamera"
        } else {
            "channelVoice.turnOnCamera"
        },
    );
    let screen_tooltip = mezon_i18n::t(
        locale,
        if screen_enabled {
            "channelVoice.stopScreenShare"
        } else {
            "channelVoice.shareYourScreen"
        },
    );
    let leave_tooltip = mezon_i18n::t(locale, "channelVoice.leave");

    let mic_button = {
        let voice = voice.clone();
        circle_button(
            "voice-mic-btn",
            neutral_bg,
            neutral_hover,
            if mic_enabled {
                IconName::VoiceMicIcon
            } else {
                IconName::VoiceMicDisabledIcon
            },
            if mic_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            },
        )
        .tooltip(Tooltip::text(mic_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_mic(cx)))
    };

    let camera_button = {
        let voice = voice.clone();
        circle_button(
            "voice-camera-btn",
            neutral_bg,
            neutral_hover,
            if camera_enabled {
                IconName::VoiceCameraIcon
            } else {
                IconName::VoiceCameraDisabledIcon
            },
            if camera_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            },
        )
        .tooltip(Tooltip::text(camera_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_camera(cx)))
    };

    let screen_button = {
        let voice = voice.clone();
        let settings = settings.clone();
        let (bg, hover, color): (Hsla, Hsla, Hsla) = if screen_enabled {
            (
                gpui::rgb(ACCENT_BLUE).into(),
                darken(gpui::rgb(ACCENT_BLUE), 0.1),
                gpui::rgb(0xffffff).into(),
            )
        } else {
            (neutral_bg.into(), neutral_hover, theme.text_primary.into())
        };
        circle_button(
            "voice-screen-btn",
            bg,
            hover,
            if screen_enabled {
                IconName::VoiceScreenShareStopIcon
            } else {
                IconName::VoiceScreenShareIcon
            },
            color,
        )
        .tooltip(Tooltip::text(screen_tooltip))
        .on_click(move |_, window, cx| {
            if screen_enabled {
                voice.update(cx, |store, cx| store.stop_screen_share(cx));
            } else {
                crate::chat::screen_share_modal::open_screen_share_modal(
                    voice.clone(),
                    settings.clone(),
                    window,
                    cx,
                );
            }
        })
    };

    let leave_button = {
        let voice = voice.clone();
        circle_button(
            "voice-leave-btn",
            theme.status_dnd,
            darken(theme.status_dnd, 0.12),
            IconName::CancelCall,
            gpui::rgb(0xffffff),
        )
        .tooltip(Tooltip::text(leave_tooltip))
        .on_click(move |_, window, cx| voice.update(cx, |store, cx| store.leave(window, cx)))
    };

    let raise_active = store.is_local_hand_raised();
    let raise_tooltip = mezon_i18n::t(
        locale,
        if raise_active {
            "channelVoice.lowerHand"
        } else {
            "channelVoice.raiseHand"
        },
    );
    let raise_color: Hsla = if raise_active {
        gpui::rgb(RAISE_HAND_GOLD).into()
    } else {
        theme.text_primary.into()
    };
    let raise_hand_button = {
        let voice = voice.clone();
        circle_button(
            "voice-raise-hand-btn",
            neutral_bg,
            neutral_hover,
            IconName::VoiceRaiseHandIcon,
            raise_color,
        )
        .tooltip(Tooltip::text(raise_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.send_raising_hand(cx)))
    };

    let mut right = div()
        .flex()
        .flex_row()
        .flex_1()
        .items_center()
        .justify_end()
        .gap_1();
    if let Some(key) = store.primary_screen_key() {
        let pip_active = store.pip_key() == Some(key);
        let is_fullscreen = store.fullscreen_screen() == Some(key);
        let pip_color: Hsla = if pip_active {
            gpui::rgb(ACCENT_BLUE).into()
        } else {
            theme.text_secondary.into()
        };

        let pip_button = {
            let voice = voice.clone();
            circle_button(
                "voice-pip-btn",
                gpui::transparent_black(),
                theme.bg_secondary.into(),
                IconName::VoicePopOutIcon,
                pip_color,
            )
            .on_click(move |_, _, cx| {
                if pip_active {
                    voice.update(cx, |store, cx| store.close_pip(cx));
                } else if let Some(handle) =
                    crate::chat::screen_share_pip::open_screen_share_pip(voice.clone(), key, cx)
                {
                    voice.update(cx, |store, cx| store.set_pip(key, handle, cx));
                }
            })
        };

        let fs_button = {
            let voice = voice.clone();
            circle_button(
                "voice-fullscreen-btn",
                gpui::transparent_black(),
                theme.bg_secondary.into(),
                if is_fullscreen {
                    IconName::ExitFullScreen
                } else {
                    IconName::FullScreen
                },
                theme.text_secondary,
            )
            .on_click(move |_, _, cx| {
                voice.update(cx, |store, cx| store.toggle_fullscreen_screen(key, cx));
            })
        };

        right = right.child(pip_button).child(fs_button);
    }

    let emoji_button = {
        let chat = chat.clone();
        circle_button(
            "voice-emoji-btn",
            neutral_bg,
            neutral_hover,
            IconName::VoiceEmojiControlIcon,
            theme.text_muted,
        )
        .tooltip(Tooltip::text(mezon_i18n::t(
            locale,
            "channelVoice.reactions",
        )))
        .on_click(move |_, window, cx| {
            chat.update(cx, |layout, cx| {
                layout.toggle_voice_emoji_picker(window, cx)
            });
        })
    };

    let sound_button = {
        let chat = chat.clone();
        circle_button(
            "voice-sound-btn",
            neutral_bg,
            neutral_hover,
            IconName::VoiceSoundControlIcon,
            theme.text_muted,
        )
        .on_click(move |_, window, cx| {
            chat.update(cx, |layout, cx| {
                layout.toggle_voice_sound_picker(window, cx)
            });
        })
    };

    let left = div()
        .flex()
        .flex_row()
        .flex_1()
        .items_center()
        .gap_3()
        .child(emoji_button)
        .child(sound_button);

    let center = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_3()
        .child(mic_button)
        .child(camera_button)
        .child(screen_button)
        .child(decorative_circle(theme, IconName::ShadowBotIcon))
        .child(raise_hand_button)
        .child(leave_button);

    div()
        .flex()
        .flex_row()
        .items_center()
        .px_4()
        .py_3()
        .bg(theme.bg_tertiary)
        .border_t_1()
        .border_color(theme.border)
        .child(left)
        .child(center)
        .child(right)
        .into_any_element()
}

fn decorative_circle(theme: &Theme, icon: IconName) -> AnyElement {
    // Purely decorative: same footprint as the real control buttons but with no
    // id/on_click and an explicit default cursor so it never reads as clickable.
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(44.))
        .h(px(44.))
        .rounded_full()
        .cursor_default()
        .bg(theme.bg_secondary)
        .child(Icon::new(icon).size(px(20.)).text_color(theme.text_muted))
        .into_any_element()
}

fn circle_button(
    id: &'static str,
    bg: impl Into<Hsla>,
    bg_hover: Hsla,
    icon: IconName,
    icon_color: impl Into<Hsla>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(44.))
        .h(px(44.))
        .rounded_full()
        .bg(bg.into())
        .cursor_pointer()
        .hover(move |s| s.bg(bg_hover))
        .child(Icon::new(icon).size(px(20.)).text_color(icon_color.into()))
}

fn darken(color: impl Into<Hsla>, amount: f32) -> Hsla {
    let mut hsla = color.into();
    hsla.l = (hsla.l - amount).max(0.0);
    hsla
}
