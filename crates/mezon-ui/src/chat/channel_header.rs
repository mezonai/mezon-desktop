use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, Div, Entity, FontWeight, Hsla,
    IntoElement, Pixels, Render, RenderOnce, SharedString, Stateful, Subscription, WeakEntity,
    Window, div, point, prelude::*, px,
};
use mezon_store::{
    CallPeer, CallStore, ChannelId, DirectKind, DirectMessageStore, DmAvatarPresence, InVoiceInfo,
    PinnedMessagesStore, Settings, StreamStore, ThreadsStore,
};
use ui::{Clickable, PopoverMenu, PopoverMenuHandle, Toggleable, Tooltip};

use crate::app::shell::Shell;
use crate::app::window_controls;
use crate::chat::edit_group_modal::EditGroupModal;
use crate::chat::files_popover::{FilesPopoverPanel, files_popover_on_open};
use crate::chat::inbox::{InboxPopoverPanel, clan_has_inbox_badge};
use crate::chat::layout::ChatLayout;
use crate::chat::pinned_popover::{PinnedPopoverPanel, pin_popover_on_open};
use crate::chat::threads_popover::{ThreadsPopoverPanel, thread_popover_on_open};
use crate::chat::{CanvasPopoverPanel, canvas_popover_on_open};
use crate::components::compositions::channel_row::{ChannelIcon, render_channel_icon};
use crate::components::primitives::{Avatar, Icon, IconName, InputState};
use crate::components::{Button, ButtonVariant, ButtonVariants, Sizable, Size};
use crate::theme::{ActiveTheme, Theme};

type ToggleHandler = Arc<dyn Fn(&mut Window, &mut App)>;
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type ThreadTriggerClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

const HEADER_POPOVER_Y_OFFSET: f32 = 4.;
const CANVAS_HEADER_BTN_H: f32 = 24.;

fn canvas_popover_y_offset() -> Pixels {
    px((window_controls::APP_HEADER_HEIGHT - CANVAS_HEADER_BTN_H) / 4.)
}

#[derive(Clone, PartialEq)]
pub struct DmHeaderInfo {
    pub channel_id: ChannelId,
    pub is_group: bool,
    /// Peer presence for a 1:1 DM; always `None` for a group.
    pub presence: DmAvatarPresence,
    pub label: SharedString,
    pub avatar_src: SharedString,
    pub avatar_raw: SharedString,
    pub members_text: Option<SharedString>,
    pub edit_tooltip: SharedString,
    pub locale: SharedString,
}

pub struct ChannelHeader {
    name: String,
    icon: Option<ChannelIcon>,
    dm: bool,
    dm_header: Option<DmHeaderInfo>,
    muted: bool,
    in_voice: Option<(SharedString, InVoiceInfo)>,
    members_action: bool,
    members_active: bool,
    on_toggle_members: Option<ToggleHandler>,
    show_inbox: bool,
    inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
    clan_id: Option<String>,
    locale: Option<String>,
    show_threads: bool,
    layout: Option<Entity<ChatLayout>>,
    thread_handle: Option<PopoverMenuHandle<ThreadsPopoverPanel>>,
    pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
    canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
    settings: Option<Entity<Settings>>,
    gallery_trigger: Option<AnyElement>,
    files_trigger: Option<AnyElement>,
    notification_trigger: Option<AnyElement>,
    search_bar: Option<AnyElement>,
    timeline_action: bool,
    timeline_active: bool,
    timeline_tooltip: SharedString,
    on_toggle_timeline: Option<ToggleHandler>,
}

