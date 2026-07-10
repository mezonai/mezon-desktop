use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, Div, Entity, Hsla, IntoElement,
    Render, RenderOnce, SharedString, Stateful, Subscription, WeakEntity, Window, div, point,
    prelude::*, px,
};
use mezon_store::{Settings, ThreadsStore};
use ui::{ButtonLike, Clickable, PopoverMenu, PopoverMenuHandle, Toggleable};

use crate::app::window_controls;
use crate::chat::inbox::{InboxPopoverPanel, clan_has_inbox_badge};
use crate::chat::layout::ChatLayout;
use crate::chat::pinned_popover::{PinnedPopoverPanel, pin_popover_on_open};
use crate::chat::threads_popover::{ThreadsPopoverPanel, thread_popover_on_open};
use crate::components::primitives::{
    Button, ButtonVariant, ButtonVariants, Icon, IconName, InputState, Sizable, Size,
};
use crate::theme::{ActiveTheme, Theme};

type ToggleHandler = Arc<dyn Fn(&mut Window, &mut App)>;
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type ThreadTriggerClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

pub struct ChannelHeader {
    name: String,
    dm: bool,
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
    settings: Option<Entity<Settings>>,
    gallery_trigger: Option<AnyElement>,
    search_bar: Option<AnyElement>,
}

