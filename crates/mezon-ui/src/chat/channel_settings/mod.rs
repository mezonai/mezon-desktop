pub mod add_mem_role_modal;
pub mod channel_acl;
pub mod permission_overrides;
pub mod permissions_tab;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, ScrollHandle, SharedString, Window,
    div, point, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, ClanList, PermissionStore, Settings,
    can_manage_channel,
};

use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};
use permissions_tab::PermissionsTab;

const SIDEBAR_WIDTH: f32 = 224.0;
const SIDEBAR_ITEM_WIDTH: f32 = 170.0;
const CONTENT_MAX_WIDTH: f32 = 740.0;
const EXIT_BUTTON_SIZE: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSettingsTab {
    Overview,
    Permissions,
    Invites,
    Integrations,
    Category,
    QuickMenu,
    StreamThumbnail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelTabContext {
    pub channel_type: ChannelType,
    pub is_thread: bool,
    pub is_welcome_channel: bool,
    pub has_manage_channel: bool,
}

impl ChannelTabContext {
    fn unknown() -> Self {
        Self {
            channel_type: ChannelType::Text,
            is_thread: false,
            is_welcome_channel: false,
            has_manage_channel: false,
        }
    }
}

pub const SIDEBAR_TABS: &[ChannelSettingsTab] = &[
    ChannelSettingsTab::Overview,
    ChannelSettingsTab::Category,
    ChannelSettingsTab::Permissions,
    ChannelSettingsTab::Integrations,
    ChannelSettingsTab::QuickMenu,
    ChannelSettingsTab::StreamThumbnail,
];

impl ChannelSettingsTab {
    pub fn from_slug(slug: &str) -> Option<Self> {
        Some(match slug {
            "overview" => Self::Overview,
            "permissions" => Self::Permissions,
            "invites" => Self::Invites,
            "integrations" => Self::Integrations,
            "category" => Self::Category,
            "quick-actions" => Self::QuickMenu,
            "stream-thumbnail" => Self::StreamThumbnail,
            _ => return None,
        })
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Permissions => "permissions",
            Self::Invites => "invites",
            Self::Integrations => "integrations",
            Self::Category => "category",
            Self::QuickMenu => "quick-actions",
            Self::StreamThumbnail => "stream-thumbnail",
        }
    }

    fn i18n_key(self) -> &'static str {
        match self {
            Self::Overview => "channelSetting.tabs.overview",
            Self::Permissions => "channelSetting.tabs.permissions",
            Self::Invites => "channelSetting.tabs.invites",
            Self::Integrations => "channelSetting.tabs.integrations",
            Self::Category => "channelSetting.tabs.category",
            Self::QuickMenu => "channelSetting.tabs.quickMenu",
            Self::StreamThumbnail => "channelSetting.tabs.streamThumbnail",
        }
    }

    pub fn visible_in_sidebar(self, ctx: ChannelTabContext) -> bool {
        let is_voice = ctx.channel_type == ChannelType::Voice;
        let is_stream = ctx.channel_type == ChannelType::Stream;
        let is_app = ctx.channel_type == ChannelType::App;
        match self {
            Self::Overview => true,
            Self::Category => !ctx.is_thread,
            Self::Permissions => {
                !ctx.is_thread
                    && !is_voice
                    && !is_stream
                    && !is_app
                    && !ctx.is_welcome_channel
                    && ctx.has_manage_channel
            }
            Self::Integrations => ctx.has_manage_channel && !is_stream && !is_voice,
            Self::QuickMenu => ctx.has_manage_channel && !is_voice && !is_stream && !is_app,
            Self::StreamThumbnail => ctx.has_manage_channel && is_stream,
            Self::Invites => false,
        }
    }

    pub fn resolve_accessible(self, ctx: ChannelTabContext) -> Self {
        if self.visible_in_sidebar(ctx) {
            return self;
        }
        SIDEBAR_TABS
            .iter()
            .copied()
            .find(|tab| tab.visible_in_sidebar(ctx))
            .unwrap_or(Self::Overview)
    }
}