impl ChannelHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            icon: None,
            dm: false,
            dm_header: None,
            muted: false,
            in_voice: None,
            members_action: true,
            members_active: false,
            on_toggle_members: None,
            show_inbox: true,
            inbox_handle: None,
            clan_id: None,
            locale: None,
            show_threads: false,
            layout: None,
            thread_handle: None,
            pin_handle: None,
            canvas_handle: None,
            settings: None,
            gallery_trigger: None,
            files_trigger: None,
            notification_trigger: None,
            search_bar: None,
            timeline_action: false,
            timeline_active: false,
            timeline_tooltip: SharedString::default(),
            on_toggle_timeline: None,
        }
    }

    pub fn dm(mut self, dm: bool) -> Self {
        self.dm = dm;
        self
    }

    pub fn dm_header(mut self, info: Option<DmHeaderInfo>) -> Self {
        self.dm_header = info;
        self
    }

    pub fn icon(mut self, icon: Option<ChannelIcon>) -> Self {
        self.icon = icon;
        self
    }

    pub fn in_voice(mut self, label: SharedString, info: InVoiceInfo) -> Self {
        self.in_voice = Some((label, info));
        self
    }

    pub fn members_action(mut self, show: bool) -> Self {
        self.members_action = show;
        self
    }

    pub fn members_active(mut self, active: bool) -> Self {
        self.members_active = active;
        self
    }

    pub fn search_bar(mut self, search_bar: AnyElement) -> Self {
        self.search_bar = Some(search_bar);
        self
    }

    pub fn on_toggle_members(mut self, handler: ToggleHandler) -> Self {
        self.on_toggle_members = Some(handler);
        self
    }

    pub fn show_inbox(mut self, show: bool) -> Self {
        self.show_inbox = show;
        self
    }

    pub fn inbox_popover(mut self, handle: PopoverMenuHandle<InboxPopoverPanel>) -> Self {
        self.inbox_handle = Some(handle);
        self
    }

    pub fn inbox_context(mut self, clan_id: impl Into<String>, locale: impl Into<String>) -> Self {
        self.clan_id = Some(clan_id.into());
        self.locale = Some(locale.into());
        self
    }

    pub fn layout(mut self, layout: Entity<ChatLayout>) -> Self {
        self.layout = Some(layout);
        self
    }

    pub fn show_threads(mut self, show: bool) -> Self {
        self.show_threads = show;
        self
    }

    pub fn thread_popover(mut self, handle: PopoverMenuHandle<ThreadsPopoverPanel>) -> Self {
        self.thread_handle = Some(handle);
        self
    }

    pub fn pin_popover(
        mut self,
        handle: PopoverMenuHandle<PinnedPopoverPanel>,
        settings: Entity<Settings>,
    ) -> Self {
        self.pin_handle = Some(handle);
        self.settings = Some(settings);
        self
    }

    pub fn canvas_popover(
        mut self,
        handle: PopoverMenuHandle<CanvasPopoverPanel>,
        settings: Entity<Settings>,
    ) -> Self {
        self.canvas_handle = Some(handle);
        self.settings = Some(settings);
        self
    }

    pub fn gallery_trigger(mut self, trigger: AnyElement) -> Self {
        self.gallery_trigger = Some(trigger);
        self
    }

    pub fn notification_trigger(mut self, trigger: Option<AnyElement>) -> Self {
        self.notification_trigger = trigger;
        self
    }

    pub fn files_trigger(mut self, trigger: AnyElement) -> Self {
        self.files_trigger = Some(trigger);
        self
    }

    pub fn timeline_action(mut self, show: bool) -> Self {
        self.timeline_action = show;
        self
    }

    pub fn timeline_active(mut self, active: bool) -> Self {
        self.timeline_active = active;
        self
    }

    pub fn timeline_tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.timeline_tooltip = tooltip.into();
        self
    }

    pub fn on_toggle_timeline(mut self, handler: ToggleHandler) -> Self {
        self.on_toggle_timeline = Some(handler);
        self
    }

    pub fn render(self, theme: &Theme, cx: &App) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        let bg_active = theme.bg_tertiary;
        let icon_color = theme.tokens.bg_icon_theme;
        let icon_active = theme.text_primary;
        let bell_icon = if self.muted {
            IconName::MuteBell
        } else {
            IconName::Bell
        };
        let channel_only_actions: &[(&str, IconName)] = &[
            ("hdr-canvas", IconName::CanvasIcon),
            ("hdr-timeline", IconName::History),
            ("hdr-thread", IconName::ThreadIcon),
            ("hdr-members", IconName::MemberList),
            ("hdr-pin", IconName::PinRight),
            ("hdr-bell", bell_icon),
            ("hdr-gallery", IconName::ImageThumbnail),
            ("hdr-files", IconName::FileIcon),
        ];
        let dm_one_to_one = self.dm_header.as_ref().is_some_and(|info| !info.is_group);
        let actions: Vec<(&str, IconName)> = if self.dm {
            let mut items: Vec<(&str, IconName)> = Vec::with_capacity(4);
            if dm_one_to_one {
                items.push(("hdr-call", IconName::IconPhoneDM));
                items.push(("hdr-video-call", IconName::IconMeetDM));
            }
            items.push(("hdr-members", IconName::MemberList));
            items.push(("hdr-pin", IconName::PinRight));
            items
        } else {
            channel_only_actions.to_vec()
        };
        let ChannelHeader {
            name,
            icon,
            dm,
            dm_header,
            muted: _,
            in_voice,
            members_action,
            members_active,
            on_toggle_members,
            show_inbox,
            inbox_handle,
            clan_id,
            locale,
            show_threads,
            layout,
            thread_handle,
            pin_handle,
            canvas_handle,
            settings,
            gallery_trigger,
            files_trigger,
            notification_trigger,
            search_bar,
            timeline_action,
            timeline_active,
            timeline_tooltip,
            on_toggle_timeline,
        } = self;
        let inbox_el = if show_inbox && !dm {
            Some(Self::render_inbox_button_for(
                theme,
                cx,
                inbox_handle,
                clan_id,
                locale,
            ))
        } else {
            None
        };
        let buttons = Self::build_action_buttons(
            actions,
            theme,
            icon_color,
            icon_active,
            bg_hover,
            bg_active,
            members_action,
            members_active,
            on_toggle_members,
            show_threads,
            thread_handle,
            layout,
            pin_handle,
            canvas_handle,
            settings,
            gallery_trigger,
            files_trigger,
            timeline_action,
            timeline_active,
            timeline_tooltip,
            on_toggle_timeline,
            notification_trigger,
            cx,
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .w_full()
            .h(px(window_controls::APP_HEADER_HEIGHT))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .when_some(icon.filter(|_| !dm), |this, icon| {
                        let glyph: Hsla = if icon.lock.is_some() {
                            theme.tokens.bg_icon_theme.into()
                        } else {
                            theme.text_muted.into()
                        };
                        this.child(render_channel_icon(
                            icon,
                            px(20.0),
                            glyph,
                            theme.tokens.bg_icon_theme_active.into(),
                        ))
                    })
                    .children(dm_header.as_ref().map(|info| {
                        let mut avatar = Avatar::new()
                            .name(info.label.clone())
                            .size_px(px(32.))
                            .group_default(info.is_group && info.avatar_raw.is_empty());
                        if !info.avatar_src.is_empty() {
                            avatar = avatar.src(info.avatar_src.clone());
                            if !info.avatar_raw.is_empty() && info.avatar_raw != info.avatar_src {
                                avatar = avatar.fallback_src(info.avatar_raw.clone());
                            }
                        } else if !info.avatar_raw.is_empty() {
                            avatar = avatar.src(info.avatar_raw.clone());
                        }
                        div()
                            .relative()
                            .flex_shrink_0()
                            .size(px(32.))
                            .child(avatar)
                            .children(crate::util::user_status::presence_badge_element(
                                info.presence,
                                theme.bg_primary,
                                theme,
                            ))
                    }))
                    .child({
                        let name_el = div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(name);
                        let group_edit = dm_header.filter(|info| info.is_group);
                        if let Some(info) = group_edit {
                            let tooltip = info.edit_tooltip.clone();
                            let members_text = info.members_text.clone();
                            div()
                                .id("hdr-dm-edit")
                                .flex()
                                .flex_col()
                                .justify_center()
                                .gap(px(1.))
                                .px_2()
                                .rounded_lg()
                                .cursor_pointer()
                                .hover(move |s| s.bg(bg_hover))
                                .tooltip(Tooltip::text(tooltip))
                                .on_click(move |_, window, cx| {
                                    let modal = cx.new(|cx| {
                                        EditGroupModal::new(
                                            info.channel_id,
                                            info.label.to_string(),
                                            info.avatar_raw.to_string(),
                                            info.locale.to_string(),
                                            window,
                                            cx,
                                        )
                                    });
                                    Shell::global(cx)
                                        .update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
                                })
                                .child(name_el.line_height(px(16.)))
                                .children(members_text.map(|text| {
                                    div()
                                        .text_xs()
                                        .line_height(px(13.))
                                        .text_color(theme.text_muted)
                                        .child(text)
                                }))
                                .into_any_element()
                        } else {
                            match in_voice {
                                Some((label, info)) => div()
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .gap(px(4.))
                                    .child(name_el.line_height(px(18.)))
                                    .child(
                                        div()
                                            .id("hdr-in-voice")
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_1()
                                            .h(px(16.))
                                            .cursor_pointer()
                                            .on_click(move |_, _, cx| {
                                                crate::router::navigate(
                                                    cx,
                                                    crate::router::Route::Channel {
                                                        clan_id: info.clan_id,
                                                        channel_id: info.channel_id,
                                                    },
                                                )
                                            })
                                            .child(
                                                Icon::new(IconName::Speaker)
                                                    .size(px(12.))
                                                    .text_color(gpui::rgb(0x22c55e)),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.text_primary)
                                                    .child(label),
                                            ),
                                    )
                                    .into_any_element(),
                                None => name_el.into_any_element(),
                            }
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .children(buttons)
                    .children(inbox_el.map(|inbox| {
                        div()
                            .flex()
                            .items_center()
                            .pl_4()
                            .border_l_1()
                            .border_color(theme.tokens.border_primary)
                            .child(inbox)
                            .into_any_element()
                    }))
                    .children(search_bar),
            )
    }

    fn render_inbox_button_for(
        theme: &Theme,
        cx: &App,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        locale: Option<String>,
    ) -> gpui::AnyElement {
        let header = ChannelHeader {
            muted: false,
            notification_trigger: None,
            name: String::new(),
            icon: None,
            dm: false,
            dm_header: None,
            in_voice: None,
            members_action: false,
            members_active: false,
            on_toggle_members: None,
            show_inbox: true,
            inbox_handle,
            clan_id,
            locale,
            show_threads: false,
            layout: None,
            thread_handle: None,
            pin_handle: None,
            canvas_handle: None,
            settings: None,
            gallery_trigger: None,
            files_trigger: None,
            search_bar: None,
            timeline_action: false,
            timeline_active: false,
            timeline_tooltip: SharedString::default(),
            on_toggle_timeline: None,
        };
        header.render_inbox_button(theme, cx)
    }

    fn build_action_buttons(
        actions: Vec<(&'static str, IconName)>,
        theme: &Theme,
        icon_color: gpui::Rgba,
        icon_active: gpui::Rgba,
        bg_hover: gpui::Rgba,
        bg_active: gpui::Rgba,
        members_action: bool,
        members_active: bool,
        on_toggle_members: Option<ToggleHandler>,
        show_threads: bool,
        thread_handle: Option<PopoverMenuHandle<ThreadsPopoverPanel>>,
        layout: Option<Entity<ChatLayout>>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        settings: Option<Entity<Settings>>,
        gallery_trigger: Option<AnyElement>,
        files_trigger: Option<AnyElement>,
        timeline_action: bool,
        timeline_active: bool,
        timeline_tooltip: SharedString,
        on_toggle_timeline: Option<ToggleHandler>,
        notification_trigger: Option<AnyElement>,
        cx: &App,
    ) -> Vec<AnyElement> {
        let header = ChannelHeader {
            muted: false,
            notification_trigger,
            name: String::new(),
            icon: None,
            dm: false,
            dm_header: None,
            in_voice: None,
            members_action,
            members_active,
            on_toggle_members,
            show_inbox: false,
            inbox_handle: None,
            clan_id: None,
            locale: None,
            show_threads,
            layout,
            thread_handle,
            pin_handle,
            canvas_handle,
            settings,
            gallery_trigger,
            files_trigger,
            search_bar: None,
            timeline_action,
            timeline_active,
            timeline_tooltip,
            on_toggle_timeline,
        };
        header.action_buttons(
            actions,
            theme,
            icon_color,
            icon_active,
            bg_hover,
            bg_active,
            cx,
        )
    }

    fn action_buttons(
        self,
        actions: Vec<(&'static str, IconName)>,
        theme: &Theme,
        icon_color: gpui::Rgba,
        icon_active: gpui::Rgba,
        bg_hover: gpui::Rgba,
        bg_active: gpui::Rgba,
        cx: &App,
    ) -> Vec<AnyElement> {
        let members_action = self.members_action;
        let members_active = self.members_active;
        let on_toggle_members = self.on_toggle_members;
        let show_threads = self.show_threads;
        let thread_handle = self.thread_handle;
        let layout = self.layout;
        let pin_handle = self.pin_handle;
        let canvas_handle = self.canvas_handle;
        let settings = self.settings;
        let mut gallery_trigger = self.gallery_trigger;
        let mut files_trigger = self.files_trigger;
        let timeline_action = self.timeline_action;
        let timeline_active = self.timeline_active;
        let timeline_tooltip = self.timeline_tooltip.clone();
        let on_toggle_timeline = self.on_toggle_timeline;
        let mut notification_trigger = self.notification_trigger;
        let mut buttons: Vec<AnyElement> = Vec::new();
        for (id, icon) in actions {
            if id == "hdr-call" || id == "hdr-video-call" {
                let video = id == "hdr-video-call";
                let tooltip = if video {
                    "Start video call"
                } else {
                    "Start voice call"
                };
                let button = div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(32.))
                    .h(px(32.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |s| s.bg(bg_hover))
                    .tooltip(Tooltip::text(tooltip))
                    .occlude()
                    .child(Icon::new(icon).size(px(20.)).text_color(icon_color))
                    .on_click(move |_, _, cx| {
                        if let Some(peer) = current_dm_call_peer(cx) {
                            CallStore::global(cx)
                                .update(cx, |store, cx| store.start_call(peer, video, cx));
                        }
                    });
                buttons.push(button.into_any_element());
                continue;
            }
            if id == "hdr-timeline" {
                if !timeline_action {
                    continue;
                }
                let active = timeline_active;
                let tint = if active { icon_active } else { icon_color };
                let tooltip = timeline_tooltip.clone();
                let mut button = div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(32.))
                    .h(px(32.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |s| s.bg(bg_hover))
                    .tooltip(Tooltip::text(tooltip))
                    .occlude()
                    .child(Icon::new(icon).size(px(20.)).text_color(tint));
                if active {
                    button = button.bg(bg_active);
                }
                if let Some(handler) = on_toggle_timeline.clone() {
                    button = button.on_click(move |_, window, cx| handler(window, cx));
                }
                buttons.push(button.into_any_element());
                continue;
            }
            if id == "hdr-members" && !members_action {
                continue;
            }
            if id == "hdr-canvas"
                && let (Some(handle), Some(settings), Some(layout)) =
                    (canvas_handle.clone(), settings.clone(), layout.clone())
            {
                let is_open = handle.is_deployed();
                let menu_handle = handle.clone();
                buttons.push(
                    PopoverMenu::new("hdr-canvas-popover")
                        .with_handle(handle)
                        .anchor(Anchor::TopRight)
                        .attach(Anchor::BottomRight)
                        .offset(point(px(0.), canvas_popover_y_offset()))
                        .on_open(canvas_popover_on_open())
                        .menu({
                            let settings = settings.clone();
                            move |window, cx| {
                                layout.update(cx, |layout, cx| {
                                    layout.ensure_canvas_search_input(window, cx);
                                });
                                let search_input = layout.read(cx).canvas_search_input.clone()?;
                                Some(cx.new(|cx| {
                                    CanvasPopoverPanel::new(
                                        settings.clone(),
                                        search_input,
                                        menu_handle.clone(),
                                        window,
                                        cx,
                                    )
                                }))
                            }
                        })
                        .trigger(CanvasPopoverTrigger::new(theme, is_open))
                        .into_any_element(),
                );
                continue;
            }
            if id == "hdr-thread" {
                if !show_threads {
                    continue;
                }
                if let (Some(handle), Some(layout)) = (thread_handle.clone(), layout.clone()) {
                    let is_open = handle.is_deployed();
                    let menu_handle = handle.clone();
                    buttons.push(
                        PopoverMenu::new("hdr-thread-popover")
                            .with_handle(handle)
                            .anchor(Anchor::TopRight)
                            .attach(Anchor::BottomRight)
                            .offset(point(px(0.), px(HEADER_POPOVER_Y_OFFSET)))
                            .on_open(thread_popover_on_open(layout.clone()))
                            .menu({
                                let layout = layout.clone();
                                move |window, cx| {
                                    layout.update(cx, |layout, cx| {
                                        layout.ensure_thread_search_input(window, cx);
                                    });
                                    let search_input =
                                        layout.read(cx).thread_search_input.clone()?;
                                    Some(cx.new(|cx| {
                                        ThreadsPopoverPanel::new(
                                            layout.clone(),
                                            search_input,
                                            menu_handle.clone(),
                                            window,
                                            cx,
                                        )
                                    }))
                                }
                            })
                            .trigger(ThreadPopoverTrigger::new(theme, is_open))
                            .into_any_element(),
                    );
                }
                continue;
            }
            if id == "hdr-pin"
                && let (Some(handle), Some(settings)) = (pin_handle.clone(), settings.clone())
            {
                let is_open = handle.is_deployed();
                let menu_handle = handle.clone();
                let show_badge = PinnedMessagesStore::global(cx)
                    .read(cx)
                    .active_has_pin_badge();
                let badge_color = theme.mention_badge;
                buttons.push(
                    PopoverMenu::new("hdr-pin-popover")
                        .with_handle(handle)
                        .anchor(Anchor::TopRight)
                        .attach(Anchor::BottomRight)
                        .offset(point(px(0.), px(HEADER_POPOVER_Y_OFFSET)))
                        .on_open(pin_popover_on_open())
                        .menu({
                            let settings = settings.clone();
                            move |window, cx| {
                                Some(cx.new(|cx| {
                                    PinnedPopoverPanel::new(
                                        settings.clone(),
                                        menu_handle.clone(),
                                        window,
                                        cx,
                                    )
                                }))
                            }
                        })
                        .trigger(PinPopoverTrigger::new(
                            theme,
                            is_open,
                            show_badge,
                            badge_color,
                        ))
                        .into_any_element(),
                );
                continue;
            }
            if id == "hdr-gallery"
                && let Some(trigger) = gallery_trigger.take()
            {
                buttons.push(trigger);
                continue;
            }
            if id == "hdr-bell" {
                if let Some(trigger) = notification_trigger.take() {
                    buttons.push(trigger);
                }
                continue;
            }
            if id == "hdr-files" {
                if let Some(trigger) = files_trigger.take() {
                    buttons.push(trigger);
                }
                continue;
            }
            let is_members = id == "hdr-members";
            let active = is_members && members_active;
            let tint = if active { icon_active } else { icon_color };
            let mut button = div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(32.))
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(bg_hover))
                .occlude()
                .child(Icon::new(icon).size(px(20.)).text_color(tint));
            if active {
                button = button.bg(bg_active);
            }
            if is_members && let Some(handler) = on_toggle_members.clone() {
                button = button.on_click(move |_, window, cx| handler(window, cx));
            }
            buttons.push(button.into_any_element());
        }
        buttons
    }

    fn render_inbox_button(&self, theme: &Theme, cx: &App) -> gpui::AnyElement {
        let Some(handle) = self.inbox_handle.clone() else {
            return div()
                .id("hdr-inbox")
                .flex()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(32.))
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(
                    Icon::new(IconName::Inbox)
                        .size(px(20.))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        };

        let show_badge = self
            .clan_id
            .as_deref()
            .is_some_and(|id| clan_has_inbox_badge(id, cx));
        let clan_id = self.clan_id.clone().unwrap_or_default();
        let locale = self.locale.clone().unwrap_or_else(|| "en".to_string());
        let badge_color = theme.mention_badge;
        let is_open = handle.is_deployed();

        PopoverMenu::new("hdr-inbox-popover")
            .with_handle(handle.clone())
            .anchor(Anchor::TopRight)
            .attach(Anchor::BottomRight)
            .offset(point(px(0.), px(HEADER_POPOVER_Y_OFFSET)))
            .menu({
                let handle = handle.clone();
                let clan_id = clan_id.clone();
                let locale = locale.clone();
                move |window, cx| {
                    Some(cx.new(|cx| {
                        InboxPopoverPanel::new(
                            clan_id.clone(),
                            locale.clone(),
                            handle.clone(),
                            window,
                            cx,
                        )
                    }))
                }
            })
            .trigger(InboxPopoverTrigger::new(
                theme,
                is_open,
                show_badge,
                badge_color,
            ))
            .into_any_element()
    }
}

