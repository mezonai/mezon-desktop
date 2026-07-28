use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, Div, Entity, Hsla, IntoElement,
    Pixels, Render, RenderOnce, SharedString, Stateful, Subscription, WeakEntity, Window, div,
    point, prelude::*, px,
};
use mezon_store::{InVoiceInfo, Settings, ThreadsStore};
use ui::{Clickable, PopoverMenu, PopoverMenuHandle, Toggleable, Tooltip};

use crate::app::window_controls;
use crate::chat::files_popover::{FilesPopoverPanel, files_popover_on_open};
use crate::chat::inbox::{InboxPopoverPanel, clan_has_inbox_badge};
use crate::chat::layout::ChatLayout;
use crate::chat::pinned_popover::{PinnedPopoverPanel, pin_popover_on_open};
use crate::chat::threads_popover::{ThreadsPopoverPanel, thread_popover_on_open};
use crate::chat::{CanvasPopoverPanel, canvas_popover_on_open};
use crate::components::primitives::{Icon, IconName, InputState};
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

pub struct ChannelHeader {
    name: String,
    dm: bool,
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
            dm: false,
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
        let icon_color = theme.text_muted;
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
        let dm_actions: &[(&str, IconName)] = &[
            ("hdr-members", IconName::MemberList),
            ("hdr-pin", IconName::PinRight),
        ];
        let actions: Vec<(&str, IconName)> = if self.dm {
            dm_actions.to_vec()
        } else {
            channel_only_actions.to_vec()
        };
        let ChannelHeader {
            name,
            dm,
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
                    .when(!dm, |this| {
                        this.child(
                            Icon::new(IconName::Hashtag)
                                .size(px(20.0))
                                .text_color(theme.text_muted),
                        )
                    })
                    .child({
                        let name_el = div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(name);
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
            dm: false,
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
            dm: false,
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
        _cx: &App,
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
                        .trigger(PinPopoverTrigger::new(theme, is_open))
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

pub struct ChatHeader {
    name: SharedString,
    dm: bool,
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
    pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
    canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
    layout: WeakEntity<ChatLayout>,
    settings: Entity<Settings>,
    _settings_observe: Subscription,
    _notification_observe: Subscription,
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
        Self {
            name: SharedString::default(),
            dm: false,
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
            pin_handle: None,
            canvas_handle: None,
            layout,
            settings: settings.clone(),
            _settings_observe,
            _notification_observe,
        }
    }

    pub fn sync(
        &mut self,
        name: Option<&str>,
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
        self.inbox_handle = inbox_handle;
        self.pin_handle = pin_handle;
        self.canvas_handle = canvas_handle;
        self.search_input = search_input;
        let show_threads = if resolving && !dm {
            self.show_threads
        } else {
            ThreadsStore::global(cx).read(cx).show_threads_popover(cx)
        };
        if self.name == name
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
        {
            return;
        }
        self.name = name;
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
        cx.notify();
    }
}

impl Render for ChatHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
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
        let mut header = ChannelHeader::new(self.name.to_string())
            .dm(self.dm)
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
            icon_color: theme.text_muted,
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
            icon_color: theme.text_muted,
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
    icon_color: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl PinPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.text_muted,
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
            .child(Icon::new(IconName::PinRight).size(px(20.)).text_color(tint));
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
            icon_color: theme.text_muted.into(),
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
            icon_idle: theme.text_muted,
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
            icon_idle: theme.text_muted,
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
            icon_idle: theme.text_muted,
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