pub struct ChannelSettingScreen {
    clan_id: ClanId,
    channel_id: ChannelId,
    current_tab: ChannelSettingsTab,
    settings: Entity<Settings>,
    permissions_tab: Option<Entity<PermissionsTab>>,
    content_scroll: ScrollHandle,
    nav_scroll: ScrollHandle,
    focus_handle: FocusHandle,
    focus_on_show: bool,
}

impl ChannelSettingScreen {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&ChannelList::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&PermissionStore::global(cx), |this, _, cx| {
            this.reresolve_tab(cx);
            cx.notify();
        })
        .detach();
        Self {
            clan_id: ClanId(0),
            channel_id: ChannelId(0),
            current_tab: ChannelSettingsTab::Overview,
            settings,
            permissions_tab: None,
            content_scroll: ScrollHandle::new(),
            nav_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            focus_on_show: false,
        }
    }

    pub fn release_active_tab(&mut self, cx: &mut Context<Self>) {
        if self.permissions_tab.take().is_some() || !self.clan_id.is_zero() {
            cx.notify();
        }
        self.clan_id = ClanId(0);
        self.channel_id = ChannelId(0);
        self.current_tab = ChannelSettingsTab::Overview;
        self.focus_on_show = false;
        self.content_scroll.set_offset(point(px(0.0), px(0.0)));
    }

    pub fn set_target(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        tab: ChannelSettingsTab,
        cx: &mut Context<Self>,
    ) {
        let target_changed = self.clan_id != clan_id || self.channel_id != channel_id;
        self.clan_id = clan_id;
        self.channel_id = channel_id;
        if clan_id.is_zero() {
            return;
        }
        PermissionStore::global(cx).update(cx, |store, cx| {
            store.load_clan_permissions(clan_id, cx);
        });
        let ctx = self.tab_context(cx);
        let resolved = tab.resolve_accessible(ctx);
        if resolved != tab {
            crate::router::replace(
                cx,
                crate::router::Route::ChannelSettings {
                    clan_id,
                    channel_id,
                    tab: resolved,
                },
            );
            return;
        }
        if target_changed || resolved != self.current_tab {
            self.permissions_tab = None;
            self.content_scroll.set_offset(point(px(0.0), px(0.0)));
        }
        self.current_tab = resolved;
        self.activate_tab(cx);
        self.focus_on_show = true;
        cx.notify();
    }

    fn reresolve_tab(&mut self, cx: &mut Context<Self>) {
        if self.clan_id.is_zero() {
            return;
        }
        if !PermissionStore::global(cx)
            .read(cx)
            .has_clan_permissions_loaded(self.clan_id, cx)
        {
            return;
        }
        let ctx = self.tab_context(cx);
        let resolved = self.current_tab.resolve_accessible(ctx);
        if resolved != self.current_tab {
            crate::router::replace(
                cx,
                crate::router::Route::ChannelSettings {
                    clan_id: self.clan_id,
                    channel_id: self.channel_id,
                    tab: resolved,
                },
            );
        }
    }

    fn tab_context(&self, cx: &App) -> ChannelTabContext {
        let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
        else {
            return ChannelTabContext::unknown();
        };
        let channel_type = channel.channel_type;
        let welcome_channel_id = ClanList::global(cx)
            .read(cx)
            .welcome_channel_id(self.clan_id);
        let has_manage_channel = can_manage_channel(self.clan_id, cx);
        ChannelTabContext {
            channel_type,
            is_thread: channel_type == ChannelType::Thread,
            is_welcome_channel: welcome_channel_id == Some(self.channel_id),
            has_manage_channel,
        }
    }

    fn activate_tab(&mut self, cx: &mut Context<Self>) {
        if self.current_tab != ChannelSettingsTab::Permissions {
            return;
        }
        if self.permissions_tab.is_some() {
            return;
        }
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let settings = self.settings.clone();
        let tab = cx.new(|cx| PermissionsTab::new(clan_id, channel_id, settings, cx));
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        self.permissions_tab = Some(tab);
    }

    fn channel_label(&self, cx: &App) -> SharedString {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| SharedString::from(channel.name.clone()))
            .unwrap_or_default()
    }

    fn render_sidebar(
        &self,
        locale: &str,
        theme: &Theme,
        ctx: ChannelTabContext,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut nav = v_flex().w(px(SIDEBAR_ITEM_WIDTH));
        nav = nav.child(
            h_flex()
                .max_w(px(SIDEBAR_ITEM_WIDTH))
                .items_start()
                .gap_1()
                .child(
                    Icon::new(channel_tab_icon(ctx))
                        .size(px(20.0))
                        .flex_shrink_0()
                        .text_color(theme.tokens.bg_icon_theme),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui::rgb(0x84_ad_ff))
                        .child(self.channel_label(cx).to_uppercase()),
                ),
        );

        for tab in SIDEBAR_TABS.iter().copied() {
            if !tab.visible_in_sidebar(ctx) {
                continue;
            }
            let is_active = tab == self.current_tab;
            let label = mezon_i18n::t(locale, tab.i18n_key());
            let route = crate::router::Route::ChannelSettings {
                clan_id: self.clan_id,
                channel_id: self.channel_id,
                tab,
            };
            nav = nav.child(
                div()
                    .id(tab.slug())
                    .w(px(SIDEBAR_ITEM_WIDTH))
                    .ml(px(-8.0))
                    .mt(px(8.0))
                    .p_2()
                    .rounded(px(5.0))
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.tokens.bg_item_theme_hover))
                    .when(is_active, |el| {
                        el.bg(theme.bg_hover).text_color(theme.text_primary)
                    })
                    .when(!is_active, |el| {
                        el.text_color(theme.tokens.text_theme_primary)
                    })
                    .child(label)
                    .on_click(move |_, _, cx| {
                        crate::router::replace(cx, route.clone());
                    }),
            );
        }

        div()
            .id("channel-settings-nav")
            .flex_shrink_0()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.nav_scroll)
            .bg(theme.tokens.theme_setting_nav)
            .flex()
            .justify_end()
            .pt(px(96.0))
            .pr_2()
            .child(nav)
    }

    fn render_exit_column(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .w(px(120.0))
            .pt(px(94.0))
            .pl(px(20.0))
            .child(
                v_flex()
                    .w(px(EXIT_BUTTON_SIZE))
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("channel-settings-exit")
                            .size(px(EXIT_BUTTON_SIZE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border_2()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::CloseButton)
                                    .size(px(16.0))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .on_click(|_, _, cx| crate::router::go_back(cx)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child("ESC"),
                    ),
            )
    }
}