/// The DM peer's badge, matching the sidebar row: the live presence, with the
/// DM list's `online` flag only bootstrapping it until presence is known.
/// Groups never carry one.
fn dm_peer_presence(dm: &mezon_store::DirectChannel, cx: &App) -> DmAvatarPresence {
    dm.peer_user_id
        .filter(|_| dm.kind != DirectKind::Group)
        .zip(mezon_store::PresenceStore::try_global(cx))
        .map(|(user_id, presence)| presence.read(cx).dm_avatar_presence(user_id, dm.online))
        .unwrap_or(DmAvatarPresence::None)
}

pub struct ChatHeader {
    name: SharedString,
    icon: Option<ChannelIcon>,
    dm: bool,
    /// Cached so the header does not re-derive it (and re-allocate the proxied
    /// avatar url and member-count string) on every repaint.
    dm_header: Option<DmHeaderInfo>,
    in_voice: Option<InVoiceInfo>,
    members_action: bool,
    members_active: bool,
    show_search_bar: bool,
    search_expanded: bool,
    show_search_options: bool,
    search_input: Option<Entity<InputState>>,
    show_inbox: bool,
    inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
    clan_id: Option<String>,
    locale: Option<SharedString>,
    show_threads: bool,
    timeline_action: bool,
    timeline_active: bool,
    stream_sidebar: bool,
    pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
    canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
    layout: WeakEntity<ChatLayout>,
    settings: Entity<Settings>,
    _settings_observe: Subscription,
    _notification_observe: Subscription,
    _pinned_observe: Subscription,
    _direct_observe: Subscription,
    _group_members_observe: Subscription,
    _presence_subscribe: Subscription,
}

