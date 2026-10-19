use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FocusHandle, FontWeight, Global,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, ListAlignment, ListState,
    ParentElement, Pixels, Render, ScrollHandle, Size, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, list,
    prelude::FluentBuilder, px, size,
};
use mezon_store::{
    AuthState, LoginStore, MeetingChatMessage, MeetingChatStatus, Settings, VoiceStore,
};

use crate::app::title_bar::TitleBar;
use crate::app::window_controls;
use crate::chat::voice::{
    CallLayoutHost, VoiceVisualState, guest_avatar_url, next_voice_grid_page,
    render_external_meeting, render_screen_fullscreen_overlay, spawn_voice_frame_pump,
};
use crate::components::primitives::{Avatar, Icon, IconName, Input, InputEvent, InputState};
use crate::theme::{ActiveTheme, Theme};

const CHAT_PANEL_WIDTH: f32 = 340.;

#[derive(Default)]
struct MeetingWindows(HashMap<String, WindowHandle<MeetingView>>);

impl Global for MeetingWindows {}

struct PendingMeetingLink {
    code: Option<String>,
    _auth: Subscription,
}

impl Global for PendingMeetingLink {}

fn has_session(state: &AuthState) -> bool {
    matches!(
        state,
        AuthState::Authenticated(_) | AuthState::Connecting(_)
    )
}

pub fn deep_link_meeting_code(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("mezonapp://")?;
    let path = rest.split(['?', '#']).next()?;
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    match (segments.next(), segments.next(), segments.next()) {
        (Some("meeting"), Some(code), None) => {
            mezon_store::meeting_room_id(code).map(|_| code.to_string())
        }
        _ => None,
    }
}

pub fn open_meeting_from_external_trigger(code: String, cx: &mut App) {
    let Some(auth) = LoginStore::try_global(cx).map(|login| login.read(cx).auth_state()) else {
        return;
    };
    if matches!(auth.read(cx), AuthState::Authenticated(_)) {
        open_meeting_window(code, cx);
        return;
    }
    tracing::info!("meeting deep link waits for sign-in");
    let subscription = cx.observe(&auth, |auth, cx| {
        if !matches!(auth.read(cx), AuthState::Authenticated(_)) {
            return;
        }
        let code = cx
            .try_global::<PendingMeetingLink>()
            .and_then(|pending| pending.code.clone());
        let Some(code) = code else {
            return;
        };
        cx.global_mut::<PendingMeetingLink>().code = None;
        open_meeting_window(code, cx);
    });
    cx.set_global(PendingMeetingLink {
        code: Some(code),
        _auth: subscription,
    });
}

pub fn open_meeting_window(code: String, cx: &mut App) {
    let Some(room_id) = mezon_store::meeting_room_id(&code) else {
        return;
    };
    let existing = cx
        .try_global::<MeetingWindows>()
        .and_then(|windows| windows.0.get(&room_id).copied());
    if let Some(handle) = existing {
        let activated = handle.update(cx, |view, window, cx| {
            window.activate_window();
            window.focus(&view.focus_handle, cx);
        });
        if activated.is_ok() {
            return;
        }
        cx.default_global::<MeetingWindows>().0.remove(&room_id);
    }
    let Some(settings) = Settings::try_global(cx) else {
        return;
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1100.), px(720.)),
            cx,
        ))),
        window_min_size: Some(size(px(640.), px(480.))),
        kind: WindowKind::Normal,
        focus: true,
        show: true,
        is_movable: true,
        is_resizable: true,
        titlebar: Some(window_controls::window_title_options()),
        window_decorations: window_controls::main_window_decorations(),
        app_id: window_controls::linux_app_id(),
        ..Default::default()
    };
    let window_room = room_id.clone();
    match cx.open_window(options, |window, cx| {
        cx.new(|cx| MeetingView::new(code, window_room, settings, window, cx))
    }) {
        Ok(handle) => {
            cx.default_global::<MeetingWindows>()
                .0
                .insert(room_id, handle);
            let _ = handle.update(cx, |view, window, cx| {
                window.activate_window();
                window.focus(&view.focus_handle, cx);
            });
        }
        Err(error) => tracing::error!("failed to open meeting window: {error}"),
    }
}

