use std::pin::pin;
use std::time::Duration;

use gpui::{
    App, Context, Div, Entity, FontWeight, ObjectFit, Pixels, Stateful, Subscription, Window,
    deferred, div, img, prelude::*, px, rgb, rgba,
};
use mezon_store::{
    AudioDeviceInfo, AudioStore, CallPeer, CallPhase, CallStore, ChannelId, DirectMessageStore,
    MediaFlags, Settings, VoiceRenderFrame,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};

const FRAME_FALLBACK: Duration = Duration::from_millis(200);
const CALL_GREEN: u32 = 0x3ba55d;
const CALL_RED: u32 = 0xda373c;

pub fn call_panel_active_for_dm(channel_id: i64, cx: &App) -> bool {
    let store = CallStore::global(cx).read(cx);
    !matches!(store.phase(), CallPhase::Idle)
        && store
            .peer()
            .is_some_and(|peer| peer.channel_id == channel_id)
}

fn viewing_call_dm(peer_channel: i64, cx: &App) -> bool {
    matches!(
        Router::global(cx).read(cx).route(),
        Route::DirectMessage { direct_id, .. } if direct_id.get() == peer_channel
    )
}

fn dm_message_type(channel_id: i64, cx: &App) -> String {
    DirectMessageStore::global(cx)
        .read(cx)
        .find(ChannelId(channel_id))
        .map(|dm| dm.kind.channel_type().to_string())
        .unwrap_or_else(|| "3".into())
}

#[derive(Clone, Copy)]
enum PermissionKind {
    Mic,
    Camera,
}

pub struct CallOverlay {
    call: Entity<CallStore>,
    last_phase: CallPhase,
}

impl CallOverlay {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let call = CallStore::global(cx);
        let last_phase = call.read(cx).phase();
        cx.observe(&call, |this, call, cx| {
            let phase = call.read(cx).phase();
            let was = this.last_phase;
            this.last_phase = phase;
            if matches!(phase, CallPhase::Connected) && !matches!(was, CallPhase::Connected) {
                let locale = Settings::try_global(cx)
                    .map(|s| s.read(cx).language.clone())
                    .unwrap_or_else(|| "en".to_string());
                let msg =
                    mezon_i18n::t(&locale, "channelVoice.toast.connectionConnected").to_string();
                Shell::global(cx).update(cx, |shell, cx| shell.success(msg, cx));
            }
            cx.notify();
        })
        .detach();
        cx.observe(&Router::global(cx), |_, _, cx| cx.notify())
            .detach();
        Self { call, last_phase }
    }

    fn render_permission_modal(
        &self,
        cx: &mut Context<Self>,
        kind: PermissionKind,
    ) -> gpui::AnyElement {
        let (
            card_bg,
            border,
            title_color,
            body_color,
            icon_bg,
            icon_color,
            later_bg,
            later_hover,
            open_bg,
            open_hover,
        ) = {
            let theme = cx.theme();
            (
                theme.bg_floating,
                theme.border,
                theme.text_primary,
                theme.text_muted,
                theme.bg_hover,
                theme.danger_text,
                theme.bg_tertiary,
                theme.bg_hover,
                theme.brand,
                theme.brand_hover,
            )
        };
        let (icon, title_key, body_key) = match kind {
            PermissionKind::Mic => (
                IconName::VoiceMicDisabledIcon,
                "channelVoice.micPermissionTitle",
                "channelVoice.micPermissionBody",
            ),
            PermissionKind::Camera => (
                IconName::VoiceCameraDisabledIcon,
                "channelVoice.permission.cameraTitle",
                "channelVoice.permission.cameraBody",
            ),
        };
        let locale = Settings::try_global(cx)
            .map(|s| s.read(cx).language.clone())
            .unwrap_or_default();
        let title = mezon_i18n::t(&locale, title_key).to_string();
        let body = mezon_i18n::t(&locale, body_key).to_string();
        let open_label = mezon_i18n::t(&locale, "channelVoice.openSettings").to_string();
        let later_label = mezon_i18n::t(&locale, "channelVoice.later").to_string();
        let call_later = self.call.clone();
        let call_open = self.call.clone();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000cc))
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_4()
                    .w(px(360.))
                    .p_6()
                    .rounded_xl()
                    .bg(card_bg)
                    .border_1()
                    .border_color(border)
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(56.))
                            .h(px(56.))
                            .rounded_full()
                            .bg(icon_bg)
                            .child(Icon::new(icon).size(px(26.)).text_color(icon_color)),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(title_color)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(body_color)
                            .child(body),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .w_full()
                            .child(
                                div()
                                    .id("call-mic-perm-later")
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .py_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(later_bg)
                                    .text_color(title_color)
                                    .hover(move |s| s.bg(later_hover))
                                    .on_click(move |_, _, cx| {
                                        call_later.update(cx, |store, cx| match kind {
                                            PermissionKind::Mic => store.dismiss_mic_prompt(cx),
                                            PermissionKind::Camera => {
                                                store.dismiss_camera_prompt(cx)
                                            }
                                        });
                                    })
                                    .child(later_label),
                            )
                            .child(
                                div()
                                    .id("call-mic-perm-open")
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .py_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(open_bg)
                                    .text_color(rgb(0xffffff))
                                    .hover(move |s| s.bg(open_hover))
                                    .on_click(move |_, _, cx| {
                                        match kind {
                                            PermissionKind::Mic => open_microphone_settings(),
                                            PermissionKind::Camera => open_camera_settings(),
                                        }
                                        call_open.update(cx, |store, cx| match kind {
                                            PermissionKind::Mic => store.dismiss_mic_prompt(cx),
                                            PermissionKind::Camera => {
                                                store.dismiss_camera_prompt(cx)
                                            }
                                        });
                                    })
                                    .child(open_label),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn open_microphone_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:privacy-microphone"])
            .spawn();
    }
}