fn current_dm_call_peer(cx: &App) -> Option<CallPeer> {
    let crate::router::Route::DirectMessage { direct_id, .. } =
        crate::router::Router::global(cx).read(cx).route()
    else {
        return None;
    };
    let store = DirectMessageStore::try_global(cx)?;
    let store = store.read(cx);
    let dm = store.find(direct_id)?;
    if dm.kind != DirectKind::Dm {
        return None;
    }
    let peer_user_id = dm.peer_user_id?;
    Some(CallPeer {
        user_id: peer_user_id.get(),
        channel_id: dm.id.get(),
        name: dm.label.clone(),
        avatar: (!dm.avatar.is_empty()).then(|| dm.avatar.clone()),
    })
}

impl ChatHeader {
    pub fn new(
        layout: WeakEntity<ChatLayout>,
        settings: &Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _settings_observe = cx.observe(settings, |_, _, cx| cx.notify());
        let _notification_observe = cx.observe(
            &mezon_store::NotificationSettingStore::global(cx),
            |_, _, cx| cx.notify(),
        );
        let _pinned_observe = cx.observe(&PinnedMessagesStore::global(cx), |_, _, cx| cx.notify());
        // The DM store carries the group's label and avatar; the layout's own
        // change gate only tracks the label, so an avatar-only edit reaches the
        // header through here. Both refresh paths repaint only on a real change.
        let _direct_observe = cx.observe(&DirectMessageStore::global(cx), |this, _, cx| {
            this.refresh_dm_header(cx)
        });
        let _group_members_observe = cx.observe(
            &mezon_store::GroupMembersStore::global(cx),
            |this, _, cx| this.refresh_dm_header(cx),
        );
        // Only the status event: presence also notifies on every typing tick.
        let _presence_subscribe = cx.subscribe(
            &mezon_store::PresenceStore::global(cx),
            |this, _, event, cx| {
                if matches!(event, mezon_store::PresenceEvent::StatusChanged) {
                    this.refresh_dm_presence(cx);
                }
            },
        );
        Self {
            name: SharedString::default(),
            icon: None,
            dm: false,
            dm_header: None,
            in_voice: None,
            members_action: true,
            members_active: false,
            show_search_bar: false,
            search_expanded: false,
            show_search_options: false,
            search_input: None,
            show_inbox: true,
            inbox_handle: None,
            clan_id: None,
            locale: None,
            show_threads: false,
            timeline_action: false,
            timeline_active: false,
            stream_sidebar: false,
            pin_handle: None,
            canvas_handle: None,
            layout,
            settings: settings.clone(),
            _settings_observe,
            _notification_observe,
            _pinned_observe,
            _direct_observe,
            _group_members_observe,
            _presence_subscribe,
        }
    }