pub struct MeetingView {
    code: String,
    room_id: String,
    voice: Entity<VoiceStore>,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    title_bar: Entity<TitleBar>,
    strip_scroll: ScrollHandle,
    strip_width: Pixels,
    grid_page: usize,
    grid_wheel_accum: f32,
    grid_size: Size<Pixels>,
    visual: VoiceVisualState,
    chat_open: bool,
    chat_read_seq: u64,
    chat_focus_pending: bool,
    chat_list: ListState,
    chat_first_seq: Option<u64>,
    chat_count: usize,
    chat_input: Entity<InputState>,
    _frame_pump: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl MeetingView {
    fn new(
        code: String,
        room_id: String,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let voice = VoiceStore::global(cx);
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let locale = settings.read(cx).language.clone();
        window.set_window_title(mezon_i18n::t(
            &locale,
            "eventCreator.eventDetail.privateRoom",
        ));

        cx.on_release(|view: &mut Self, cx| {
            cx.default_global::<MeetingWindows>()
                .0
                .remove(&view.room_id);
            let room_id = view.room_id.clone();
            view.voice
                .update(cx, |store, cx| store.leave_closed_meeting(&room_id, cx));
        })
        .detach();

        let chat_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(&locale, "meeting.chat.placeholder"))
        });
        let subscriptions = vec![
            cx.observe(&voice, |this, _, cx| {
                this.sync_frame_pump(cx);
                this.sync_chat_list(cx);
                cx.notify();
            }),
            cx.subscribe(&chat_input, |this, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::PressEnter) {
                    return;
                }
                let text = input.read(cx).value().to_string();
                let accepted = this
                    .voice
                    .update(cx, |store, cx| store.send_meeting_chat(&text, cx));
                if accepted {
                    input.update(cx, |input, cx| input.clear(cx));
                }
            }),
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() || !this.in_meeting(cx) {
                    return;
                }
                if let Some(store) = this.voice.read(cx).frame_store() {
                    store.screen_views.clear_main();
                }
                this.voice
                    .update(cx, |store, cx| store.set_push_to_talk(false, cx));
            }),
        ];
        let mut subscriptions = subscriptions;
        if let Some(auth) = LoginStore::try_global(cx).map(|login| login.read(cx).auth_state()) {
            subscriptions.push(cx.observe_in(&auth, window, |_, auth, window, cx| {
                if !has_session(auth.read(cx)) {
                    window.remove_window();
                }
            }));
        }
        let title_bar = cx.new(|cx| TitleBar::new(settings.clone(), cx));

        let mut this = Self {
            code,
            room_id,
            voice,
            settings,
            focus_handle,
            title_bar,
            strip_scroll: ScrollHandle::new(),
            strip_width: px(0.),
            grid_page: 0,
            grid_wheel_accum: 0.,
            grid_size: Size::default(),
            visual: VoiceVisualState::default(),
            chat_open: false,
            chat_read_seq: 0,
            chat_focus_pending: false,
            chat_list: ListState::new(0, ListAlignment::Bottom, px(200.)),
            chat_first_seq: None,
            chat_count: 0,
            chat_input,
            _frame_pump: None,
            _subscriptions: subscriptions,
        };
        this.sync_frame_pump(cx);
        this.sync_chat_list(cx);
        this.chat_read_seq = this
            .voice
            .read(cx)
            .meeting_chat()
            .last()
            .map_or(0, |message| message.seq);
        this
    }

    fn sync_chat_list(&mut self, cx: &App) {
        let messages = self.voice.read(cx).meeting_chat();
        let first = messages.first().map(|message| message.seq);
        let count = messages.len();
        match chat_list_change(self.chat_first_seq, self.chat_count, first, count) {
            ChatListChange::Splice {
                removed_front,
                kept,
                appended,
            } => {
                if removed_front > 0 {
                    self.chat_list.splice(0..removed_front, 0);
                }
                if appended > 0 {
                    self.chat_list.splice(kept..kept, appended);
                }
            }
            ChatListChange::Reset => self.chat_list.reset(count),
        }
        self.chat_first_seq = first;
        self.chat_count = count;
    }

    fn render_chat_panel(&self, theme: &Theme, locale: &str, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let empty = self.chat_count == 0;
        let body = if empty {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .px_6()
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_sm()
                        .whitespace_normal()
                        .text_color(theme.text_secondary)
                        .child(mezon_i18n::t(locale, "meeting.chat.empty")),
                )
                .into_any_element()
        } else {
            list(self.chat_list.clone(), move |index, _window, cx| {
                entity.update(cx, |this, cx| {
                    let message = this.voice.read(cx).meeting_chat().get(index).cloned();
                    let locale = this.settings.read(cx).language.clone();
                    message
                        .map(|message| render_chat_message(&message, &locale, cx))
                        .unwrap_or_else(|| div().into_any_element())
                })
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(CHAT_PANEL_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.bg_secondary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(50.))
                    .px_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(locale, "meeting.chat.title")),
                    )
                    .child(
                        div()
                            .id("meeting-chat-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.))
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(14.))
                                    .text_color(theme.text_secondary),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_voice_chat(cx))),
                    ),
            )
            .child(body)
            .child(div().p_3().child(Input::new(&self.chat_input)))
            .into_any_element()
    }

    fn in_meeting(&self, cx: &App) -> bool {
        let store = self.voice.read(cx);
        store
            .external_meeting()
            .is_some_and(|meeting| meeting.room_id == self.room_id)
            && store.is_active_in(&self.room_id)
    }

    fn sync_frame_pump(&mut self, cx: &mut Context<Self>) {
        let want_pump = self.in_meeting(cx) && self.voice.read(cx).has_active_video();
        if !want_pump {
            self._frame_pump = None;
            return;
        }
        if self._frame_pump.is_none() {
            self._frame_pump = Some(spawn_voice_frame_pump(cx));
        }
    }

    fn hold_to_talk(&self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_meeting(cx) {
            crate::app::root::voice_hold_to_talk(active, window, cx);
        }
    }
}