fn open_camera_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Camera")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:privacy-webcam"])
            .spawn();
    }
}

impl Render for CallOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, peer) = {
            let store = self.call.read(cx);
            (store.phase(), store.peer().cloned())
        };
        if self.call.read(cx).mic_prompt() {
            return self.render_permission_modal(cx, PermissionKind::Mic);
        }
        if self.call.read(cx).camera_prompt() {
            return self.render_permission_modal(cx, PermissionKind::Camera);
        }
        let Some(peer) = peer else {
            return div().into_any_element();
        };
        if !matches!(phase, CallPhase::Incoming) || viewing_call_dm(peer.channel_id, cx) {
            return div().into_any_element();
        }
        let name = display_name(&peer);
        let avatar = peer.avatar.clone();
        let call = self.call.clone();
        let (card_bg, title_color, subtitle_color) = {
            let theme = cx.theme();
            (theme.bg_secondary, theme.text_primary, theme.text_secondary)
        };
        let locale = Settings::try_global(cx)
            .map(|s| s.read(cx).language.clone())
            .unwrap_or_default();
        let incoming_label = mezon_i18n::t(&locale, "message.callLog.incomingCall").to_string();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000cc))
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_4()
                    .w(px(280.))
                    .p_6()
                    .rounded_xl()
                    .bg(card_bg)
                    .child(avatar_element(&name, avatar.as_deref(), px(96.)))
                    .child(div().text_color(title_color).text_xl().child(name))
                    .child(div().text_color(subtitle_color).child(incoming_label))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_5()
                            .mt_2()
                            .child(circle_button(
                                "call-decline",
                                IconName::EndCall,
                                CALL_RED,
                                px(56.),
                                {
                                    let call = call.clone();
                                    move |cx: &mut App| {
                                        call.update(cx, |store, cx| store.decline(cx));
                                    }
                                },
                            ))
                            .child(circle_button(
                                "call-accept-audio",
                                IconName::IconPhoneDM,
                                CALL_GREEN,
                                px(56.),
                                {
                                    let call = call.clone();
                                    move |cx: &mut App| {
                                        call.update(cx, |store, cx| store.accept(false, cx));
                                    }
                                },
                            )),
                    ),
            )
            .into_any_element()
    }
}