    /// Derives the DM avatar / name / member-count block from the DM and
    /// group-member stores. Kept out of `render` because it allocates.
    fn compute_dm_header(dm: bool, locale: &str, cx: &App) -> Option<DmHeaderInfo> {
        if !dm {
            return None;
        }
        let crate::router::Route::DirectMessage { direct_id, .. } =
            crate::router::Router::global(cx).read(cx).route()
        else {
            return None;
        };
        let store = DirectMessageStore::try_global(cx)?;
        let dm = store.read(cx).find(direct_id)?;
        let is_group = dm.kind == DirectKind::Group;
        let presence = dm_peer_presence(dm, cx);
        let avatar_src = if dm.avatar.is_empty() {
            String::new()
        } else {
            crate::util::imgproxy::avatar_url(cx, &dm.avatar)
        };
        let members_text = is_group.then(|| {
            let count = mezon_store::GroupMembersStore::try_global(cx)
                .map(|gm| gm.read(cx).members(dm.id).len())
                .unwrap_or(0);
            let key = if count == 1 {
                "common.member"
            } else {
                "common.members"
            };
            SharedString::from(format!(
                "{} {}",
                count,
                mezon_i18n::t(locale, key).to_lowercase()
            ))
        });
        Some(DmHeaderInfo {
            channel_id: dm.id,
            is_group,
            presence,
            label: SharedString::from(dm.label.clone()),
            avatar_src: SharedString::from(avatar_src),
            avatar_raw: SharedString::from(dm.avatar.clone()),
            members_text,
            edit_tooltip: SharedString::from(
                mezon_i18n::t(locale, "channelTopbar.tooltips.clickToEdit").to_string(),
            ),
            locale: SharedString::from(locale.to_string()),
        })
    }

    /// Allocation-free check that the cached block still describes the conversation
    /// the router is on, so `sync` can reuse it instead of rebuilding.
    fn dm_header_matches(&self, locale: Option<&str>, cx: &App) -> bool {
        let route_id = match crate::router::Router::global(cx).read(cx).route() {
            crate::router::Route::DirectMessage { direct_id, .. } => Some(direct_id),
            _ => None,
        };
        self.dm_header.as_ref().map(|info| info.channel_id) == route_id
            && self.locale.as_deref() == locale
    }

    /// Presence-only refresh. The status tick fires for every peer the app
    /// tracks, so it patches the cached block's badge in place instead of
    /// rebuilding it -- a hash lookup rather than a fresh set of strings.
    fn refresh_dm_presence(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self
            .dm_header
            .as_ref()
            .filter(|info| !info.is_group)
            .map(|info| info.channel_id)
        else {
            return;
        };
        let next = DirectMessageStore::try_global(cx)
            .and_then(|store| {
                let store = store.read(cx);
                let dm = store.find(channel_id)?;
                Some(dm_peer_presence(dm, cx))
            })
            .unwrap_or(DmAvatarPresence::None);
        if let Some(info) = self.dm_header.as_mut()
            && info.presence != next
        {
            info.presence = next;
            cx.notify();
        }
    }