fn channel_tab_icon(ctx: ChannelTabContext) -> IconName {
    match ctx.channel_type {
        ChannelType::Thread => IconName::ThreadIcon,
        ChannelType::Voice => IconName::Speaker,
        ChannelType::Stream => IconName::Stream,
        _ => IconName::Hashtag,
    }
}

impl Focusable for ChannelSettingScreen {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChannelSettingScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_on_show {
            self.focus_on_show = false;
            window.focus(&self.focus_handle, cx);
        }

        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let ctx = self.tab_context(cx);

        let body = match self.current_tab {
            ChannelSettingsTab::Permissions => self
                .permissions_tab
                .as_ref()
                .map(|tab| tab.clone().into_any_element()),
            _ => None,
        }
        .unwrap_or_else(|| {
            div()
                .flex_1()
                .py_8()
                .text_sm()
                .text_color(theme.text_muted)
                .child(mezon_i18n::t(&locale, "common.comingSoon"))
                .into_any_element()
        });

        h_flex()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                crate::router::go_back(cx);
            }))
            .size_full()
            .min_h_0()
            .items_stretch()
            .bg(theme.tokens.theme_setting_primary)
            .child(self.render_sidebar(&locale, &theme, ctx, cx))
            .child(
                div()
                    .id("channel-settings-content")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.content_scroll)
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .justify_start()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(CONTENT_MAX_WIDTH))
                                    .pl(px(40.0))
                                    .pr(px(10.0))
                                    .pt(px(94.0))
                                    .pb(px(28.0))
                                    .child(body),
                            )
                            .child(self.render_exit_column(&theme)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(
        channel_type: ChannelType,
        is_thread: bool,
        welcome: bool,
        manage: bool,
    ) -> ChannelTabContext {
        ChannelTabContext {
            channel_type,
            is_thread,
            is_welcome_channel: welcome,
            has_manage_channel: manage,
        }
    }

    #[test]
    fn permissions_tab_requires_manage_channel() {
        assert!(ChannelSettingsTab::Permissions.visible_in_sidebar(ctx(
            ChannelType::Text,
            false,
            false,
            true
        )));
        assert!(!ChannelSettingsTab::Permissions.visible_in_sidebar(ctx(
            ChannelType::Text,
            false,
            false,
            false
        )));
    }

    #[test]
    fn permissions_tab_hidden_for_thread_voice_stream_app_and_welcome() {
        for channel_type in [ChannelType::Voice, ChannelType::Stream, ChannelType::App] {
            assert!(!ChannelSettingsTab::Permissions.visible_in_sidebar(ctx(
                channel_type,
                false,
                false,
                true
            )));
        }
        assert!(!ChannelSettingsTab::Permissions.visible_in_sidebar(ctx(
            ChannelType::Thread,
            true,
            false,
            true
        )));
        assert!(!ChannelSettingsTab::Permissions.visible_in_sidebar(ctx(
            ChannelType::Text,
            false,
            true,
            true
        )));
    }

    #[test]
    fn category_tab_is_hidden_on_threads_only() {
        assert!(ChannelSettingsTab::Category.visible_in_sidebar(ctx(
            ChannelType::Text,
            false,
            false,
            false
        )));
        assert!(!ChannelSettingsTab::Category.visible_in_sidebar(ctx(
            ChannelType::Thread,
            true,
            false,
            true
        )));
    }

    #[test]
    fn stream_thumbnail_only_for_stream_with_manage() {
        assert!(ChannelSettingsTab::StreamThumbnail.visible_in_sidebar(ctx(
            ChannelType::Stream,
            false,
            false,
            true
        )));
        assert!(!ChannelSettingsTab::StreamThumbnail.visible_in_sidebar(ctx(
            ChannelType::Text,
            false,
            false,
            true
        )));
    }

    #[test]
    fn inaccessible_tab_falls_back_to_first_visible() {
        let no_manage = ctx(ChannelType::Text, false, false, false);
        assert_eq!(
            ChannelSettingsTab::Permissions.resolve_accessible(no_manage),
            ChannelSettingsTab::Overview
        );
        let manage = ctx(ChannelType::Text, false, false, true);
        assert_eq!(
            ChannelSettingsTab::Permissions.resolve_accessible(manage),
            ChannelSettingsTab::Permissions
        );
    }

    #[test]
    fn slug_roundtrips_for_every_tab() {
        for tab in [
            ChannelSettingsTab::Overview,
            ChannelSettingsTab::Permissions,
            ChannelSettingsTab::Invites,
            ChannelSettingsTab::Integrations,
            ChannelSettingsTab::Category,
            ChannelSettingsTab::QuickMenu,
            ChannelSettingsTab::StreamThumbnail,
        ] {
            assert_eq!(ChannelSettingsTab::from_slug(tab.slug()), Some(tab));
        }
    }
}