pub struct CallPanelView {
    call: Entity<CallStore>,
    input_devices: Vec<AudioDeviceInfo>,
    output_devices: Vec<AudioDeviceInfo>,
    default_input: Option<String>,
    default_output: Option<String>,
    device_menu_open: bool,
    _frame_pump: Option<gpui::Task<()>>,
    _audio_sub: Option<Subscription>,
}

impl CallPanelView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let call = CallStore::global(cx);
        cx.observe(&call, |this, call, cx| {
            let active = !matches!(call.read(cx).phase(), CallPhase::Idle);
            if active {
                if this._frame_pump.is_none() {
                    this.start_frame_pump(cx);
                }
            } else {
                this._frame_pump = None;
            }
            cx.notify();
        })
        .detach();
        cx.observe(&Router::global(cx), |_, _, cx| cx.notify())
            .detach();
        let mut audio_sub = None;
        let (input_devices, output_devices, default_input, default_output) =
            if let Some(audio) = AudioStore::try_global(cx) {
                audio_sub = Some(cx.observe(&audio, |this, store, cx| {
                    let store = store.read(cx);
                    this.input_devices = store.input_devices.clone();
                    this.output_devices = store.output_devices.clone();
                    this.default_input = store.default_input_name.clone();
                    this.default_output = store.default_output_name.clone();
                    cx.notify();
                }));
                AudioStore::ensure_devices(&audio, cx);
                let store = audio.read(cx);
                (
                    store.input_devices.clone(),
                    store.output_devices.clone(),
                    store.default_input_name.clone(),
                    store.default_output_name.clone(),
                )
            } else {
                (Vec::new(), Vec::new(), None, None)
            };
        let mut this = Self {
            call,
            input_devices,
            output_devices,
            default_input,
            default_output,
            device_menu_open: false,
            _frame_pump: None,
            _audio_sub: audio_sub,
        };
        if !matches!(this.call.read(cx).phase(), CallPhase::Idle) {
            this.start_frame_pump(cx);
        }
        this
    }

    fn start_frame_pump(&mut self, cx: &mut Context<Self>) {
        self._frame_pump = Some(cx.spawn(async move |this, cx| {
            let mut last_seq = 0u64;
            loop {
                let store = match this.update(cx, |this, cx| this.call.read(cx).frame_store()) {
                    Ok(store) => store,
                    Err(_) => break,
                };
                let Some(store) = store else {
                    cx.background_executor().timer(FRAME_FALLBACK).await;
                    continue;
                };
                {
                    let mut rx = store.frame_watch();
                    let changed = pin!(rx.changed());
                    let fallback = pin!(cx.background_executor().timer(FRAME_FALLBACK));
                    let _ = futures::future::select(changed, fallback).await;
                }
                let seq = store.publish_seq();
                if seq != last_seq {
                    last_seq = seq;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }
        }));
    }

    fn render_panel(
        &self,
        surface: gpui::Rgba,
        border: gpui::Rgba,
        phase: CallPhase,
        peer: CallPeer,
        local: MediaFlags,
        remote: MediaFlags,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let name = display_name(&peer);
        let avatar = peer.avatar.clone();
        let (self_name, self_avatar) = {
            let store = self.call.read(cx);
            (
                store.self_name().to_string(),
                store.self_avatar().to_string(),
            )
        };
        let remote_active = remote.cam_on || self.call.read(cx).has_remote_video();
        let two_tile = local.cam_on || remote_active;

        let (remote_frame, self_frame) = if two_tile {
            let remote_key = self.call.read(cx).remote_frame_key();
            let self_key = self.call.read(cx).self_frame_key();
            let remote_frame = remote
                .cam_on
                .then(|| self.call.read(cx).render_frame(remote_key))
                .flatten();
            let self_frame = local
                .cam_on
                .then(|| self.call.read(cx).render_frame(self_key))
                .flatten();
            self.call
                .update(cx, |store, cx| store.flush_texture_drops(Some(window), cx));
            (remote_frame, self_frame)
        } else {
            (None, None)
        };

        let self_avatar_opt = (!self_avatar.is_empty()).then_some(self_avatar.as_str());

        let content = if two_tile {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_4()
                .w_full()
                .flex_1()
                .min_h(px(0.))
                .child(video_tile(
                    remote_frame,
                    &name,
                    avatar.as_deref(),
                    remote.mic_on,
                ))
                .child(video_tile(
                    self_frame,
                    &self_name,
                    self_avatar_opt,
                    local.mic_on,
                ))
        } else {
            div()
                .relative()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(avatar_element(&name, avatar.as_deref(), px(72.)))
                .when(
                    !remote.mic_on && matches!(phase, CallPhase::Connected),
                    |el| {
                        el.child(
                            div()
                                .absolute()
                                .bottom_2()
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::VoiceMicDisabledIcon)
                                        .size(px(20.))
                                        .text_color(rgb(0xffffff)),
                                ),
                        )
                    },
                )
        };

        let controls = self.render_controls(phase, local, cx);

        div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .w_full()
            .flex_none()
            .h(if two_tile { px(320.) } else { px(200.) })
            .p_3()
            .gap_3()
            .border_b_1()
            .border_color(border)
            .bg(surface)
            .child(content)
            .child(controls)
            .into_any_element()
    }

    fn render_controls(
        &self,
        phase: CallPhase,
        local: MediaFlags,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let call = self.call.clone();
        let row = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_3()
            .w_full()
            .flex_none();
        if matches!(phase, CallPhase::Incoming) {
            row.child(circle_button(
                "call-panel-accept-video",
                IconName::VoiceCameraIcon,
                CALL_GREEN,
                px(52.),
                {
                    let call = call.clone();
                    move |cx: &mut App| {
                        call.update(cx, |store, cx| store.accept(true, cx));
                    }
                },
            ))
            .child(circle_button(
                "call-panel-accept-audio",
                IconName::IconPhoneDM,
                CALL_GREEN,
                px(52.),
                {
                    let call = call.clone();
                    move |cx: &mut App| {
                        call.update(cx, |store, cx| store.accept(false, cx));
                    }
                },
            ))
            .child(circle_button(
                "call-panel-decline",
                IconName::EndCall,
                CALL_RED,
                px(52.),
                {
                    let call = call.clone();
                    move |cx: &mut App| {
                        call.update(cx, |store, cx| store.decline(cx));
                    }
                },
            ))
            .into_any_element()
        } else {
            row.child(circle_button(
                "call-panel-camera",
                if local.cam_on {
                    IconName::VoiceCameraIcon
                } else {
                    IconName::VoiceCameraDisabledIcon
                },
                if local.cam_on { CALL_GREEN } else { 0x4e5058 },
                px(52.),
                {
                    let call = call.clone();
                    move |cx: &mut App| {
                        call.update(cx, |store, cx| store.toggle_camera(cx));
                    }
                },
            ))
            .child(circle_button(
                "call-panel-mic",
                if local.mic_on {
                    IconName::VoiceMicIcon
                } else {
                    IconName::VoiceMicDisabledIcon
                },
                if local.mic_on { 0x4e5058 } else { CALL_RED },
                px(52.),
                {
                    let call = call.clone();
                    move |cx: &mut App| {
                        call.update(cx, |store, cx| store.toggle_mic(cx));
                    }
                },
            ))
            .child(self.render_settings_button(cx))
            .child(circle_button(
                "call-panel-hangup",
                IconName::EndCall,
                CALL_RED,
                px(52.),
                {
                    let call = call.clone();
                    move |cx: &mut App| {
                        call.update(cx, |store, cx| store.hangup(cx));
                    }
                },
            ))
            .into_any_element()
        }
    }

    fn render_settings_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view = cx.entity();
        div()
            .relative()
            .child(circle_button(
                "call-panel-settings",
                IconName::CallSetting,
                if self.device_menu_open {
                    0x6d6f78
                } else {
                    0x4e5058
                },
                px(52.),
                {
                    let view = view.clone();
                    move |cx: &mut App| {
                        view.update(cx, |this, cx| {
                            this.device_menu_open = !this.device_menu_open;
                            cx.notify();
                        });
                    }
                },
            ))
            .when(self.device_menu_open, |el| {
                el.child(deferred(self.render_device_menu(cx)))
            })
            .into_any_element()
    }

    fn render_device_menu(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected_in = self.call.read(cx).selected_input().map(|s| s.to_string());
        let selected_out = self.call.read(cx).selected_output().map(|s| s.to_string());
        let default_in = self
            .default_input
            .clone()
            .unwrap_or_else(|| "System default".to_string());
        let default_out = self
            .default_output
            .clone()
            .unwrap_or_else(|| "System default".to_string());

        let mut menu = div()
            .absolute()
            .top(px(58.))
            .left(px(-114.))
            .w(px(280.))
            .p_1()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_lg()
            .bg(rgb(0x1e1f22))
            .border_1()
            .border_color(rgba(0x00000066))
            .shadow_lg()
            .occlude()
            .child(device_section_header("Input device"))
            .child(self.device_row(
                None,
                format!("Default — {default_in}"),
                selected_in.is_none(),
                true,
                cx,
            ));
        for device in self.input_devices.clone() {
            let selected = selected_in.as_deref() == Some(device.id.as_str());
            menu = menu.child(self.device_row(Some(device.id), device.name, selected, true, cx));
        }
        menu = menu
            .child(div().h(px(1.)).my_1().bg(rgba(0xffffff14)))
            .child(device_section_header("Output device"))
            .child(self.device_row(
                None,
                format!("Default — {default_out}"),
                selected_out.is_none(),
                false,
                cx,
            ));
        for device in self.output_devices.clone() {
            let selected = selected_out.as_deref() == Some(device.id.as_str());
            menu = menu.child(self.device_row(Some(device.id), device.name, selected, false, cx));
        }
        menu.into_any_element()
    }

    fn device_row(
        &self,
        id: Option<String>,
        label: String,
        selected: bool,
        is_input: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let call = self.call.clone();
        let view = cx.entity();
        let row_id = format!(
            "call-dev-{}-{}",
            if is_input { "in" } else { "out" },
            id.as_deref().unwrap_or("default")
        );
        div()
            .id(row_id)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x35373c)))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(0xffffff))
                    .child(label),
            )
            .when(selected, |el| {
                el.child(
                    Icon::new(IconName::Check)
                        .size(px(16.))
                        .text_color(rgb(CALL_GREEN)),
                )
            })
            .on_click(move |_, _, cx| {
                let id = id.clone();
                call.update(cx, |store, cx| {
                    if is_input {
                        store.set_input_device(id, cx);
                    } else {
                        store.set_output_device(id, cx);
                    }
                });
                view.update(cx, |this, cx| {
                    this.device_menu_open = false;
                    cx.notify();
                });
            })
            .into_any_element()
    }
}