    fn refresh_dm_header(&mut self, cx: &mut Context<Self>) {
        let locale = self
            .locale
            .clone()
            .unwrap_or_else(|| SharedString::from("en"));
        let next = Self::compute_dm_header(self.dm, &locale, cx);
        if self.dm_header != next {
            self.dm_header = next;
            cx.notify();
        }
    }

    pub fn sync(
        &mut self,
        name: Option<&str>,
        icon: Option<ChannelIcon>,
        dm: bool,
        in_voice: Option<InVoiceInfo>,
        members_action: bool,
        members_active: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        timeline_action: bool,
        timeline_active: bool,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        stream_sidebar: bool,
        locale: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let resolving = name.is_none();
        let name = match name {
            Some(name) if self.name.as_ref() == name => self.name.clone(),
            Some(name) => SharedString::from(name.to_string()),
            None if dm => SharedString::default(),
            None => self.name.clone(),
        };
        let icon = match icon {
            Some(icon) => Some(icon),
            None if dm => None,
            None if resolving => self.icon,
            None => None,
        };
        self.inbox_handle = inbox_handle;
        self.pin_handle = pin_handle;
        self.canvas_handle = canvas_handle;
        self.search_input = search_input;
        let show_threads = if resolving && !dm {
            self.show_threads
        } else {
            ThreadsStore::global(cx).read(cx).show_threads_popover(cx)
        };
        // `sync` runs from the layout's render path, so only rebuild the DM block
        // when the conversation or locale actually moved -- content edits arrive
        // through `_direct_observe` / `_group_members_observe` instead.
        let dm_header = if dm && self.dm_header_matches(locale, cx) {
            self.dm_header.clone()
        } else {
            Self::compute_dm_header(dm, locale.unwrap_or("en"), cx)
        };
        if self.name == name
            && self.dm_header == dm_header
            && self.icon == icon
            && self.dm == dm
            && self.in_voice == in_voice
            && self.members_action == members_action
            && self.members_active == members_active
            && self.show_search_bar == show_search_bar
            && self.search_expanded == search_expanded
            && self.show_search_options == show_search_options
            && self.show_inbox == show_inbox
            && self.clan_id == clan_id
            && self.locale.as_deref() == locale
            && self.show_threads == show_threads
            && self.timeline_action == timeline_action
            && self.timeline_active == timeline_active
            && self.stream_sidebar == stream_sidebar
        {
            return;
        }
        self.name = name;
        self.dm_header = dm_header;
        self.icon = icon;
        self.dm = dm;
        self.in_voice = in_voice;
        self.members_action = members_action;
        self.members_active = members_active;
        self.show_search_bar = show_search_bar;
        self.search_expanded = search_expanded;
        self.show_search_options = show_search_options;
        self.show_inbox = show_inbox;
        self.clan_id = clan_id;
        self.locale = locale.map(|locale| SharedString::from(locale.to_string()));
        self.show_threads = show_threads;
        self.timeline_action = timeline_action;
        self.timeline_active = timeline_active;
        self.stream_sidebar = stream_sidebar;
        cx.notify();
    }
}

impl Render for ChatHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.stream_sidebar {
            return render_stream_chat_sidebar_header(&theme, &self.name, self.layout.clone(), cx)
                .into_any_element();
        }
        let layout_weak = self.layout.clone();
        let settings = self.settings.clone();
        let show_threads = self.show_threads;
        let show_search_bar = self.show_search_bar;
        let search_expanded = self.search_expanded;
        let show_search_options = self.show_search_options;
        let search_input = self.search_input.clone();
        let locale = self
            .locale
            .clone()
            .unwrap_or_else(|| SharedString::from("en"));

        let muted = crate::chat::files_popover::active_files_channel(cx)
            .map(|(clan_id, channel_id)| {
                mezon_store::NotificationSettingStore::global(cx)
                    .read(cx)
                    .is_muted(channel_id, clan_id, cx)
            })
            .unwrap_or(false);
        let notification_trigger = if self.dm {
            None
        } else {
            Some(
                PopoverMenu::new("hdr-bell-popover")
                    .anchor(Anchor::TopRight)
                    .attach(Anchor::BottomRight)
                    .offset(point(px(0.), px(HEADER_POPOVER_Y_OFFSET)))
                    .trigger(NotificationSettingTrigger::new(&theme, muted))
                    .menu({
                        let settings = settings.clone();
                        move |window, cx| {
                            let (clan_id, channel_id) =
                                crate::chat::files_popover::active_files_channel(cx)?;
                            Some(cx.new(|cx| {
                                crate::chat::notification_setting_popover::NotificationSettingPanel::new(
                                    clan_id,
                                    channel_id,
                                    settings.clone(),
                                    window,
                                    cx,
                                )
                            }))
                        }
                    })
                    .into_any_element(),
            )
        };

        let gallery_trigger = PopoverMenu::new("hdr-gallery-popover")
            .anchor(Anchor::TopRight)
            .attach(Anchor::BottomRight)
            .offset(point(px(0.), px(HEADER_POPOVER_Y_OFFSET)))
            .trigger(GalleryTrigger::new(&theme))
            .menu({
                let settings = settings.clone();
                move |window, cx| build_gallery_modal(settings.clone(), window, cx)
            })
            .into_any_element();

        let files_trigger = if !self.dm {
            Some(
                PopoverMenu::new("hdr-files-popover")
                    .anchor(Anchor::TopRight)
                    .attach(Anchor::BottomRight)
                    .offset(point(px(0.), px(HEADER_POPOVER_Y_OFFSET)))
                    .on_open(files_popover_on_open())
                    .menu({
                        let settings = settings.clone();
                        move |window, cx| {
                            let (clan_id, channel_id) =
                                crate::chat::files_popover::active_files_channel(cx)?;
                            Some(cx.new(|cx| {
                                FilesPopoverPanel::new(
                                    settings.clone(),
                                    clan_id,
                                    channel_id,
                                    PopoverMenuHandle::default(),
                                    window,
                                    cx,
                                )
                            }))
                        }
                    })
                    .trigger(FilesPopoverTrigger::new(&theme))
                    .into_any_element(),
            )
        } else {
            None
        };

        let members_toggle = Arc::new(move |_window: &mut Window, cx: &mut App| {
            let _ = layout_weak.update(cx, |this, cx| this.toggle_member_list(cx));
        });
        let layout_weak_timeline = self.layout.clone();
        let timeline_toggle = Arc::new(move |_window: &mut Window, cx: &mut App| {
            let _ = layout_weak_timeline.update(cx, |this, cx| this.toggle_media_channel_view(cx));
        });
        let timeline_tooltip: SharedString = if self.timeline_active {
            mezon_i18n::t(&locale, "channelTopbar.tooltips.defaultView").into()
        } else {
            mezon_i18n::t(&locale, "channelTopbar.tooltips.timelineView").into()
        };
        let dm_header = self.dm_header.clone();
        let mut header = ChannelHeader::new(self.name.to_string())
            .icon(self.icon)
            .dm(self.dm)
            .dm_header(dm_header)
            .members_action(self.members_action)
            .members_active(self.members_active)
            .gallery_trigger(gallery_trigger)
            .notification_trigger(notification_trigger)
            .show_inbox(self.show_inbox)
            .on_toggle_members(members_toggle)
            .show_threads(show_threads);
        if self.timeline_action {
            header = header
                .timeline_action(true)
                .timeline_active(self.timeline_active)
                .timeline_tooltip(timeline_tooltip)
                .on_toggle_timeline(timeline_toggle);
        }
        if let Some(files_trigger) = files_trigger {
            header = header.files_trigger(files_trigger);
        }
        if self.dm
            && let Some(info) = self.in_voice
        {
            let label: SharedString = mezon_i18n::t(&locale, "channelTopbar.invoice").into();
            header = header.in_voice(label, info);
        }
        if show_search_bar {
            let search_bar = crate::chat::message_search::render_header_search_bar(
                &theme,
                &locale,
                search_input.as_ref(),
                search_expanded,
                show_search_options,
                self.layout.clone(),
                cx,
            );
            header = header.search_bar(search_bar);
        }
        if show_threads
            && let Ok(thread_handle) = self
                .layout
                .read_with(cx, |layout, _| layout.thread_popover_handle.clone())
            && let Some(layout) = self.layout.upgrade()
        {
            header = header.layout(layout).thread_popover(thread_handle);
        }
        if let (Some(handle), Some(clan_id), Some(locale)) = (
            self.inbox_handle.clone(),
            self.clan_id.clone(),
            self.locale.clone(),
        ) {
            header = header
                .inbox_popover(handle)
                .inbox_context(clan_id, locale.to_string());
        }
        if let Some(handle) = self.pin_handle.clone() {
            header = header.pin_popover(handle, settings.clone());
        }
        if let Some(handle) = self.canvas_handle.clone() {
            header = header.canvas_popover(handle, settings);
        }
        header.render(&theme, cx).into_any_element()
    }
}

