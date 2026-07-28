use std::collections::HashMap;

use gpui::{
    Animation, AnimationExt, AnyElement, App, ClipboardItem, Context, Entity, FontWeight, Hsla,
    MouseButton, MouseDownEvent, ObjectFit, Pixels, ScrollHandle, SharedString, StyledImage,
    Window, canvas, deferred, div, img, prelude::*, px, relative,
};
use mezon_store::{
    AppConfig, AudioStore, Channel, ChannelId, ClanId, ClanMembersStore, DeviceKind,
    DeviceMenuKind, DisplayedReaction, NetworkQuality, PERMISSION_MANAGE_CHANNEL, PermissionStore,
    Settings, UserId, VoiceCallStatus, VoiceConnection, VoiceMember, VoiceParticipant,
    VoiceRenderFrame, VoiceStore,
};

use crate::ChatLayout;
use crate::components::primitives::{
    Avatar, ContextMenu, Icon, IconName, Sizable, Size, Spinner, context_menu_at,
};
use crate::theme::{ActiveTheme, Theme};
use ui::{ScrollAxes, Scrollbars, Tooltip, WithScrollbar};

/// Shared brand accent (Discord-style blurple) used across the voice UI and
/// the screen-share modal. Single source of truth — do not duplicate.
pub(crate) const ACCENT_BLUE: u32 = 0x5865f2;

const RAISE_HAND_GOLD: u32 = 0xefbc39;

const LEAVE_RED: u32 = 0xda373c;
const LEAVE_RED_HOVER: u32 = 0xa12829;

const SPEAKING_BLUE: u32 = 0x1f8cf9;
const SPEAKING_BORDER_WIDTH: f32 = 2.5;