impl Render for CallPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = cx.theme().bg_secondary;
        let border = cx.theme().border;
        let (phase, peer, local, remote) = {
            let store = self.call.read(cx);
            (
                store.phase(),
                store.peer().cloned(),
                store.local_flags(),
                store.remote_flags(),
            )
        };
        let Some(peer) = peer else {
            return div().into_any_element();
        };
        if matches!(phase, CallPhase::Idle) || !viewing_call_dm(peer.channel_id, cx) {
            return div().into_any_element();
        }
        self.render_panel(surface, border, phase, peer, local, remote, window, cx)
    }
}

pub fn render_call_mini_bar(theme: &Theme, cx: &App) -> Option<gpui::AnyElement> {
    let call = CallStore::global(cx);
    let store = call.read(cx);
    let phase = store.phase();
    if !matches!(
        phase,
        CallPhase::Outgoing | CallPhase::Connecting | CallPhase::Connected
    ) {
        return None;
    }
    let peer = store.peer()?.clone();
    let connected = matches!(phase, CallPhase::Connected);
    let status = if connected {
        "Call Connected"
    } else {
        "Calling…"
    };
    let name = display_name(&peer);
    let name = if name.chars().count() > 30 {
        format!("{}...", name.chars().take(30).collect::<String>())
    } else {
        name
    };
    let channel_id = peer.channel_id;
    let hover_color = theme.text_primary;
    let call_hangup = call.clone();

    Some(
        div()
            .flex()
            .flex_col()
            .px_3()
            .py_2()
            .border_b_2()
            .border_color(theme.tokens.border_primary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
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
                                            .text_color(rgb(CALL_GREEN)),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(CALL_GREEN))
                                            .font_weight(FontWeight::BOLD)
                                            .child(status),
                                    ),
                            )
                            .child(
                                div()
                                    .id("call-mini-jump")
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .hover(move |s| s.text_color(hover_color))
                                    .child(name)
                                    .on_click(move |_, _, cx| {
                                        let message_type = dm_message_type(channel_id, cx);
                                        crate::router::navigate(
                                            cx,
                                            Route::DirectMessage {
                                                direct_id: ChannelId(channel_id),
                                                message_type,
                                            },
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("call-mini-hangup")
                            .cursor_pointer()
                            .p_1()
                            .rounded_md()
                            .hover(|s| s.bg(rgba(0x5e5e5e66)))
                            .child(
                                Icon::new(IconName::EndCall)
                                    .size(px(20.))
                                    .text_color(theme.text_secondary),
                            )
                            .on_click(move |_, _, cx| {
                                call_hangup.update(cx, |store, cx| store.hangup(cx));
                            }),
                    ),
            )
            .into_any_element(),
    )
}