impl CallLayoutHost for MeetingView {
    fn record_voice_grid_size(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        if self.grid_size != size {
            self.grid_size = size;
            cx.notify();
        }
    }

    fn scroll_voice_grid_page(&mut self, delta: f32, total_pages: usize, cx: &mut Context<Self>) {
        let page = next_voice_grid_page(
            self.grid_page,
            &mut self.grid_wheel_accum,
            delta,
            total_pages,
        );
        if page != self.grid_page {
            self.grid_page = page;
            cx.notify();
        }
    }

    fn record_voice_strip_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if (self.strip_width - width).abs() > px(0.5) {
            self.strip_width = width;
            cx.notify();
        }
    }

    fn toggle_voice_member_strip(&mut self, cx: &mut Context<Self>) {
        self.voice
            .update(cx, |store, cx| store.toggle_member_strip(cx));
        cx.notify();
    }

    fn toggle_voice_chat(&mut self, cx: &mut Context<Self>) {
        self.chat_open = !self.chat_open;
        self.chat_focus_pending = self.chat_open;
        cx.notify();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatListChange {
    Splice {
        removed_front: usize,
        kept: usize,
        appended: usize,
    },
    Reset,
}

fn chat_list_change(
    old_first: Option<u64>,
    old_count: usize,
    new_first: Option<u64>,
    new_count: usize,
) -> ChatListChange {
    let removed_front = match (old_first, new_first) {
        (None, _) if old_count == 0 => 0,
        (Some(old), Some(new)) if new >= old => (new - old) as usize,
        _ => return ChatListChange::Reset,
    };
    let Some(kept) = old_count.checked_sub(removed_front) else {
        return ChatListChange::Reset;
    };
    match new_count.checked_sub(kept) {
        Some(appended) => ChatListChange::Splice {
            removed_front,
            kept,
            appended,
        },
        None => ChatListChange::Reset,
    }
}

fn render_chat_message(message: &MeetingChatMessage, locale: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut avatar = Avatar::new().name(message.name.clone()).size_px(px(28.));
    let avatar_url = guest_avatar_url(cx, &message.avatar, 64);
    if !avatar_url.is_empty() {
        avatar = avatar.src(avatar_url);
    }
    let time = chrono::DateTime::from_timestamp_millis(message.timestamp_ms)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default();
    div()
        .flex()
        .flex_row()
        .gap_2()
        .px_3()
        .py(px(6.))
        .child(div().flex_none().child(avatar))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if message.is_me {
                                    theme.text_link
                                } else {
                                    theme.text_primary
                                })
                                .child(message.name.clone()),
                        )
                        .child(div().text_xs().text_color(theme.text_muted).child(time)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(if message.status == MeetingChatStatus::Sending {
                            theme.text_muted
                        } else {
                            theme.text_primary
                        })
                        .whitespace_normal()
                        .child(message.content.clone()),
                )
                .when(message.status == MeetingChatStatus::Failed, |column| {
                    column.child(
                        div()
                            .text_xs()
                            .text_color(theme.danger_text)
                            .child(mezon_i18n::t(locale, "meeting.chat.failed")),
                    )
                }),
        )
        .into_any_element()
}