fn speaking_border_color(cell: &VideoCell) -> Hsla {
    if cell.speaking && !cell.muted && !cell.is_screen {
        gpui::rgb(SPEAKING_BLUE).into()
    } else {
        gpui::transparent_black()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_voice_channel(
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    camera_device_id: Option<String>,
    strip_scroll: &ScrollHandle,
    grid_page: usize,
    grid_size: gpui::Size<Pixels>,
    show_members: bool,
    visual: &mut VoiceVisualState,
    window_width: Pixels,
    window: &mut Window,
    cx: &mut Context<ChatLayout>,
) -> AnyElement {
    let connecting = {
        let store = voice.read(cx);
        matches!(
            store.connection(),
            VoiceConnection::Connecting { channel_id, .. } if *channel_id == channel.id.to_string()
        )
    };
    let in_call = voice.read(cx).is_connected_to(&channel.id.to_string()) || connecting;

    if in_call {
        let chat = cx.entity();
        return render_in_call(
            locale,
            channel,
            voice,
            settings,
            connecting,
            &chat,
            strip_scroll,
            grid_page,
            grid_size,
            show_members,
            visual,
            window,
            cx,
        );
    }

    let error = match voice.read(cx).connection() {
        VoiceConnection::Failed {
            channel_id,
            message,
        } if *channel_id == channel.id.to_string() => Some(message.clone()),
        _ => None,
    };

    render_pre_join(
        cx.theme(),
        locale,
        channel,
        voice,
        input_device_id,
        output_device_id,
        camera_device_id,
        error,
        window_width,
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
            theme.text_primary,
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
            theme.text_primary,
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
            IconName::EndCall,
            gpui::rgb(LEAVE_RED),
            gpui::rgb(LEAVE_RED_HOVER),
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
        .border_b_2()
        .border_color(theme.tokens.border_primary)
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

fn voice_header(theme: &Theme, name: &str, in_call: bool) -> AnyElement {
    let right = in_call.then(|| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(decorative_icon(theme, IconName::Chat))
            .child(decorative_icon(theme, IconName::VoiceGridIcon))
            .child(decorative_icon(theme, IconName::VoiceFocusIcon))
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
        3
    }
}

#[allow(clippy::too_many_arguments)]
fn render_pre_join(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    camera_device_id: Option<String>,
    error: Option<String>,
    window_width: Pixels,
    cx: &App,
) -> AnyElement {
    let members = &channel.voice_members;
    let subtitle = if members.is_empty() {
        mezon_i18n::t(locale, "channelVoice.noOneInRoom")
    } else {
        mezon_i18n::t(locale, "channelVoice.everyoneWaiting")
    };

    let avatars = (!members.is_empty()).then(|| {
        let max_members = pre_join_max_members(f32::from(window_width));
        let remaining = members.len().saturating_sub(max_members);
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .children(members.iter().take(max_members).map(|m| {
                let (name, avatar_url) = resolve_voice_member(cx, channel.clan_id, m);
                let mut avatar = Avatar::new().name(name).size_px(px(56.));
                if !avatar_url.is_empty() {
                    avatar = avatar.src(avatar_url);
                }
                avatar
            }))
            .when(remaining > 0, |this| {
                this.child(
                    div()
                        .size(px(56.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme.bg_secondary)
                        .text_color(theme.text_primary)
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("+{remaining}")),
                )
            })
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
        let camera_device_id = camera_device_id.clone();
        move || {
            let voice = voice.clone();
            let channel_id = channel_id.clone();
            let clan_id = clan_id.clone();
            let channel_label = channel_label.clone();
            let input_device_id = input_device_id.clone();
            let output_device_id = output_device_id.clone();
            let camera_device_id = camera_device_id.clone();
            move |_: &gpui::ClickEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
                voice.update(cx, |store, cx| {
                    store.join(
                        channel_id.clone(),
                        clan_id.clone(),
                        channel_label.clone(),
                        input_device_id.clone(),
                        output_device_id.clone(),
                        camera_device_id.clone(),
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
        .child(voice_header(theme, &channel.name, false))
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
    is_local: bool,
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
            is_local: p.is_local,
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
            is_local: p.is_local,
            speaking: p.speaking,
            muted: p.muted,
            quality: p.quality,
        }
    }

    fn placeholder(identity: String, name: String, avatar_url: String) -> Self {
        Self {
            id: mezon_store::camera_tile_id(&identity),
            identity,
            name,
            avatar_url,
            key: None,
            is_screen: false,
            is_local: false,
            speaking: false,
            muted: false,
            quality: NetworkQuality::Unknown,
        }
    }
}

#[derive(Default)]
pub struct VoiceVisualState {
    order: Vec<String>,
    max_items: usize,
}

fn cell_category(cell: &VideoCell) -> u8 {
    match (cell.is_local, cell.is_screen) {
        (true, false) => 0,
        (false, true) => 1,
        (true, true) => 2,
        (false, false) => 3,
    }
}

fn target_visual_order(
    cells: &[VideoCell],
    join_rank: &dyn Fn(&str) -> usize,
    last_spoke_rank: &dyn Fn(&str) -> u64,
) -> Vec<String> {
    let mut idx: Vec<usize> = (0..cells.len()).collect();
    idx.sort_by(|&a, &b| {
        let ca = &cells[a];
        let cb = &cells[b];
        cell_category(ca)
            .cmp(&cell_category(cb))
            .then_with(|| match cell_category(ca) {
                1 | 2 => join_rank(&ca.identity).cmp(&join_rank(&cb.identity)),
                3 => cb
                    .speaking
                    .cmp(&ca.speaking)
                    .then_with(|| last_spoke_rank(&cb.identity).cmp(&last_spoke_rank(&ca.identity)))
                    .then_with(|| cb.key.is_some().cmp(&ca.key.is_some()))
                    .then_with(|| join_rank(&ca.identity).cmp(&join_rank(&cb.identity))),
                _ => std::cmp::Ordering::Equal,
            })
    });
    idx.into_iter().map(|i| cells[i].id.clone()).collect()
}

fn stable_visual_order(
    state: &mut VoiceVisualState,
    target: &[String],
    max_items: usize,
) -> Vec<String> {
    if max_items == 0 || max_items != state.max_items {
        state.order = target.to_vec();
        state.max_items = max_items;
        return state.order.clone();
    }
    state.order = update_pages(&state.order, target, max_items);
    state.order.clone()
}

fn update_pages(current: &[String], next: &[String], max_items: usize) -> Vec<String> {
    let mut updated: Vec<String> = current.to_vec();
    if updated.len() < next.len() {
        let missing: Vec<String> = next
            .iter()
            .filter(|id| !updated.contains(id))
            .cloned()
            .collect();
        updated.extend(missing);
    }
    let page_count = updated
        .len()
        .div_ceil(max_items)
        .min(next.len().div_ceil(max_items));
    for page in 0..page_count {
        let page_of = |list: &[String]| -> Vec<String> {
            list.iter()
                .skip(page * max_items)
                .take(max_items)
                .cloned()
                .collect()
        };
        let updated_page = page_of(&updated);
        let next_page = page_of(next);
        let dropped: Vec<String> = updated_page
            .iter()
            .filter(|id| !next_page.contains(id))
            .cloned()
            .collect();
        let added: Vec<String> = next_page
            .iter()
            .filter(|id| !updated_page.contains(id))
            .cloned()
            .collect();
        if added.len() == dropped.len() {
            for (add, drop) in added.iter().zip(dropped.iter()) {
                let (Some(add_at), Some(drop_at)) = (
                    updated.iter().position(|x| x == add),
                    updated.iter().position(|x| x == drop),
                ) else {
                    return next.to_vec();
                };
                updated.swap(add_at, drop_at);
            }
        } else if added.is_empty() {
            updated.retain(|id| !dropped.contains(id));
        } else if dropped.is_empty() {
            for add in added {
                if !updated.contains(&add) {
                    updated.push(add);
                }
            }
        }
    }
    if updated.len() > next.len() {
        updated.retain(|id| next.contains(id));
    }
    updated
}

const AGENT_AVATAR_PATH: &str = "0/0/1779484387973271600/1737423959329_undefined173740153013517374015248704886401586613166392.png";

fn resolve_cell_identity(cx: &App, clan_id: ClanId, p: &VoiceParticipant) -> (String, String) {
    let (name, avatar_url) = resolve_voice_identity(cx, clan_id, &p.identity, &p.name);
    if p.is_agent {
        let source = crate::util::imgproxy::cdn_asset_url(cx, AGENT_AVATAR_PATH);
        (name, crate::util::imgproxy::avatar_url(cx, &source))
    } else {
        (name, avatar_url)
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
    crate::util::voice_member::resolve_display(cx, Some(clan_id), m)
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

enum InCallBodyLayout {
    Focus {
        cells: Vec<VideoCell>,
        focused_idx: usize,
    },
    Grid {
        cells: Vec<VideoCell>,
    },
}

#[allow(clippy::too_many_arguments)]
fn render_in_call(
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    connecting: bool,
    chat: &Entity<ChatLayout>,
    strip_scroll: &ScrollHandle,
    grid_page: usize,
    grid_size: gpui::Size<Pixels>,
    show_members: bool,
    visual: &mut VoiceVisualState,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let fullscreen_active = voice.read(cx).fullscreen_screen().is_some();

    let body_layout = (!fullscreen_active).then(|| {
        let store = voice.read(cx);
        let participants = store.participants();
        let focused = store.focused_tile();

        let mut cells: Vec<VideoCell> = Vec::new();
        for p in participants {
            if p.screenshare.is_some() {
                let (name, avatar) = resolve_cell_identity(cx, channel.clan_id, p);
                cells.push(VideoCell::screen(p, name, avatar));
            }
        }
        for p in participants {
            let (name, avatar) = resolve_cell_identity(cx, channel.clan_id, p);
            cells.push(VideoCell::camera(p, name, avatar));
        }

        let focused_id = focused
            .filter(|id| cells.iter().any(|c| c.id == *id))
            .map(|id| id.to_string());
        let target = target_visual_order(&cells, &|id| store.join_rank(id), &|id| {
            store.last_spoke_rank(id)
        });
        let stable = match &focused_id {
            None => {
                let (cols, rows) = select_grid_layout(
                    GRID_LAYOUTS,
                    cells.len().max(1),
                    f32::from(grid_size.width),
                    f32::from(grid_size.height),
                );
                Some(stable_visual_order(visual, &target, cols * rows))
            }
            Some(fid) if show_members => {
                let screen_focus = cells.iter().any(|c| &c.id == fid && c.is_screen);
                let strip_target: Vec<String> = if screen_focus {
                    target
                } else {
                    target.into_iter().filter(|id| id != fid).collect()
                };
                let bounds = strip_scroll.bounds();
                let viewport_w = f32::from(bounds.size.width);
                let aside_h = carousel_aside_height(f32::from(bounds.size.height));
                let max_items = if viewport_w > 0. {
                    carousel_max_visible_tiles(viewport_w, aside_h)
                } else {
                    0
                };
                Some(stable_visual_order(visual, &strip_target, max_items))
            }
            Some(_) => None,
        };
        if let Some(order) = stable {
            let ranks: HashMap<&str, usize> = order
                .iter()
                .enumerate()
                .map(|(rank, id)| (id.as_str(), rank))
                .collect();
            cells.sort_by_key(|c| ranks.get(c.id.as_str()).copied().unwrap_or(usize::MAX));
        }

        let focused_idx = focused_id.and_then(|fid| cells.iter().position(|c| c.id == fid));
        match focused_idx {
            Some(idx) => InCallBodyLayout::Focus {
                cells,
                focused_idx: idx,
            },
            None => InCallBodyLayout::Grid { cells },
        }
    });

    let body = body_layout.map(|layout| match layout {
        InCallBodyLayout::Focus { cells, focused_idx } => render_focus_layout(
            locale,
            voice,
            &cells,
            focused_idx,
            chat,
            show_members,
            strip_scroll,
            window,
            cx,
        ),
        InCallBodyLayout::Grid { cells } => render_grid(
            cx.theme(),
            locale,
            voice,
            &cells,
            &channel.voice_members,
            connecting,
            channel.clan_id,
            chat,
            grid_page,
            grid_size,
            cx,
        ),
    });

    let theme = cx.theme();
    let connection_status: Option<(SharedString, Hsla, bool)> = if connecting {
        Some((
            SharedString::from(mezon_i18n::t(locale, "channelVoice.connecting").to_string()),
            theme.text_primary.into(),
            true,
        ))
    } else {
        match voice.read(cx).call_status() {
            VoiceCallStatus::Reconnecting => Some((
                SharedString::from(mezon_i18n::t(locale, "channelVoice.reconnecting").to_string()),
                theme.status_idle.into(),
                true,
            )),
            VoiceCallStatus::WeakNetwork => Some((
                SharedString::from(mezon_i18n::t(locale, "channelVoice.weakNetwork").to_string()),
                theme.status_idle.into(),
                false,
            )),
            VoiceCallStatus::Stable => None,
        }
    };

    let connection_toast = connection_status.map(|(label, color, spinner)| {
        div()
            .absolute()
            .top(px(12.))
            .left_0()
            .right_0()
            .flex()
            .flex_row()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py(px(6.))
                    .rounded_full()
                    .bg(theme.bg_floating)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .when(spinner, |this| {
                        this.child(Spinner::new().with_size(Size::XSmall).color(color))
                    })
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(label),
                    ),
            )
            .into_any_element()
    });

    let mic_modal = voice
        .read(cx)
        .mic_permission_denied()
        .then(|| mic_permission_modal(theme, locale, voice));

    let participant_menu = voice
        .read(cx)
        .participant_menu()
        .and_then(|(identity, position)| {
            let participant = voice
                .read(cx)
                .participants()
                .iter()
                .find(|p| p.identity == identity)?;
            let (name, _) =
                resolve_voice_identity(cx, channel.clan_id, identity, &participant.name);
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

    let kick_modal = voice
        .read(cx)
        .pending_kick()
        .map(|(_, name)| kick_confirm_modal(theme, locale, voice, name));

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(voice_header(theme, &channel.name, true))
        .children(body)
        .when(!fullscreen_active, |this| {
            this.child(control_bar(
                theme,
                locale,
                voice,
                settings,
                voice.read(cx),
                chat,
                cx,
            ))
        })
        .children(connection_toast)
        .children(reactions_overlay(voice.read(cx)))
        .children(raised_hands_overlay(cx, channel.clan_id, voice.read(cx)))
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
    cx: &App,
) -> Option<AnyElement> {
    let key = store.fullscreen_screen()?;
    let exit_voice = voice.clone();
    let bg_voice = voice.clone();

    let media = match store.render_frame(key) {
        Some(frame) => render_voice_frame(frame, ObjectFit::Contain),
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
                    .child(control_bar(theme, locale, voice, settings, store, chat, cx)),
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

fn in_call_placeholder_cells(
    cx: &App,
    clan_id: ClanId,
    room_members: &[VoiceMember],
) -> Vec<VideoCell> {
    let local_id = mezon_store::BadgeService::global(cx)
        .read(cx)
        .current_user_id(cx);

    let mut cells = Vec::new();
    if let Some(uid) = local_id {
        let fallback = mezon_store::AccountStore::try_global(cx)
            .and_then(|account| {
                account
                    .read(cx)
                    .account
                    .as_ref()
                    .map(|a| a.display_name.clone())
            })
            .unwrap_or_default();
        let identity = uid.to_string();
        let (name, avatar_url) = resolve_voice_identity(cx, clan_id, &identity, &fallback);
        cells.push(VideoCell::placeholder(identity, name, avatar_url));
    }
    for member in room_members {
        if Some(member.user_id) == local_id {
            continue;
        }
        let (name, avatar_url) = resolve_voice_member(cx, clan_id, member);
        cells.push(VideoCell::placeholder(
            member.user_id.to_string(),
            name,
            avatar_url,
        ));
    }
    cells
}

#[allow(clippy::too_many_arguments)]
fn render_grid(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    cells: &[VideoCell],
    room_members: &[VoiceMember],
    connecting: bool,
    clan_id: ClanId,
    chat: &Entity<ChatLayout>,
    grid_page: usize,
    grid_size: gpui::Size<Pixels>,
    cx: &App,
) -> AnyElement {
    let store = voice.read(cx);
    let placeholder_cells: Vec<VideoCell>;
    let cells: &[VideoCell] = if !cells.is_empty() {
        cells
    } else {
        placeholder_cells = in_call_placeholder_cells(cx, clan_id, room_members);
        if placeholder_cells.is_empty() {
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
        &placeholder_cells
    };

    let count = cells.len();
    let measured = grid_size.width > px(0.) && grid_size.height > px(0.);
    let (cols, rows) = select_grid_layout(
        GRID_LAYOUTS,
        count,
        f32::from(grid_size.width),
        f32::from(grid_size.height),
    );
    let max_tiles = cols * rows;
    let total_pages = count.div_ceil(max_tiles).max(1);
    let page = grid_page.min(total_pages - 1);
    let page_offset = page * max_tiles;
    let paginated = measured && total_pages > 1;

    let mut grid = div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .p_2()
        .when(paginated, |this| this.pb(px(16.)))
        .bg(theme.bg_tertiary);
    for r in 0..rows {
        let mut row = div().flex().flex_row().flex_1().min_h_0().w_full().gap_2();
        for c in 0..cols {
            let index = page_offset + r * cols + c;
            let mut cell_el = div().flex_1().min_w_0().min_h_0().w_full();
            if index < count {
                cell_el = cell_el.child(video_tile(theme, locale, store, voice, &cells[index]));
            }
            row = row.child(cell_el);
        }
        grid = grid.child(row);
    }

    grid = grid.child({
        let chat = chat.clone();
        canvas(
            move |bounds, _, cx| {
                chat.update(cx, |layout, cx| {
                    layout.record_voice_grid_size(bounds.size, cx)
                })
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    });

    if paginated {
        let chat = chat.clone();
        grid = grid
            .on_scroll_wheel(move |event, window, cx| {
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                chat.update(cx, |layout, cx| {
                    layout.scroll_voice_grid_page(delta, total_pages, cx)
                });
            })
            .child(
                div()
                    .absolute()
                    .bottom(px(4.))
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(3.))
                            .px_2()
                            .py(px(3.))
                            .rounded_full()
                            .bg(gpui::rgb(0x1e1e1e))
                            .children((0..total_pages).map(|i| {
                                div()
                                    .size(px(6.))
                                    .rounded_full()
                                    .bg(gpui::rgb(0xffffff))
                                    .opacity(if i == page { 0.9 } else { 0.35 })
                            })),
                    ),
            );
    }
    grid.into_any_element()
}

enum GridOrientation {
    Portrait,
    Landscape,
}

struct GridLayoutDef {
    columns: usize,
    rows: usize,
    min_width: f32,
    orientation: Option<GridOrientation>,
}

impl GridLayoutDef {
    const fn new(columns: usize, rows: usize, min_width: f32) -> Self {
        Self {
            columns,
            rows,
            min_width,
            orientation: None,
        }
    }

    const fn oriented(columns: usize, rows: usize, orientation: GridOrientation) -> Self {
        Self {
            columns,
            rows,
            min_width: 0.,
            orientation: Some(orientation),
        }
    }

    fn max_tiles(&self) -> usize {
        self.columns * self.rows
    }

    fn fits_orientation(&self, landscape: bool) -> bool {
        match self.orientation {
            None => true,
            Some(GridOrientation::Landscape) => landscape,
            Some(GridOrientation::Portrait) => !landscape,
        }
    }
}

const GRID_LAYOUTS: &[GridLayoutDef] = &[
    GridLayoutDef::new(1, 1, 0.),
    GridLayoutDef::oriented(1, 2, GridOrientation::Portrait),
    GridLayoutDef::oriented(2, 1, GridOrientation::Landscape),
    GridLayoutDef::new(2, 2, 560.),
    GridLayoutDef::new(3, 3, 700.),
    GridLayoutDef::new(4, 4, 960.),
    GridLayoutDef::new(5, 5, 1100.),
];

fn select_grid_layout(
    layouts: &[GridLayoutDef],
    participant_count: usize,
    width: f32,
    height: f32,
) -> (usize, usize) {
    if width <= 0. || height <= 0. {
        return (layouts[0].columns, layouts[0].rows);
    }
    let landscape = width / height > 1.;
    let mut selected = None;
    for (index, layout) in layouts.iter().enumerate() {
        let bigger_same_capacity = layouts[index + 1..]
            .iter()
            .any(|l| l.max_tiles() == layout.max_tiles() && l.fits_orientation(landscape));
        if layout.max_tiles() >= participant_count && !bigger_same_capacity {
            selected = Some(index);
            break;
        }
    }
    let index = selected.unwrap_or(layouts.len() - 1);
    let layout = &layouts[index];
    if width < layout.min_width && index > 0 {
        let smaller_count = layouts[index - 1].max_tiles();
        return select_grid_layout(&layouts[..index], smaller_count, width, height);
    }
    (layout.columns, layout.rows)
}

const CAROUSEL_MIN_TILE_WIDTH: f32 = 140.;
const CAROUSEL_MAX_ROW_HEIGHT: f32 = 93.;
const CAROUSEL_SCROLLBAR_RESERVE: f32 = 14.;
const CAROUSEL_SCROLLBAR_GAP_MIN: f32 = 10.;
const CAROUSEL_ASPECT_RATIO: f32 = 16. / 10.;

fn carousel_tile_width(aside_height: f32) -> f32 {
    (aside_height.max(1.) * CAROUSEL_ASPECT_RATIO).max(CAROUSEL_MIN_TILE_WIDTH)
}

fn carousel_content_width(tile_count: usize, tile_width: f32, gap: f32) -> f32 {
    if tile_count == 0 {
        0.
    } else {
        tile_count as f32 * (tile_width + gap) - gap
    }
}

fn carousel_aside_height(strip_height: f32) -> f32 {
    if strip_height > 0. {
        strip_height
    } else {
        CAROUSEL_MAX_ROW_HEIGHT - 4.
    }
}

fn carousel_overflows(viewport_width: f32, tile_count: usize, aside_height: f32, gap: f32) -> bool {
    viewport_width > 0.
        && carousel_content_width(tile_count, carousel_tile_width(aside_height), gap)
            > viewport_width
}

fn carousel_max_visible_tiles(viewport_width: f32, aside_height: f32) -> usize {
    let target = (aside_height * CAROUSEL_ASPECT_RATIO).max(CAROUSEL_MIN_TILE_WIDTH);
    ((viewport_width / target).floor() as usize).max(1)
}

fn carousel_visible_range(
    total: usize,
    viewport_w: f32,
    tile_step: f32,
    scroll_offset_x: f32,
) -> (usize, usize) {
    if total == 0 || viewport_w <= 0. || tile_step <= 0. {
        return (0, total.min(16));
    }
    let scrolled = (-scroll_offset_x).max(0.);
    let first = (scrolled / tile_step) as usize;
    let visible = (viewport_w / tile_step).ceil() as usize + 1;
    (
        first.saturating_sub(2).min(total),
        (first + visible + 2).min(total),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_focus_layout(
    locale: &str,
    voice: &Entity<VoiceStore>,
    cells: &[VideoCell],
    focused_idx: usize,
    chat: &Entity<ChatLayout>,
    show_members: bool,
    strip_scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let focused = &cells[focused_idx];

    let (focused_tile, bg_tertiary) = {
        let theme = cx.theme();
        let store = voice.read(cx);
        (
            focus_main_tile(theme, locale, store, voice, focused),
            theme.bg_tertiary,
        )
    };

    let main = div()
        .flex()
        .flex_grow(5.)
        .flex_basis(px(0.))
        .min_h_0()
        .w_full()
        .child(focused_tile);

    let member_count = cells.iter().filter(|c| !c.is_screen).count();
    let toggle_pill = {
        let chat = chat.clone();
        div()
            .id("voice-member-strip-toggle")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(20.))
            .bg(gpui::rgb(0x2b2b2b))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                chat.update(cx, |layout, cx| layout.toggle_voice_member_strip(cx))
            })
            .child(
                Icon::new(if show_members {
                    IconName::VoiceArowDownIcon
                } else {
                    IconName::VoiceArowUpIcon
                })
                .size(px(20.))
                .text_color(gpui::rgb(0xffffff)),
            )
            .child(
                Icon::new(IconName::MemberList)
                    .size(px(20.))
                    .text_color(gpui::rgb(0xffffff)),
            )
            .child(
                div()
                    .pl(px(2.))
                    .pr(px(6.))
                    .text_color(gpui::rgb(0xffffff))
                    .child(member_count.to_string()),
            )
    };

    let container = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .p_2()
        .gap_2()
        .bg(bg_tertiary)
        .child(main);

    let container = if show_members {
        let strip_cells: Vec<&VideoCell> = if focused.is_screen {
            cells.iter().collect()
        } else {
            cells
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != focused_idx)
                .map(|(_, c)| c)
                .collect()
        };

        let strip_bounds = strip_scroll.bounds();
        let viewport = strip_bounds.size.width;
        let mut viewport_w = f32::from(viewport);
        if viewport_w <= 0. {
            viewport_w = (f32::from(window.viewport_size().width) * 0.55).max(400.);
        }
        let gap = 8.;
        let total = strip_cells.len();
        let strip_h = f32::from(strip_bounds.size.height);
        let aside_h = carousel_aside_height(strip_h);
        let tile_w = carousel_tile_width(aside_h);
        let tile_step = tile_w + gap;
        let overflows = carousel_overflows(viewport_w, total, aside_h, gap);
        let avatar_size = px((aside_h * 0.6).clamp(24., 80.));
        let (start, end) = if overflows {
            carousel_visible_range(
                total,
                viewport_w,
                tile_step,
                f32::from(strip_scroll.offset().x),
            )
        } else {
            (0, total)
        };
        let lead_spacer =
            (start > 0).then(|| div().flex_none().w(px(start as f32 * tile_step - gap)));
        let trail_spacer = (end < total).then(|| {
            div()
                .flex_none()
                .w(px((total - end) as f32 * tile_step - gap))
        });

        let scrollbar_gap = overflows.then(|| {
            div()
                .id("voice-carousel-scrollbar-gap")
                .flex_1()
                .min_h(px(CAROUSEL_SCROLLBAR_GAP_MIN))
                .w_full()
                .flex()
                .items_center()
                .child(
                    div()
                        .id("voice-carousel-scrollbar")
                        .w_full()
                        .h(px(CAROUSEL_SCROLLBAR_RESERVE))
                        .custom_scrollbars(
                            Scrollbars::always_visible(ScrollAxes::Horizontal)
                                .tracked_scroll_handle(strip_scroll),
                            window,
                            cx,
                        ),
                )
        });

        let theme = cx.theme();
        let strip_tiles = {
            let store = voice.read(cx);
            strip_cells[start..end]
                .iter()
                .copied()
                .map(|c| strip_tile(theme, locale, store, voice, c, tile_w, avatar_size))
                .collect::<Vec<_>>()
        };

        let tiles_row = div()
            .id("voice-carousel")
            .flex()
            .flex_row()
            .flex_none()
            .h(px(aside_h))
            .gap(px(gap))
            .when(!overflows, |this| this.justify_center())
            .children(lead_spacer)
            .children(strip_tiles)
            .children(trail_spacer);

        let carousel: AnyElement = if overflows {
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("voice-carousel-scroll")
                        .w_full()
                        .h(px(aside_h))
                        .overflow_x_scroll()
                        .track_scroll(strip_scroll)
                        .child(tiles_row),
                )
                .children(scrollbar_gap)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .flex()
                .flex_row()
                .justify_center()
                .child(tiles_row)
                .into_any_element()
        };

        let strip_max_h = if overflows {
            CAROUSEL_MAX_ROW_HEIGHT + CAROUSEL_SCROLLBAR_GAP_MIN + CAROUSEL_SCROLLBAR_RESERVE
        } else {
            CAROUSEL_MAX_ROW_HEIGHT
        };

        let strip = div()
            .relative()
            .flex_grow(1.)
            .flex()
            .flex_col()
            .flex_basis(px(0.))
            .min_h_0()
            .max_h(px(strip_max_h))
            .w_full()
            .child(carousel)
            .child(
                div()
                    .absolute()
                    .top(px(-26.))
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(toggle_pill),
            );

        container.child(strip)
    } else {
        container.relative().child(
            div()
                .absolute()
                .bottom(px(8.))
                .left_0()
                .right_0()
                .flex()
                .flex_row()
                .justify_center()
                .child(toggle_pill),
        )
    };

    container.into_any_element()
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
    let inner = tile_inner(theme, store, cell, px(120.));

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
        .child(tile_metadata(locale, cell))
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
    tile_width: f32,
    avatar_size: Pixels,
) -> AnyElement {
    let voice = voice.clone();
    let id = cell.id.clone();
    let inner = tile_inner(theme, store, cell, avatar_size);
    let border_color = speaking_border_color(cell);

    div()
        .id(SharedString::from(format!("strip-{}", cell.id)))
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h_full()
        .w(px(tile_width))
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .border(px(SPEAKING_BORDER_WIDTH))
        .border_color(border_color)
        .child(inner)
        .child(tile_metadata(locale, cell))
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

fn video_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
) -> AnyElement {
    let voice = voice.clone();
    let id = cell.id.clone();
    let inner = tile_inner(theme, store, cell, px(80.));
    let border_color = speaking_border_color(cell);

    div()
        .id(SharedString::from(format!("tile-{}", cell.id)))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .border(px(SPEAKING_BORDER_WIDTH))
        .border_color(border_color)
        .child(inner)
        .child(tile_metadata(locale, cell))
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

fn tile_inner(
    theme: &Theme,
    store: &VoiceStore,
    cell: &VideoCell,
    avatar_size: Pixels,
) -> AnyElement {
    if let Some(key) = cell.key
        && let Some(frame) = store.render_frame(key)
    {
        let fit = if cell.is_screen {
            ObjectFit::Contain
        } else {
            ObjectFit::Cover
        };
        return render_voice_frame(frame, fit);
    }

    if cell.is_screen {
        return Icon::new(IconName::VoiceScreenShareIcon)
            .size(px(48.))
            .text_color(theme.text_muted)
            .into_any_element();
    }

    let mut avatar = Avatar::new().name(cell.name.clone()).size_px(avatar_size);
    if !cell.avatar_url.is_empty() {
        avatar = avatar.src(cell.avatar_url.clone());
    }
    avatar.into_any_element()
}

fn render_voice_frame(frame: VoiceRenderFrame, fit: ObjectFit) -> AnyElement {
    match frame {
        VoiceRenderFrame::Image(image) => img(image).size_full().object_fit(fit).into_any_element(),
        #[cfg(target_os = "macos")]
        VoiceRenderFrame::Surface(surface) => gpui::surface(surface.into_inner())
            .size_full()
            .object_fit(fit)
            .into_any_element(),
    }
}

fn tile_metadata(locale: &str, cell: &VideoCell) -> AnyElement {
    let label = if cell.is_screen {
        mezon_i18n::t(locale, "channelVoice.usernameScreen").replace("{{username}}", &cell.name)
    } else {
        cell.name.clone()
    };

    let quality_icon = match cell.quality {
        NetworkQuality::Excellent => IconName::SvgQualityExcellentIcon,
        NetworkQuality::Good => IconName::SvgQualityGoodIcon,
        NetworkQuality::Poor => IconName::SvgQualityPoorIcon,
        NetworkQuality::Unknown => IconName::SvgQualityUnknownIcon,
    };

    div()
        .absolute()
        .left_2()
        .right_2()
        .bottom_2()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .min_w_0()
                .overflow_hidden()
                .p(px(5.))
                .rounded_md()
                .bg(gpui::rgba(0x00000080))
                .when(cell.muted && !cell.is_screen, |this| {
                    this.child(
                        Icon::new(IconName::VoiceMicDisabledIcon)
                            .size(px(16.))
                            .flex_none()
                            .text_color(gpui::rgba(0xffffff80)),
                    )
                })
                .when(cell.is_screen, |this| {
                    this.child(
                        Icon::new(IconName::VoiceScreenShareIcon)
                            .size(px(16.))
                            .flex_none()
                            .text_color(gpui::rgb(0xffffff)),
                    )
                })
                .child(
                    div()
                        .relative()
                        .min_w_0()
                        .py(px(2.))
                        .text_xs()
                        .line_height(px(12.))
                        .text_color(gpui::rgb(0xffffff))
                        .child(div().invisible().whitespace_nowrap().child(label.clone()))
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .py(px(2.))
                                .truncate()
                                .child(label),
                        ),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .p(px(5.))
                .rounded_md()
                .bg(gpui::rgba(0x00000080))
                .child(
                    Icon::new(quality_icon)
                        .size(px(16.))
                        .text_color(gpui::rgb(0xffffff)),
                ),
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

#[allow(clippy::too_many_arguments)]
fn control_bar(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    chat: &Entity<ChatLayout>,
    cx: &App,
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
            theme.text_primary,
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
            theme.text_primary,
        )
        .tooltip(Tooltip::text(camera_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_camera(cx)))
    };

    let mic_button = device_control(
        mic_button.into_any_element(),
        theme,
        locale,
        voice,
        settings,
        store,
        DeviceMenuKind::Microphone,
        cx,
    );
    let camera_button = device_control(
        camera_button.into_any_element(),
        theme,
        locale,
        voice,
        settings,
        store,
        DeviceMenuKind::Camera,
        cx,
    );

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
            gpui::rgb(LEAVE_RED),
            gpui::rgb(LEAVE_RED_HOVER).into(),
            IconName::EndCall,
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

    let agent_active = store.agent_active();
    let agent_clan_id = store
        .connection()
        .connected_channel()
        .and_then(|(_, clan)| clan.parse::<i64>().ok())
        .map(ClanId);
    let can_manage_agent = agent_clan_id.is_some_and(|clan_id| {
        PermissionStore::try_global(cx).is_some_and(|store| {
            store
                .read(cx)
                .check_permission(clan_id, PERMISSION_MANAGE_CHANNEL, cx)
        })
    });
    let agent_button = can_manage_agent.then(|| {
        let voice = voice.clone();
        let (bg, hover, color): (Hsla, Hsla, Hsla) = if agent_active {
            (
                gpui::rgb(ACCENT_BLUE).into(),
                darken(gpui::rgb(ACCENT_BLUE), 0.1),
                gpui::rgb(0xffffff).into(),
            )
        } else {
            (neutral_bg.into(), neutral_hover, theme.text_primary.into())
        };
        let agent_tooltip = mezon_i18n::t(
            locale,
            if agent_active {
                "channelVoice.removeAgent"
            } else {
                "channelVoice.addAgent"
            },
        );
        circle_button(
            "voice-agent-btn",
            bg,
            hover,
            IconName::VoiceAgentIcon,
            color,
        )
        .tooltip(Tooltip::text(agent_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_agent(cx)))
    });

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
        .children(agent_button)
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

const DEVICE_RADIO_BLUE: u32 = 0x3b82f6;

#[allow(clippy::too_many_arguments)]
fn device_control(
    button: AnyElement,
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    menu_kind: DeviceMenuKind,
    cx: &App,
) -> AnyElement {
    let is_open = store.device_menu() == Some(menu_kind);
    let arrow_id = match menu_kind {
        DeviceMenuKind::Microphone => "voice-mic-devices-btn",
        DeviceMenuKind::Camera => "voice-camera-devices-btn",
    };
    let arrow_hover = theme.bg_hover;
    let arrow = {
        let voice = voice.clone();
        div()
            .id(arrow_id)
            .absolute()
            .bottom(px(0.))
            .left(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded_full()
            .border_2()
            .border_color(theme.bg_tertiary)
            .bg(theme.bg_secondary)
            .cursor_pointer()
            .hover(move |s| s.bg(arrow_hover))
            .child(
                Icon::new(if is_open {
                    IconName::VoiceArowUpIcon
                } else {
                    IconName::VoiceArowDownIcon
                })
                .size(px(10.))
                .text_color(theme.text_primary),
            )
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                voice.update(cx, |store, cx| store.toggle_device_menu(menu_kind, cx));
            })
    };
    let flyout =
        is_open.then(|| device_flyout(theme, locale, voice, settings, store, menu_kind, cx));
    div()
        .relative()
        .child(button)
        .child(arrow)
        .children(flyout)
        .into_any_element()
}

fn device_flyout(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    menu_kind: DeviceMenuKind,
    cx: &App,
) -> AnyElement {
    let kinds: &[DeviceKind] = match menu_kind {
        DeviceMenuKind::Microphone => &[DeviceKind::AudioInput, DeviceKind::AudioOutput],
        DeviceMenuKind::Camera => &[DeviceKind::VideoInput],
    };
    let settings = settings.read(cx);
    let input_id = settings.input_device_id.clone();
    let output_id = settings.output_device_id.clone();
    let camera_id = settings.camera_device_id.clone();
    let active_for = |kind: DeviceKind| match kind {
        DeviceKind::AudioInput => input_id.clone(),
        DeviceKind::AudioOutput => output_id.clone(),
        DeviceKind::VideoInput => camera_id.clone(),
    };
    let submenu = store.device_submenu();

    let rows = kinds.iter().map(|&kind| {
        let entries = device_entries(kind, store, locale, cx);
        let active_id = active_for(kind);
        let active_name = active_device_name(&entries, &active_id);
        device_row(
            theme,
            locale,
            voice,
            kind,
            active_name,
            submenu == Some(kind),
        )
    });

    let rows_panel = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .w(px(220.))
        .p_2()
        .rounded_md()
        .bg(theme.tokens.bg_theme_contexify)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .children(rows);

    let list_panel = submenu.filter(|kind| kinds.contains(kind)).map(|kind| {
        let entries = device_entries(kind, store, locale, cx);
        let active_id = active_for(kind);
        device_list_panel(theme, voice, kind, entries, active_id)
    });

    deferred(
        div()
            .id("voice-device-flyout")
            .absolute()
            .bottom(px(56.))
            .left(px(-6.))
            .flex()
            .flex_row()
            .items_end()
            .gap_2()
            .occlude()
            .on_hover({
                let voice = voice.clone();
                move |hovered, _, cx| {
                    if !*hovered {
                        voice.update(cx, |store, cx| store.set_device_submenu(None, cx));
                    }
                }
            })
            .on_mouse_down_out({
                let voice = voice.clone();
                move |_: &MouseDownEvent, _, cx: &mut App| {
                    voice.update(cx, |store, cx| store.close_device_menu(cx));
                }
            })
            .child(rows_panel)
            .children(list_panel),
    )
    .into_any_element()
}

fn device_row(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    kind: DeviceKind,
    active_name: String,
    expanded: bool,
) -> AnyElement {
    let label = device_kind_label(kind, locale);
    let row_id = match kind {
        DeviceKind::AudioInput => "voice-device-row-input",
        DeviceKind::AudioOutput => "voice-device-row-output",
        DeviceKind::VideoInput => "voice-device-row-camera",
    };
    let base_bg = if expanded {
        theme.bg_hover
    } else {
        theme.bg_secondary
    };
    let hover_bg = theme.bg_hover;
    let voice = voice.clone();
    div()
        .id(row_id)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(px(6.))
        .bg(base_bg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .child(div().text_sm().text_color(theme.text_primary).child(label))
                .child(
                    div()
                        .mt(px(2.))
                        .text_xs()
                        .text_color(theme.text_muted)
                        .truncate()
                        .child(active_name),
                ),
        )
        .child(
            Icon::new(IconName::ChevronRight)
                .size(px(14.))
                .text_color(theme.text_muted),
        )
        .on_hover(move |hovered, _, cx| {
            if *hovered {
                voice.update(cx, |store, cx| store.set_device_submenu(Some(kind), cx));
            }
        })
        .into_any_element()
}

fn device_list_panel(
    theme: &Theme,
    voice: &Entity<VoiceStore>,
    kind: DeviceKind,
    entries: Vec<(Option<String>, String)>,
    active_id: Option<String>,
) -> AnyElement {
    let voice = voice.clone();
    let hover_bg = theme.bg_hover;
    let text_color = theme.text_primary;
    div()
        .id(SharedString::from(format!(
            "voice-device-list-{}",
            kind_slug(kind)
        )))
        .flex()
        .flex_col()
        .gap(px(2.))
        .min_w(px(240.))
        .max_h(px(320.))
        .overflow_y_scroll()
        .p_1()
        .rounded_md()
        .bg(theme.bg_secondary)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .children(entries.into_iter().map(move |(id, name)| {
            let selected = id.as_deref() == active_id.as_deref();
            let slug = id.as_deref().unwrap_or("default").to_string();
            let radio = device_radio(theme, selected);
            let voice = voice.clone();
            div()
                .id(SharedString::from(format!(
                    "dev-{}-{}",
                    kind_slug(kind),
                    slug
                )))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .w_full()
                .px_3()
                .py_2()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_sm()
                        .text_color(text_color)
                        .child(name),
                )
                .child(radio)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let id = id.clone();
                    voice.update(cx, |store, cx| match kind {
                        DeviceKind::AudioInput => store.set_input_device(id, cx),
                        DeviceKind::AudioOutput => store.set_output_device(id, cx),
                        DeviceKind::VideoInput => store.set_camera_device(id, cx),
                    });
                })
        }))
        .into_any_element()
}

fn device_radio(theme: &Theme, selected: bool) -> AnyElement {
    if selected {
        div()
            .flex_shrink_0()
            .size(px(16.))
            .rounded_full()
            .bg(gpui::rgb(DEVICE_RADIO_BLUE))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(6.)).rounded_full().bg(gpui::rgb(0xffffff)))
            .into_any_element()
    } else {
        div()
            .flex_shrink_0()
            .size(px(16.))
            .rounded_full()
            .border_2()
            .border_color(theme.text_muted)
            .into_any_element()
    }
}

fn device_entries(
    kind: DeviceKind,
    store: &VoiceStore,
    locale: &str,
    cx: &App,
) -> Vec<(Option<String>, String)> {
    let mut entries = vec![(
        None,
        mezon_i18n::t(locale, "channelVoice.device.systemDefault").to_string(),
    )];
    match kind {
        DeviceKind::AudioInput | DeviceKind::AudioOutput => {
            if let Some(audio) = AudioStore::try_global(cx) {
                let audio = audio.read(cx);
                let devices = if matches!(kind, DeviceKind::AudioInput) {
                    &audio.input_devices
                } else {
                    &audio.output_devices
                };
                for device in devices {
                    entries.push((Some(device.id.clone()), device.name.clone()));
                }
            }
        }
        DeviceKind::VideoInput => {
            for device in store.camera_devices() {
                entries.push((Some(device.id.clone()), device.name.clone()));
            }
        }
    }
    entries
}

fn device_kind_label(kind: DeviceKind, locale: &str) -> String {
    let key = match kind {
        DeviceKind::AudioInput => "channelVoice.device.inputDevice",
        DeviceKind::AudioOutput => "channelVoice.device.outputDevice",
        DeviceKind::VideoInput => "channelVoice.device.camera",
    };
    mezon_i18n::t(locale, key).to_string()
}

fn active_device_name(entries: &[(Option<String>, String)], active_id: &Option<String>) -> String {
    entries
        .iter()
        .find(|(id, _)| id == active_id)
        .or_else(|| entries.first())
        .map(|(_, name)| name.clone())
        .unwrap_or_default()
}

fn kind_slug(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::AudioInput => "input",
        DeviceKind::AudioOutput => "output",
        DeviceKind::VideoInput => "camera",
    }
}

#[cfg(test)]
mod carousel_tests {
    use super::{
        CAROUSEL_MIN_TILE_WIDTH, carousel_content_width, carousel_overflows, carousel_tile_width,
        carousel_visible_range,
    };

    #[test]
    fn uses_fixed_min_tile_width_like_react() {
        let tile_w = carousel_tile_width(89.);
        assert!(tile_w >= CAROUSEL_MIN_TILE_WIDTH);
        assert!(tile_w > carousel_tile_width(50.));
    }

    #[test]
    fn eleven_participants_overflow_typical_strip_viewport() {
        let tile_w = carousel_tile_width(89.);
        assert!(carousel_overflows(1200., 11, 89., 8.));
        assert!(carousel_content_width(11, tile_w, 8.) > 1200.);
    }

    #[test]
    fn participants_fit_without_scroll_on_wide_viewport() {
        assert!(!carousel_overflows(3000., 11, 89., 8.));
    }

    #[test]
    fn visible_range_windows_large_strip() {
        let tile_step = 148.;
        let (start, end) = carousel_visible_range(100, 600., tile_step, -450.);
        assert!(start < end);
        assert!(end - start < 20);
        assert!(start >= 1);
    }
}

#[cfg(test)]
mod device_menu_tests {
    use super::active_device_name;

    fn entries() -> Vec<(Option<String>, String)> {
        vec![
            (None, "System default".to_string()),
            (Some("a".to_string()), "Mic A".to_string()),
            (Some("b".to_string()), "Mic B".to_string()),
        ]
    }

    #[test]
    fn resolves_selected_device_name() {
        assert_eq!(
            active_device_name(&entries(), &Some("b".to_string())),
            "Mic B"
        );
    }

    #[test]
    fn resolves_system_default_when_none() {
        assert_eq!(active_device_name(&entries(), &None), "System default");
    }

    #[test]
    fn falls_back_to_first_when_missing() {
        assert_eq!(
            active_device_name(&entries(), &Some("gone".to_string())),
            "System default"
        );
    }
}