impl ChannelHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dm: false,
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
            settings: None,
            gallery_trigger: None,
            search_bar: None,
        }
    }

    pub fn dm(mut self, dm: bool) -> Self {
        self.dm = dm;
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

    pub fn gallery_trigger(mut self, trigger: AnyElement) -> Self {
        self.gallery_trigger = Some(trigger);
        self
    }

    pub fn render(self, theme: &Theme, cx: &App) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        let bg_active = theme.bg_tertiary;
        let icon_color = theme.text_muted;
        let icon_active = theme.text_primary;
        let actions = [
            ("hdr-canvas", IconName::CanvasIcon),
            ("hdr-timeline", IconName::History),
            ("hdr-thread", IconName::ThreadIcon),
            ("hdr-members", IconName::MemberList),
            ("hdr-pin", IconName::PinRight),
            ("hdr-bell", IconName::Bell),
            ("hdr-gallery", IconName::ImageThumbnail),
            ("hdr-files", IconName::FileIcon),
        ];
        let ChannelHeader {
            name,
            dm,
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
            settings,
            gallery_trigger,
            search_bar,
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
            settings,
            gallery_trigger,
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
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(name),
                    ),
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
                            .border_color(theme.tokens.border_theme_primary)
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
            name: String::new(),
            dm: false,
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
            settings: None,
            gallery_trigger: None,
            search_bar: None,
        };
        header.render_inbox_button(theme, cx)
    }

    fn build_action_buttons(
        actions: [(&'static str, IconName); 8],
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
        settings: Option<Entity<Settings>>,
        gallery_trigger: Option<AnyElement>,
        cx: &App,
    ) -> Vec<AnyElement> {
        let header = ChannelHeader {
            name: String::new(),
            dm: false,
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
            settings,
            gallery_trigger,
            search_bar: None,
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
        actions: [(&'static str, IconName); 8],
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
        let settings = self.settings;
        let mut gallery_trigger = self.gallery_trigger;
        let mut buttons: Vec<AnyElement> = Vec::new();
        for (id, icon) in actions {
            if id == "hdr-members" && !members_action {
                continue;
            }
            if id == "hdr-thread" {
                if !show_threads {
                    continue;
                }
                if let (Some(handle), Some(layout)) = (thread_handle.clone(), layout.clone()) {
                    let menu_handle = handle.clone();
                    buttons.push(
                        PopoverMenu::new("hdr-thread-popover")
                            .with_handle(handle)
                            .anchor(Anchor::TopRight)
                            .attach(Anchor::BottomRight)
                            .offset(point(px(0.), px(9.)))
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
                            .trigger(ThreadPopoverTrigger::new(theme, false))
                            .into_any_element(),
                    );
                }
                continue;
            }
            if id == "hdr-pin"
                && let (Some(handle), Some(settings)) = (pin_handle.clone(), settings.clone())
            {
                let menu_handle = handle.clone();
                buttons.push(
                    PopoverMenu::new("hdr-pin-popover")
                        .with_handle(handle)
                        .anchor(Anchor::TopRight)
                        .attach(Anchor::BottomRight)
                        .offset(point(px(0.), px(9.)))
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
                        .trigger(PinPopoverTrigger::new(theme, false))
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
        let mention_badge = theme.mention_badge;
        let is_open = handle.is_deployed();

        PopoverMenu::new("hdr-inbox-popover")
            .with_handle(handle.clone())
            .anchor(Anchor::TopRight)
            .attach(Anchor::BottomRight)
            .offset(gpui::point(px(0.), px(8.)))
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
            .trigger(
                ButtonLike::new("hdr-inbox-btn")
                    .toggle_state(is_open)
                    .child(
                        div()
                            .relative()
                            .child(Icon::new(IconName::Inbox).size(px(20.)).text_color(
                                if is_open {
                                    theme.interactive_active
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .when(show_badge, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(0.))
                                        .right(px(0.))
                                        .w(px(8.))
                                        .h(px(8.))
                                        .rounded_full()
                                        .bg(mention_badge),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}

pub struct ChatHeader {
    name: SharedString,
    dm: bool,
    members_action: bool,
    members_active: bool,
    show_search_bar: bool,
    search_expanded: bool,
    show_search_options: bool,
    search_input: Option<Entity<InputState>>,
    show_inbox: bool,
    inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
    clan_id: Option<String>,
    locale: Option<String>,
    show_threads: bool,
    pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
    layout: WeakEntity<ChatLayout>,
    settings: Entity<Settings>,
    _settings_observe: Subscription,
}

impl ChatHeader {
    pub fn new(
        layout: WeakEntity<ChatLayout>,
        settings: &Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _settings_observe = cx.observe(settings, |_, _, cx| cx.notify());
        Self {
            name: SharedString::default(),
            dm: false,
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
            pin_handle: None,
            layout,
            settings: settings.clone(),
            _settings_observe,
        }
    }

    pub fn sync(
        &mut self,
        name: Option<SharedString>,
        dm: bool,
        members_action: bool,
        members_active: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        locale: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let resolving = name.is_none();
        let name = match name {
            Some(name) => name,
            None if dm => SharedString::default(),
            None => self.name.clone(),
        };
        self.inbox_handle = inbox_handle;
        self.pin_handle = pin_handle;
        self.search_input = search_input;
        let show_threads = if resolving && !dm {
            self.show_threads
        } else {
            ThreadsStore::global(cx).read(cx).show_threads_popover(cx)
        };
        if self.name == name
            && self.dm == dm
            && self.members_action == members_action
            && self.members_active == members_active
            && self.show_search_bar == show_search_bar
            && self.search_expanded == search_expanded
            && self.show_search_options == show_search_options
            && self.show_inbox == show_inbox
            && self.clan_id == clan_id
            && self.locale == locale
            && self.show_threads == show_threads
        {
            return;
        }
        self.name = name;
        self.dm = dm;
        self.members_action = members_action;
        self.members_active = members_active;
        self.show_search_bar = show_search_bar;
        self.search_expanded = search_expanded;
        self.show_search_options = show_search_options;
        self.show_inbox = show_inbox;
        self.clan_id = clan_id;
        self.locale = locale;
        self.show_threads = show_threads;
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
        let locale = self.locale.clone().unwrap_or_else(|| "en".to_string());

        let gallery_trigger = PopoverMenu::new("hdr-gallery-popover")
            .anchor(Anchor::TopRight)
            .attach(Anchor::BottomRight)
            .offset(point(px(0.), px(4.)))
            .trigger(GalleryTrigger::new(&theme))
            .menu({
                let settings = settings.clone();
                move |window, cx| build_gallery_modal(settings.clone(), window, cx)
            })
            .into_any_element();

        let members_toggle = Arc::new(move |_window: &mut Window, cx: &mut App| {
            let _ = layout_weak.update(cx, |this, cx| this.toggle_member_list(cx));
        });
        let mut header = ChannelHeader::new(self.name.to_string())
            .dm(self.dm)
            .members_action(self.members_action)
            .members_active(self.members_active)
            .gallery_trigger(gallery_trigger)
            .show_inbox(self.show_inbox)
            .on_toggle_members(members_toggle)
            .show_threads(show_threads);
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
            header = header.inbox_popover(handle).inbox_context(clan_id, locale);
        }
        if let Some(handle) = self.pin_handle.clone() {
            header = header.pin_popover(handle, settings);
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
struct PinPopoverTrigger {
    open: bool,
    icon_color: Hsla,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl PinPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.text_muted.into(),
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
        let mut button = Button::new("hdr-pin-trigger").with_size(Size::Small).icon(
            Icon::new(IconName::PinRight)
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
