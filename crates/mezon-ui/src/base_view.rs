use gpui::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    label::Label,
    scroll::ScrollableElement as _,
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    v_flex,
};

use crate::router::{Route, Router};

pub struct BaseView {
    router: Router,
}

impl BaseView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            router: Router::new(),
        }
    }

    pub fn navigate(entity: &Entity<Self>, path: impl Into<String>, cx: &mut gpui::App) {
        let path = path.into();
        entity.update(cx, |this, cx| {
            this.router.navigate(path);
            cx.notify();
        });
    }

    pub fn current_path(&self) -> &str {
        self.router.current_path()
    }

    fn render_sidebar(&self, entity: Entity<Self>) -> AnyElement {
        let current_path = self.router.current_path().to_string();

        let mut menu = SidebarMenu::new();
        for (label, path, icon) in [
            ("Home", "/chat", IconName::LayoutDashboard),
            ("Direct", "/chat/direct", IconName::CircleUser),
            (
                "Channel",
                "/chat/clans/demo-clan/channels/general",
                IconName::FolderOpen,
            ),
            ("Settings", "/settings", IconName::Settings),
        ] {
            let item_entity = entity.clone();
            let path_string = path.to_string();
            menu = menu.child(
                SidebarMenuItem::new(label)
                    .icon(Icon::new(icon).size_4())
                    .active(route_is_active(&current_path, path))
                    .on_click(move |_, _window, cx| {
                        Self::navigate(&item_entity, path_string.clone(), cx);
                    }),
            );
        }

        Sidebar::left()
            .collapsible(false)
            .header(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::GalleryVerticalEnd).size_5())
                    .child(
                        v_flex().min_w_0().child(Label::new("Mezon")).child(
                            div()
                                .text_xs()
                                .text_color(gpui::hsla(0.0, 0.0, 0.65, 1.0))
                                .child(self.router.current_path().to_string()),
                        ),
                    ),
            )
            .child(menu)
            .into_any_element()
    }

    fn render_content(&self, entity: Entity<Self>) -> AnyElement {
        let route = self.router.route();
        let current_path = self.router.current_path().to_string();

        let (title, subtitle, placeholder) = match route {
            Route::Chat => (
                "Chat".into(),
                format!("Authenticated app shell foundation - {current_path}"),
                self.render_placeholder(
                    IconName::Inbox,
                    "Chat foundation",
                    "Clan, channel, and message data will attach here in the next phase.",
                    None,
                ),
            ),
            Route::Direct => (
                "Direct Messages".into(),
                format!("Base route: /chat/direct - {current_path}"),
                self.render_placeholder(
                    IconName::CircleUser,
                    "Direct message list",
                    "Direct conversations are intentionally placeholder-only for now.",
                    None,
                ),
            ),
            Route::DirectMessage {
                direct_id,
                message_type,
            } => (
                "Direct Message".into(),
                format!("Base route: /chat/direct/message/:direct_id/:type - {current_path}"),
                self.render_placeholder(
                    IconName::CircleUser,
                    format!("Direct {direct_id}"),
                    format!("Message type: {message_type}"),
                    None,
                ),
            ),
            Route::Channel {
                clan_id,
                channel_id,
            } => (
                "Channel".into(),
                format!("Base route: /chat/clans/:clan_id/channels/:channel_id - {current_path}"),
                self.render_placeholder(
                    IconName::FolderOpen,
                    format!("#{channel_id}"),
                    format!("Clan: {clan_id}"),
                    None,
                ),
            ),
            Route::SettingsAccount
            | Route::SettingsProfile
            | Route::SettingsDevices
            | Route::SettingsAppearance
            | Route::SettingsActivity
            | Route::SettingsNotifications
            | Route::SettingsLanguage
            | Route::SettingsVoice
            | Route::SettingsAdvanced => (
                "Settings".into(),
                format!("Base route: /settings - {current_path}"),
                self.render_placeholder(
                    IconName::Settings,
                    "Settings foundation",
                    "Settings panels will be added after the shell is connected.",
                    None,
                ),
            ),
            Route::NotFound { path } => {
                let not_found_entity = entity.clone();
                (
                    "Not Found".into(),
                    format!("Unknown route - {current_path}"),
                    self.render_placeholder(
                        IconName::TriangleAlert,
                        format!("No route for {path}"),
                        "This path is not registered in the local Mezon router.",
                        Some(
                            Button::new("back-to-chat")
                                .primary()
                                .small()
                                .label("Back to Chat")
                                .on_click(move |_, _window, cx| {
                                    Self::navigate(&not_found_entity, Router::DEFAULT_PATH, cx);
                                })
                                .into_any_element(),
                        ),
                    ),
                )
            }
        };

        let entity_for_direct = entity.clone();
        let entity_for_unknown = entity.clone();

        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.render_header(
                title,
                subtitle,
                vec![
                    Button::new("sample-dm")
                        .label("Sample DM")
                        .small()
                        .icon(Icon::new(IconName::CircleUser))
                        .on_click(move |_, _window, cx| {
                            Self::navigate(
                                &entity_for_direct,
                                "/chat/direct/message/demo-user/default",
                                cx,
                            );
                        })
                        .into_any_element(),
                    Button::new("not-found")
                        .label("404")
                        .ghost()
                        .small()
                        .on_click(move |_, _window, cx| {
                            Self::navigate(&entity_for_unknown, "/unknown/path", cx);
                        })
                        .into_any_element(),
                ],
            ))
            .child(
                div()
                    .id("content-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_6()
                    .child(placeholder)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }

    fn render_header(
        &self,
        title: SharedString,
        subtitle: String,
        actions: Vec<AnyElement>,
    ) -> AnyElement {
        h_flex()
            .justify_between()
            .items_center()
            .gap_4()
            .px_6()
            .py_4()
            .border_b_1()
            .border_color(gpui::hsla(0.0, 0.0, 0.22, 1.0))
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().text_lg().font_semibold().child(title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(gpui::hsla(0.0, 0.0, 0.65, 1.0))
                            .child(subtitle),
                    ),
            )
            .child(h_flex().gap_2().children(actions))
            .into_any_element()
    }

    fn render_placeholder(
        &self,
        icon: IconName,
        title: impl Into<SharedString>,
        subtitle: impl Into<SharedString>,
        action: Option<AnyElement>,
    ) -> AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .rounded_lg()
            .border_1()
            .border_color(gpui::hsla(0.0, 0.0, 0.20, 1.0))
            .child(Icon::new(icon).size_8())
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_base().font_medium().child(title.into()))
                    .child(
                        div()
                            .max_w_96()
                            .text_center()
                            .text_sm()
                            .text_color(gpui::hsla(0.0, 0.0, 0.65, 1.0))
                            .child(subtitle.into()),
                    ),
            )
            .when_some(action, |this, action| this.child(action))
            .into_any_element()
    }
}

impl Render for BaseView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();
        let sidebar = self.render_sidebar(entity.clone());
        let content = self.render_content(entity);

        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(sidebar)
            .child(content)
    }
}

fn route_is_active(current: &str, target: &str) -> bool {
    if target == Router::DEFAULT_PATH {
        current == target
    } else {
        current == target || current.starts_with(&format!("{target}/"))
    }
}