impl Render for MeetingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.voice
            .update(cx, |store, cx| store.flush_texture_drops(Some(window), cx));
        let theme = cx.theme().clone();
        let (locale, input_device_id, output_device_id, camera_device_id) = {
            let settings = self.settings.read(cx);
            (
                settings.language.clone(),
                settings.input_device_id.clone(),
                settings.output_device_id.clone(),
                settings.camera_device_id.clone(),
            )
        };
        let show_members = self.voice.read(cx).member_strip_visible();
        let in_meeting = self.in_meeting(cx);
        let chat_open = self.chat_open && in_meeting;
        let chat_unread = {
            let messages = self.voice.read(cx).meeting_chat();
            if chat_open {
                self.chat_read_seq = messages.last().map_or(0, |message| message.seq);
            }
            messages
                .iter()
                .filter(|message| message.seq > self.chat_read_seq && !message.is_me)
                .count()
        };
        if chat_open && self.chat_focus_pending {
            self.chat_focus_pending = false;
            self.chat_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        let host = cx.entity();
        let content = render_external_meeting(
            &locale,
            &self.code,
            &self.voice,
            &self.settings,
            input_device_id,
            output_device_id,
            camera_device_id,
            &host,
            &self.strip_scroll,
            self.strip_width,
            self.grid_page,
            self.grid_size,
            show_members,
            chat_open,
            chat_unread,
            &mut self.visual,
            window,
            cx,
        );
        let chat_panel = chat_open.then(|| self.render_chat_panel(&theme, &locale, cx));
        let fullscreen = in_meeting
            .then(|| {
                render_screen_fullscreen_overlay(
                    &theme,
                    &locale,
                    &self.voice,
                    &self.settings,
                    self.voice.read(cx),
                    None,
                    cx,
                )
            })
            .flatten();

        div()
            .id("meeting-window")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(crate::theme::ui_font_family(cx))
            .text_color(theme.text_primary)
            .bg(theme.bg_tertiary)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "space"
                    && !event.is_held
                    && !event.keystroke.modifiers.modified()
                {
                    this.hold_to_talk(true, window, cx);
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                if event.keystroke.key == "space" {
                    this.hold_to_talk(false, window, cx);
                }
            }))
            .when(window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(self.title_bar.clone())
            })
            .when(!window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(window_controls::window_drag_handle(
                    div().flex_shrink_0().h_8().w_full().bg(theme.title_bar_bg),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(content)
                            .children(fullscreen),
                    )
                    .children(chat_panel),
            )
            .when(window_controls::is_edge_resizable(), |el| {
                el.child(window_controls::render_resize_edges(window))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatListChange, chat_list_change, deep_link_meeting_code};

    #[test]
    fn appended_chat_rows_splice_in_at_the_end() {
        assert_eq!(
            chat_list_change(None, 0, Some(1), 2),
            ChatListChange::Splice {
                removed_front: 0,
                kept: 0,
                appended: 2
            }
        );
        assert_eq!(
            chat_list_change(Some(1), 2, Some(1), 3),
            ChatListChange::Splice {
                removed_front: 0,
                kept: 2,
                appended: 1
            }
        );
    }

    #[test]
    fn a_full_chat_history_drops_from_the_front_and_appends_at_the_end() {
        assert_eq!(
            chat_list_change(Some(1), 200, Some(3), 200),
            ChatListChange::Splice {
                removed_front: 2,
                kept: 198,
                appended: 2
            }
        );
    }

    #[test]
    fn a_cleared_or_unrelated_chat_history_resets_the_list() {
        assert_eq!(chat_list_change(Some(1), 3, None, 0), ChatListChange::Reset);
        assert_eq!(
            chat_list_change(Some(5), 3, Some(2), 3),
            ChatListChange::Reset
        );
        assert_eq!(
            chat_list_change(Some(1), 3, Some(9), 1),
            ChatListChange::Reset
        );
    }

    const MEETING_CODE: &str = "MTc2MDQzMjcxOTAwMDAwMDAwMDoxOTc3NTY0MTg3MzY0ODk2NzY4.c2ln";

    #[test]
    fn a_meeting_deep_link_yields_its_code() {
        assert_eq!(
            deep_link_meeting_code(&format!("mezonapp://meeting/{MEETING_CODE}")).as_deref(),
            Some(MEETING_CODE)
        );
        assert_eq!(
            deep_link_meeting_code(&format!("mezonapp://meeting/{MEETING_CODE}/?x=1")).as_deref(),
            Some(MEETING_CODE)
        );
    }

    #[test]
    fn other_deep_links_are_not_meetings() {
        for url in [
            "mezonapp://invite/123".to_string(),
            "mezonapp://meeting/abc".to_string(),
            format!("https://mezon.ai/meeting/{MEETING_CODE}"),
            format!("mezonapp://meeting/{MEETING_CODE}/extra"),
        ] {
            assert_eq!(deep_link_meeting_code(&url), None, "{url}");
        }
    }
}