fn render_stream_chat_sidebar_header(
    theme: &Theme,
    name: &SharedString,
    layout: WeakEntity<ChatLayout>,
    _cx: &App,
) -> impl IntoElement {
    let label = crate::chat::stream::truncate_label(name, 30);
    let hover = theme.bg_hover;

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .px_4()
        .h(px(50.))
        .min_h(px(50.))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.bg_primary)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(crate::chat::voice::chat_toggle_icon(
                    theme.text_primary,
                    px(20.),
                ))
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(label),
                ),
        )
        .child(
            div()
                .id("stream-chat-close")
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(theme.text_primary)
                .hover(move |s| s.bg(hover))
                .rounded_md()
                .p_1()
                .child(
                    Icon::new(IconName::Close)
                        .size(px(20.))
                        .text_color(theme.text_primary),
                )
                .on_click(move |_, _, cx| {
                    StreamStore::global(cx).update(cx, |store, cx| {
                        if store.show_chat() {
                            store.toggle_chat(cx);
                        }
                    });
                    let _ = layout.update(cx, |layout, cx| {
                        if layout.voice_show_chat() {
                            layout.toggle_voice_chat(cx);
                        }
                    });
                }),
        )
}

#[derive(IntoElement)]
struct ThreadPopoverTrigger {
    open: bool,
    icon_color: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    on_click: Option<ThreadTriggerClickHandler>,
}

impl ThreadPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.tokens.bg_icon_theme,
            icon_active: theme.text_primary,
            bg_hover: theme.bg_hover,
            bg_active: theme.bg_tertiary,
            on_click: None,
        }
    }
}

impl Toggleable for ThreadPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for ThreadPopoverTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for ThreadPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let tint = if self.open {
            self.icon_active
        } else {
            self.icon_color
        };
        let bg_hover = self.bg_hover;
        let mut button = div()
            .id("hdr-thread-trigger")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(
                Icon::new(IconName::ThreadIcon)
                    .size(px(20.))
                    .text_color(tint),
            );
        if self.open {
            button = button.bg(self.bg_active);
        }
        if let Some(handler) = self.on_click {
            button.on_click(handler)
        } else {
            button
        }
    }
}

#[derive(IntoElement)]
struct InboxPopoverTrigger {
    open: bool,
    show_badge: bool,
    badge_color: gpui::Rgba,
    icon_color: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl InboxPopoverTrigger {
    fn new(theme: &Theme, open: bool, show_badge: bool, badge_color: gpui::Rgba) -> Self {
        Self {
            open,
            show_badge,
            badge_color,
            icon_color: theme.tokens.bg_icon_theme,
            icon_active: theme.interactive_active,
            bg_hover: theme.bg_hover,
            on_click: None,
        }
    }
}

impl Toggleable for InboxPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for InboxPopoverTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for InboxPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let tint = if self.open {
            self.icon_active
        } else {
            self.icon_color
        };
        let bg_hover = self.bg_hover;
        let mut button = div()
            .id("hdr-inbox-trigger")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(
                div()
                    .relative()
                    .child(Icon::new(IconName::Inbox).size(px(20.)).text_color(tint))
                    .when(self.show_badge, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .right(px(0.))
                                .w(px(8.))
                                .h(px(8.))
                                .rounded_full()
                                .bg(self.badge_color),
                        )
                    }),
            );
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        button
    }
}