fn display_name(peer: &CallPeer) -> String {
    if peer.name.is_empty() {
        "Unknown".to_string()
    } else {
        peer.name.clone()
    }
}

fn avatar_element(name: &str, avatar: Option<&str>, size: Pixels) -> gpui::AnyElement {
    let mut element = Avatar::new().name(name.to_string()).size_px(size);
    if let Some(url) = avatar
        && !url.is_empty()
    {
        element = element.src(url.to_string());
    }
    element.into_any_element()
}

fn frame_element(
    frame: Option<VoiceRenderFrame>,
    name: &str,
    avatar: Option<&str>,
    avatar_size: Pixels,
) -> gpui::AnyElement {
    match frame {
        Some(VoiceRenderFrame::Image(image)) => img(image)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        #[cfg(target_os = "macos")]
        Some(VoiceRenderFrame::Surface(surface)) => gpui::surface(surface.into_inner())
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        _ => avatar_element(name, avatar, avatar_size),
    }
}

fn video_tile(
    frame: Option<VoiceRenderFrame>,
    name: &str,
    avatar: Option<&str>,
    mic_on: bool,
) -> gpui::AnyElement {
    div()
        .relative()
        .w(px(320.))
        .h(px(220.))
        .rounded_lg()
        .overflow_hidden()
        .bg(rgb(0x000000))
        .flex()
        .items_center()
        .justify_center()
        .child(frame_element(frame, name, avatar, px(56.)))
        .when(!mic_on, |el| {
            el.child(
                div()
                    .absolute()
                    .bottom_2()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        Icon::new(IconName::VoiceMicDisabledIcon)
                            .size(px(24.))
                            .text_color(rgb(0xffffff)),
                    ),
            )
        })
        .into_any_element()
}

fn device_section_header(text: &'static str) -> gpui::AnyElement {
    div()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(0x949ba4))
        .child(text)
        .into_any_element()
}

fn circle_button(
    id: &'static str,
    icon: IconName,
    bg: u32,
    size: Pixels,
    on_click: impl Fn(&mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(size)
        .h(size)
        .rounded_full()
        .bg(rgb(bg))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .occlude()
        .child(Icon::new(icon).size(px(22.)).text_color(rgb(0xffffff)))
        .on_click(move |_, _, cx| on_click(cx))
}