#[derive(IntoElement)]
struct PinPopoverTrigger {
    open: bool,
    show_badge: bool,
    badge_color: gpui::Rgba,
    icon_color: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl PinPopoverTrigger {
    fn new(theme: &Theme, open: bool, show_badge: bool, badge_color: gpui::Rgba) -> Self {
        Self {
            open,
            show_badge,
            badge_color,
            icon_color: theme.tokens.bg_icon_theme,
            icon_active: theme.text_primary,
            bg_hover: theme.bg_hover,
            bg_active: theme.bg_tertiary,
            on_click: None,
        }
    }
}

impl Toggleable for PinPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for PinPopoverTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for PinPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let tint = if self.open {
            self.icon_active
        } else {
            self.icon_color
        };
        let bg_hover = self.bg_hover;
        let mut button = div()
            .id("hdr-pin-trigger")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(
                div()
                    .relative()
                    .child(Icon::new(IconName::PinRight).size(px(20.)).text_color(tint))
                    .when(self.show_badge, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(0.))
                                .right(px(0.))
                                .w(px(8.))
                                .h(px(8.))
                                .rounded_full()
                                .bg(self.badge_color),
                        )
                    }),
            );
        if self.open {
            button = button.bg(self.bg_active);
        }
        if let Some(handler) = self.on_click {
            button.on_click(handler)
        } else {
            button
        }
    }
}

#[derive(IntoElement)]
struct CanvasPopoverTrigger {
    open: bool,
    icon_color: Hsla,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl CanvasPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.tokens.bg_icon_theme.into(),
            on_click: None,
        }
    }
}

impl Toggleable for CanvasPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for CanvasPopoverTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for CanvasPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut button = Button::new("hdr-canvas-trigger")
            .with_size(Size::Small)
            .icon(
                Icon::new(IconName::CanvasIcon)
                    .size(px(20.))
                    .text_color(self.icon_color),
            );
        button = if self.open {
            button.with_variant(ButtonVariant::Secondary)
        } else {
            button.ghost()
        };
        if let Some(handler) = self.on_click {
            button.on_click(handler)
        } else {
            button
        }
    }
}

fn build_gallery_modal(
    settings: Entity<Settings>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<crate::gallery::GalleryModal>> {
    use crate::router::{Route, Router};
    use mezon_store::ClanId;

    let (clan_id, channel_id) = match Router::global(cx).read(cx).route() {
        Route::Channel {
            clan_id,
            channel_id,
        }
        | Route::Thread {
            clan_id,
            channel_id,
            ..
        }
        | Route::Canvas {
            clan_id,
            channel_id,
            ..
        } => (clan_id, channel_id),
        Route::DirectMessage { direct_id, .. } => (ClanId(0), direct_id),
        _ => return None,
    };
    Some(cx.new(|cx| {
        crate::gallery::GalleryModal::new(
            clan_id,
            channel_id,
            SharedString::default(),
            settings,
            window,
            cx,
        )
    }))
}

struct GalleryTrigger {
    icon_idle: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    selected: bool,
    on_click: Option<ClickHandler>,
    cursor: Option<CursorStyle>,
}

impl GalleryTrigger {
    fn new(theme: &Theme) -> Self {
        Self {
            icon_idle: theme.tokens.bg_icon_theme,
            icon_active: theme.text_primary,
            bg_hover: theme.bg_hover,
            bg_active: theme.bg_tertiary,
            selected: false,
            on_click: None,
            cursor: None,
        }
    }
}

impl Clickable for GalleryTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(mut self, cursor_style: CursorStyle) -> Self {
        self.cursor = Some(cursor_style);
        self
    }
}

impl Toggleable for GalleryTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl IntoElement for GalleryTrigger {
    type Element = Stateful<Div>;

    fn into_element(self) -> Self::Element {
        let bg_hover = self.bg_hover;
        let tint = if self.selected {
            self.icon_active
        } else {
            self.icon_idle
        };
        let mut button = div()
            .id("hdr-gallery")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(
                Icon::new(IconName::ImageThumbnail)
                    .size(px(20.))
                    .text_color(tint),
            );
        if self.selected {
            button = button.bg(self.bg_active);
        }
        if let Some(cursor) = self.cursor {
            button = button.cursor(cursor);
        }
        if let Some(handler) = self.on_click {
            button = button.on_click(handler);
        }
        button
    }
}

struct FilesPopoverTrigger {
    icon_idle: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    selected: bool,
    on_click: Option<ClickHandler>,
    cursor: Option<CursorStyle>,
}

impl FilesPopoverTrigger {
    fn new(theme: &Theme) -> Self {
        Self {
            icon_idle: theme.tokens.bg_icon_theme,
            icon_active: theme.text_primary,
            bg_hover: theme.bg_hover,
            bg_active: theme.bg_tertiary,
            selected: false,
            on_click: None,
            cursor: None,
        }
    }
}

impl Clickable for FilesPopoverTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(mut self, cursor_style: CursorStyle) -> Self {
        self.cursor = Some(cursor_style);
        self
    }
}

impl Toggleable for FilesPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl IntoElement for FilesPopoverTrigger {
    type Element = Stateful<Div>;

    fn into_element(self) -> Self::Element {
        let bg_hover = self.bg_hover;
        let tint = if self.selected {
            self.icon_active
        } else {
            self.icon_idle
        };
        let mut button = div()
            .id("hdr-files")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(Icon::new(IconName::FileIcon).size(px(20.)).text_color(tint));
        if self.selected {
            button = button.bg(self.bg_active);
        }
        if let Some(cursor) = self.cursor {
            button = button.cursor(cursor);
        }
        if let Some(handler) = self.on_click {
            button = button.on_click(handler);
        }
        button
    }
}

pub struct NotificationSettingTrigger {
    icon: IconName,
    icon_idle: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    selected: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    cursor: Option<CursorStyle>,
}

impl NotificationSettingTrigger {
    fn new(theme: &Theme, muted: bool) -> Self {
        Self {
            icon: if muted {
                IconName::MuteBell
            } else {
                IconName::Bell
            },
            icon_idle: theme.tokens.bg_icon_theme,
            icon_active: theme.text_primary,
            bg_hover: theme.bg_hover,
            bg_active: theme.bg_tertiary,
            selected: false,
            on_click: None,
            cursor: None,
        }
    }
}

impl Clickable for NotificationSettingTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(mut self, cursor_style: CursorStyle) -> Self {
        self.cursor = Some(cursor_style);
        self
    }
}

impl Toggleable for NotificationSettingTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl IntoElement for NotificationSettingTrigger {
    type Element = Stateful<Div>;

    fn into_element(self) -> Self::Element {
        let bg_hover = self.bg_hover;
        let tint = if self.selected {
            self.icon_active
        } else {
            self.icon_idle
        };
        let mut button = div()
            .id("hdr-bell")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(Icon::new(self.icon).size(px(20.)).text_color(tint));
        if self.selected {
            button = button.bg(self.bg_active);
        }
        if let Some(cursor) = self.cursor {
            button = button.cursor(cursor);
        }
        if let Some(handler) = self.on_click {
            button = button.on_click(handler);
        }
        button
    }
}
