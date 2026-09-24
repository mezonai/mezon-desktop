use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString, Window, deferred,
    div, point, prelude::*, px,
};
use mezon_store::{
    ChannelList, ClanId, ClanList, ClanSettingsPermissions, PermissionStore, Settings,
};

use super::archived_channel_page::ArchivedChannelPage;
use super::audit_log_setting_page::AuditLogSettingPage;
use super::category_sort_page::CategorySortPage;
use super::community_setting_page::{CommunitySettingPage, render_community_save_bar};
use super::emoji_setting_page::EmojiSettingPage;
use super::integration_setting_page::IntegrationSettingPage;
use super::onboarding_setting_page::{
    OnboardingSettingPage, render_onboarding_editor_modal, render_onboarding_save_bar,
};
use super::overview_setting_page::{OverviewSettingPage, render_clan_overview_save_bar};
use super::role_icon_picker::render_role_icon_picker_modal;
use super::role_setting_page::{RoleSettingPage, render_role_save_bar};
use super::sound_setting_page::SoundSettingPage;
use super::sticker_setting_page::StickerSettingPage;
use crate::theme::{ActiveTheme, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClanSettingsPage {
    Overview,
    Roles,
    CategoryOrder,
    ArchivedChannels,
    Emoji,
    ImageStickers,
    VoiceStickers,
    Integrations,
    AuditLog,
    Onboarding,
    ClanCommunity,
}

impl ClanSettingsPage {
    pub fn from_slug(slug: &str) -> Option<Self> {
        Some(match slug {
            "overview" => Self::Overview,
            "roles" => Self::Roles,
            "category-order" => Self::CategoryOrder,
            "archived-channels" => Self::ArchivedChannels,
            "emoji" => Self::Emoji,
            "image-stickers" => Self::ImageStickers,
            "voice-stickers" => Self::VoiceStickers,
            "integrations" => Self::Integrations,
            "audit-log" => Self::AuditLog,
            "onboarding" => Self::Onboarding,
            "clan-community" => Self::ClanCommunity,
            _ => return None,
        })
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Roles => "roles",
            Self::CategoryOrder => "category-order",
            Self::ArchivedChannels => "archived-channels",
            Self::Emoji => "emoji",
            Self::ImageStickers => "image-stickers",
            Self::VoiceStickers => "voice-stickers",
            Self::Integrations => "integrations",
            Self::AuditLog => "audit-log",
            Self::Onboarding => "onboarding",
            Self::ClanCommunity => "clan-community",
        }
    }

    fn i18n_key(self) -> &'static str {
        match self {
            Self::Overview => "clanSettings.sidebar.items.overview",
            Self::Roles => "clanSettings.sidebar.items.roles",
            Self::CategoryOrder => "clanSettings.sidebar.items.categoryOrder",
            Self::ArchivedChannels => "clanSettings.sidebar.items.archivedChannels",
            Self::Emoji => "clanSettings.sidebar.items.emoji",
            Self::ImageStickers => "clanSettings.sidebar.items.imageStickers",
            Self::VoiceStickers => "clanSettings.sidebar.items.voiceStickers",
            Self::Integrations => "clanSettings.sidebar.items.integrations",
            Self::AuditLog => "clanSettings.sidebar.items.auditLog",
            Self::Onboarding => "clanSettings.sidebar.items.onboarding",
            Self::ClanCommunity => "clanSettings.sidebar.items.clanCommunity",
        }
    }

    pub fn default_for_permissions(perms: ClanSettingsPermissions) -> Self {
        if perms.has_manage_clan {
            Self::Overview
        } else {
            Self::Emoji
        }
    }

    pub fn visible_in_sidebar(self, perms: ClanSettingsPermissions) -> bool {
        match self {
            Self::Integrations => perms.has_manage_clan || perms.has_manage_channel,
            Self::ArchivedChannels => {
                perms.has_manage_clan || perms.has_administrator || perms.is_clan_owner
            }
            Self::Overview
            | Self::Roles
            | Self::AuditLog
            | Self::Onboarding
            | Self::ClanCommunity => perms.has_manage_clan,
            Self::CategoryOrder | Self::Emoji | Self::ImageStickers | Self::VoiceStickers => true,
        }
    }

    pub fn resolve_accessible(self, perms: ClanSettingsPermissions) -> Self {
        if self.visible_in_sidebar(perms) {
            return self;
        }
        SIDEBAR_SECTIONS
            .iter()
            .flat_map(|section| section.pages.iter().copied())
            .find(|page| page.visible_in_sidebar(perms))
            .unwrap_or_else(|| Self::default_for_permissions(perms))
    }
}

struct SidebarSection {
    title_key: Option<&'static str>,
    pages: &'static [ClanSettingsPage],
}

const SIDEBAR_SECTIONS: &[SidebarSection] = &[
    SidebarSection {
        title_key: None,
        pages: &[
            ClanSettingsPage::Overview,
            ClanSettingsPage::Roles,
            ClanSettingsPage::CategoryOrder,
            ClanSettingsPage::ArchivedChannels,
        ],
    },
    SidebarSection {
        title_key: Some("clanSettings.sidebar.sectionTitles.emotions"),
        pages: &[
            ClanSettingsPage::Emoji,
            ClanSettingsPage::ImageStickers,
            ClanSettingsPage::VoiceStickers,
        ],
    },
    SidebarSection {
        title_key: Some("clanSettings.sidebar.sectionTitles.apps"),
        pages: &[ClanSettingsPage::Integrations],
    },
    SidebarSection {
        title_key: Some("clanSettings.sidebar.sectionTitles.moderation"),
        pages: &[ClanSettingsPage::AuditLog],
    },
    SidebarSection {
        title_key: None,
        pages: &[
            ClanSettingsPage::Onboarding,
            ClanSettingsPage::ClanCommunity,
        ],
    },
];

pub struct ClanSettingScreen {
    clan_id: ClanId,
    settings: Entity<Settings>,
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    current_page: ClanSettingsPage,
    overview_page: Option<Entity<OverviewSettingPage>>,
    category_sort_page: Option<Entity<CategorySortPage>>,
    archived_channel_page: Option<Entity<ArchivedChannelPage>>,
    audit_log_page: Option<Entity<AuditLogSettingPage>>,
    emoji_page: Option<Entity<EmojiSettingPage>>,
    sticker_page: Option<Entity<StickerSettingPage>>,
    sound_page: Option<Entity<SoundSettingPage>>,
    roles_page: Option<Entity<RoleSettingPage>>,
    integrations_page: Option<Entity<IntegrationSettingPage>>,
    community_page: Option<Entity<CommunitySettingPage>>,
    onboarding_page: Option<Entity<OnboardingSettingPage>>,
    scroll: ScrollHandle,
    nav_scroll: ScrollHandle,
    focus_handle: FocusHandle,
    focus_on_show: bool,
}

impl ClanSettingScreen {
    pub fn new(
        clan_id: ClanId,
        page: ClanSettingsPage,
        settings: Entity<Settings>,
        clan_list: Entity<ClanList>,
        channel_list: Entity<ChannelList>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&clan_list, |_, _, cx| cx.notify()).detach();
        cx.observe(&PermissionStore::global(cx), |this, _, cx| {
            this.reresolve_page(cx);
            cx.notify();
        })
        .detach();
        let mut this = Self {
            clan_id,
            settings,
            clan_list,
            channel_list,
            current_page: page,
            overview_page: None,
            category_sort_page: None,
            archived_channel_page: None,
            audit_log_page: None,
            emoji_page: None,
            sticker_page: None,
            sound_page: None,
            roles_page: None,
            integrations_page: None,
            community_page: None,
            onboarding_page: None,
            scroll: ScrollHandle::new(),
            nav_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            focus_on_show: false,
        };
        if !clan_id.is_zero() {
            PermissionStore::global(cx).update(cx, |store, cx| {
                store.load_clan_permissions(clan_id, cx);
            });
            this.activate_page(page, cx);
        }
        this
    }

    pub fn release_active_page(&mut self, cx: &mut Context<Self>) {
        self.release_page(self.current_page, cx);
        self.reset_content_scroll();
    }

    fn reresolve_page(&mut self, cx: &mut Context<Self>) {
        if self.clan_id.is_zero() {
            return;
        }
        let resolved = {
            let store = PermissionStore::global(cx);
            let store = store.read(cx);
            if !store.has_clan_permissions_loaded(self.clan_id, cx) {
                return;
            }
            self.current_page
                .resolve_accessible(store.clan_settings_permissions(self.clan_id, cx))
        };
        if resolved != self.current_page {
            crate::router::replace(
                cx,
                crate::router::Route::ClanSettings {
                    clan_id: self.clan_id,
                    page: resolved,
                },
            );
        }
    }

    pub fn set_clan_and_page(
        &mut self,
        clan_id: ClanId,
        page: ClanSettingsPage,
        cx: &mut Context<Self>,
    ) {
        let clan_changed = self.clan_id != clan_id;
        self.clan_id = clan_id;
        let resolved = {
            let store = PermissionStore::global(cx);
            store.update(cx, |store, cx| {
                store.load_clan_permissions(clan_id, cx);
            });
            let store = store.read(cx);
            if store.has_clan_permissions_loaded(clan_id, cx) {
                page.resolve_accessible(store.clan_settings_permissions(clan_id, cx))
            } else {
                page
            }
        };
        if resolved != page {
            crate::router::replace(
                cx,
                crate::router::Route::ClanSettings {
                    clan_id,
                    page: resolved,
                },
            );
            return;
        }
        let page_changed = resolved != self.current_page;
        if clan_changed || page_changed {
            self.release_page(self.current_page, cx);
            if clan_changed {
                self.release_page(ClanSettingsPage::Overview, cx);
                self.release_page(ClanSettingsPage::CategoryOrder, cx);
                self.release_page(ClanSettingsPage::ArchivedChannels, cx);
                self.release_page(ClanSettingsPage::AuditLog, cx);
                self.release_page(ClanSettingsPage::Emoji, cx);
                self.release_page(ClanSettingsPage::ImageStickers, cx);
                self.release_page(ClanSettingsPage::VoiceStickers, cx);
                self.release_page(ClanSettingsPage::Roles, cx);
                self.release_page(ClanSettingsPage::Integrations, cx);
                self.release_page(ClanSettingsPage::ClanCommunity, cx);
                self.release_page(ClanSettingsPage::Onboarding, cx);
            }
            self.reset_content_scroll();
        }
        self.current_page = resolved;
        self.activate_page(resolved, cx);
        self.focus_on_show = true;
        cx.notify();
    }

    fn release_page(&mut self, page: ClanSettingsPage, cx: &mut Context<Self>) {
        match page {
            ClanSettingsPage::Overview => {
                if let Some(entity) = self.overview_page.take() {
                    entity.update(cx, |page, cx| page.release(cx));
                }
            }
            ClanSettingsPage::CategoryOrder => {
                if let Some(entity) = self.category_sort_page.take() {
                    entity.update(cx, |page, _| page.release());
                }
            }
            ClanSettingsPage::ArchivedChannels => {
                if let Some(entity) = self.archived_channel_page.take() {
                    entity.update(cx, |page, _| page.release());
                }
            }
            ClanSettingsPage::AuditLog => {
                if let Some(entity) = self.audit_log_page.take() {
                    entity.update(cx, |page, cx| page.release(cx));
                }
            }
            ClanSettingsPage::Emoji => {
                if let Some(entity) = self.emoji_page.take() {
                    entity.update(cx, |page, cx| page.release(cx));
                }
            }
            ClanSettingsPage::ImageStickers => {
                if let Some(entity) = self.sticker_page.take() {
                    entity.update(cx, |page, cx| page.release(cx));
                }
            }
            ClanSettingsPage::VoiceStickers => {
                if let Some(entity) = self.sound_page.take() {
                    entity.update(cx, |page, cx| page.release(cx));
                }
            }
            ClanSettingsPage::ClanCommunity => {
                if let Some(entity) = self.community_page.take() {
                    entity.update(cx, |page, cx| page.release(cx));
                }
            }
            ClanSettingsPage::Onboarding => {
                if let Some(entity) = self.onboarding_page.take() {
                    entity.update(cx, |page, _| page.release());
                }
            }
            _ => {}
        }
        if page == ClanSettingsPage::Roles
            && let Some(entity) = self.roles_page.take()
        {
            entity.update(cx, |page, _| page.release());
        }
        if page == ClanSettingsPage::Integrations
            && let Some(entity) = self.integrations_page.take()
        {
            entity.update(cx, |page, _| page.release());
        }
    }

    fn reset_content_scroll(&mut self) {
        self.scroll.set_offset(point(px(0.0), px(0.0)));
    }

    fn activate_page(&mut self, page: ClanSettingsPage, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let clan_id = self.clan_id;
        match page {
            ClanSettingsPage::Overview if self.overview_page.is_none() => {
                let clan_list = self.clan_list.clone();
                let channel_list = self.channel_list.clone();
                self.overview_page = Some(cx.new(|cx| {
                    OverviewSettingPage::new(clan_id, clan_list, channel_list, settings, cx)
                }));
                if let Some(overview) = &self.overview_page {
                    cx.observe(overview, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::CategoryOrder if self.category_sort_page.is_none() => {
                let channel_list = self.channel_list.clone();
                self.category_sort_page =
                    Some(cx.new(|cx| CategorySortPage::new(clan_id, channel_list, cx)));
                if let Some(page) = &self.category_sort_page {
                    cx.observe(page, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::ArchivedChannels if self.archived_channel_page.is_none() => {
                let channel_list = self.channel_list.clone();
                self.archived_channel_page = Some(
                    cx.new(|cx| ArchivedChannelPage::new(clan_id, channel_list, settings, cx)),
                );
                if let Some(page) = &self.archived_channel_page {
                    cx.observe(page, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::AuditLog if self.audit_log_page.is_none() => {
                self.audit_log_page =
                    Some(cx.new(|cx| AuditLogSettingPage::new(clan_id, settings, cx)));
                if let Some(audit_log) = &self.audit_log_page {
                    cx.observe(audit_log, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::Emoji if self.emoji_page.is_none() => {
                self.emoji_page = Some(cx.new(|cx| EmojiSettingPage::new(clan_id, settings, cx)));
                if let Some(page) = &self.emoji_page {
                    cx.observe(page, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::ImageStickers if self.sticker_page.is_none() => {
                self.sticker_page =
                    Some(cx.new(|cx| StickerSettingPage::new(clan_id, settings, cx)));
            }
            ClanSettingsPage::VoiceStickers if self.sound_page.is_none() => {
                self.sound_page = Some(cx.new(|cx| SoundSettingPage::new(clan_id, settings, cx)));
                if let Some(page) = &self.sound_page {
                    cx.observe(page, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::ClanCommunity if self.community_page.is_none() => {
                let clan_list = self.clan_list.clone();
                self.community_page =
                    Some(cx.new(|cx| CommunitySettingPage::new(clan_id, clan_list, settings, cx)));
                if let Some(page) = &self.community_page {
                    cx.observe(page, |_, _, cx| cx.notify()).detach();
                }
            }
            ClanSettingsPage::Onboarding if self.onboarding_page.is_none() => {
                let clan_list = self.clan_list.clone();
                let channel_list = self.channel_list.clone();
                self.onboarding_page = Some(cx.new(|cx| {
                    OnboardingSettingPage::new(clan_id, clan_list, channel_list, settings, cx)
                }));
                if let Some(page) = &self.onboarding_page {
                    cx.observe(page, |_, _, cx| cx.notify()).detach();
                }
            }
            _ => {}
        }
        if page == ClanSettingsPage::Roles && self.roles_page.is_none() {
            let settings = self.settings.clone();
            let clan_id = self.clan_id;
            self.roles_page = Some(cx.new(|cx| RoleSettingPage::new(clan_id, settings, cx)));
            if let Some(roles) = &self.roles_page {
                cx.observe(roles, |_, _, cx| cx.notify()).detach();
            }
        }
        if page == ClanSettingsPage::Integrations && self.integrations_page.is_none() {
            let channel_list = self.channel_list.clone();
            let settings = self.settings.clone();
            let clan_id = self.clan_id;
            let can_manage_clan_webhooks = {
                let store = PermissionStore::global(cx).read(cx);
                let perms = store.clan_settings_permissions(clan_id, cx);
                perms.is_clan_owner || perms.has_manage_clan
            };
            self.integrations_page = Some(cx.new(|cx| {
                IntegrationSettingPage::new(
                    clan_id,
                    channel_list,
                    settings,
                    can_manage_clan_webhooks,
                    cx,
                )
            }));
            if let Some(integrations) = &self.integrations_page {
                cx.observe(integrations, |_, _, cx| cx.notify()).detach();
            }
        }
    }

    fn page_title(&self, page: ClanSettingsPage, locale: &str) -> SharedString {
        mezon_i18n::t(locale, page.i18n_key()).into()
    }

    fn current_page_view(&self, locale: &str, theme: &Theme, cx: &App) -> Option<gpui::AnyElement> {
        match self.current_page {
            ClanSettingsPage::Overview => self
                .overview_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::CategoryOrder => self
                .category_sort_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::ArchivedChannels => self
                .archived_channel_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::AuditLog => self
                .audit_log_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::Emoji => self
                .emoji_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::ImageStickers => self
                .sticker_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::VoiceStickers => self
                .sound_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::Roles => self
                .roles_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::Integrations => self
                .integrations_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::ClanCommunity => self
                .community_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            ClanSettingsPage::Onboarding => self.onboarding_page.as_ref().map(|page| {
                if page.read(cx).is_setup_open() {
                    OnboardingSettingPage::render_enable_card(page.clone(), locale, theme)
                } else {
                    page.clone().into_any_element()
                }
            }),
        }
    }
}

impl Focusable for ClanSettingScreen {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ClanSettingScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_on_show {
            self.focus_on_show = false;
            window.focus(&self.focus_handle, cx);
        }

        const SETTINGS_CONTENT_WIDTH: f32 = 808.0;
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let page = self.current_page;
        let clan_id = self.clan_id;
        let perms = PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(clan_id, cx);
        let fixed_content_layout = matches!(
            page,
            ClanSettingsPage::AuditLog | ClanSettingsPage::ArchivedChannels
        );
        let content = self
            .current_page_view(&locale, &theme, cx)
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .py_8()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(&locale, "common.comingSoon")),
                    )
                    .into_any_element()
            });

        let show_overview_save = page == ClanSettingsPage::Overview
            && self
                .overview_page
                .as_ref()
                .is_some_and(|overview| overview.read(cx).should_show_save_bar(cx));
        let overview_save_bar = self.overview_page.clone().filter(|_| show_overview_save);
        let roles_edit_mode = page == ClanSettingsPage::Roles
            && self
                .roles_page
                .as_ref()
                .is_some_and(|roles| roles.read(cx).is_in_edit_mode());
        let show_role_save = page == ClanSettingsPage::Roles
            && self
                .roles_page
                .as_ref()
                .is_some_and(|roles| roles.read(cx).should_show_save_bar(cx));
        let role_save_bar = self.roles_page.clone().filter(|_| show_role_save);
        let show_role_icon_picker = page == ClanSettingsPage::Roles
            && self
                .roles_page
                .as_ref()
                .is_some_and(|roles| roles.read(cx).is_role_icon_picker_open());
        let role_icon_picker = self.roles_page.clone().filter(|_| show_role_icon_picker);
        let show_community_save = page == ClanSettingsPage::ClanCommunity
            && self
                .community_page
                .as_ref()
                .is_some_and(|community| community.read(cx).should_show_save_bar(cx));
        let community_save_bar = self.community_page.clone().filter(|_| show_community_save);
        let show_onboarding_save = page == ClanSettingsPage::Onboarding
            && self
                .onboarding_page
                .as_ref()
                .is_some_and(|onboarding| onboarding.read(cx).should_show_save_bar());
        let onboarding_save_bar = self
            .onboarding_page
            .clone()
            .filter(|_| show_onboarding_save);
        let onboarding_setup_modal = self.onboarding_page.clone().filter(|onboarding| {
            page == ClanSettingsPage::Onboarding && onboarding.read(cx).is_setup_open()
        });
        let onboarding_editor_modal = self.onboarding_page.clone().filter(|onboarding| {
            page == ClanSettingsPage::Onboarding && onboarding.read(cx).is_editor_open()
        });
        let onboarding_enabled = self
            .clan_list
            .read(cx)
            .clan_by_id(clan_id)
            .is_some_and(|clan| clan.is_onboarding);

        fn nav_item(
            page: ClanSettingsPage,
            label: SharedString,
            is_active: bool,
            theme: &Theme,
            path: String,
            status: Option<bool>,
            status_label: Option<SharedString>,
        ) -> impl IntoElement {
            div()
                .id(page.slug())
                .relative()
                .children(crate::tour::probe(
                    crate::tour::TourAnchor::ClanSettingsRow(page),
                ))
                .flex()
                .items_center()
                .w_full()
                .px(px(10.0))
                .py(px(8.0))
                .mb(px(4.0))
                .rounded(px(4.0))
                .text_base()
                .font_weight(gpui::FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .when(is_active, |el| {
                    el.bg(theme.bg_hover).text_color(theme.text_primary)
                })
                .when(!is_active, |el| {
                    el.text_color(theme.tokens.text_theme_primary)
                })
                .child(h_flex().w_full().justify_between().child(label).when_some(
                    status.zip(status_label),
                    |el, (enabled, label)| {
                        el.child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(if enabled {
                                    theme.status_online
                                } else {
                                    theme.danger
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(label),
                                ),
                        )
                    },
                ))
                .on_click(move |_, _, cx| {
                    crate::router::replace(cx, crate::router::Route::from_path(&path));
                })
        }

        fn section_title(text: String, theme: &Theme) -> gpui::Div {
            div()
                .px(px(10.0))
                .py(px(4.0))
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_secondary)
                .child(text.to_uppercase())
        }

        let sidebar_title =
            mezon_i18n::t(&locale, "clanMenu.modalPanel.clanSettings").to_uppercase();
        let mut nav = v_flex().w(px(220.0));

        for section in SIDEBAR_SECTIONS {
            let visible_pages: Vec<ClanSettingsPage> = section
                .pages
                .iter()
                .copied()
                .filter(|item| item.visible_in_sidebar(perms))
                .collect();
            if visible_pages.is_empty() {
                continue;
            }
            if let Some(key) = section.title_key {
                nav = nav.child(
                    section_title(mezon_i18n::t(&locale, key).to_string(), &theme).mt(px(4.0)),
                );
            }
            for item in visible_pages {
                let path = format!("/chat/clans/{}/settings/{}", clan_id.get(), item.slug());
                nav = nav.child(nav_item(
                    item,
                    self.page_title(item, &locale),
                    page == item,
                    &theme,
                    path,
                    (item == ClanSettingsPage::Onboarding).then_some(onboarding_enabled),
                    (item == ClanSettingsPage::Onboarding).then(|| {
                        mezon_i18n::t(
                            &locale,
                            if onboarding_enabled {
                                "onBoardingClan.status.on"
                            } else {
                                "onBoardingClan.status.off"
                            },
                        )
                        .into()
                    }),
                ));
            }
            nav = nav.child(div().mt(px(4.0)).border_b_1().border_color(theme.border));
        }

        nav = nav.child(
            crate::tour::settings_entry_row("clan-settings-tour", &locale, &theme).mt(px(4.0)),
        );

        let locale_for_delete = locale.clone();
        if perms.is_clan_owner {
            nav = nav.child(
                div()
                    .id("clan-settings-delete")
                    .mt(px(4.0))
                    .w_full()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.danger_text)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .child(mezon_i18n::t(&locale, "clanSettings.sidebar.deleteClan"))
                    .on_click(move |_, window, cx| {
                        crate::app::shell::Shell::global(cx).update(cx, |shell, cx| {
                            shell.confirm_delete_clan(clan_id, &locale_for_delete, window, cx);
                        });
                    }),
            );
        }

        h_flex()
            .id("clan-settings-screen")
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                crate::router::go_back(cx);
            }))
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .relative()
            .bg(theme.tokens.theme_setting_primary)
            .child(
                v_flex()
                    .id("clan-settings-nav")
                    .relative()
                    .children(crate::tour::probe(crate::tour::TourAnchor::ClanSettingsNav))
                    .flex_shrink_0()
                    .w(gpui::relative(0.25))
                    .min_w(px(220.0))
                    .h_full()
                    .min_h_0()
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        div()
                            .flex_shrink_0()
                            .w_full()
                            .pt(px(80.0))
                            .pr(px(20.0))
                            .pl(px(20.0))
                            .pb(px(6.0))
                            .child(
                                div().flex().flex_row().justify_end().w_full().child(
                                    div()
                                        .w(px(220.0))
                                        .pl(px(10.0))
                                        .text_base()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(theme.text_primary)
                                        .child(sidebar_title),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("clan-settings-nav-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.nav_scroll)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .w_full()
                                    .pb(px(60.0))
                                    .pr(px(20.0))
                                    .pl(px(20.0))
                                    .child(nav),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .justify_start()
                    .bg(theme.tokens.theme_setting_primary)
                    .child(
                        h_flex()
                            .h_full()
                            .min_h_0()
                            .flex_shrink_0()
                            .items_start()
                            .child(
                                v_flex()
                                    .h_full()
                                    .min_h_0()
                                    .w(px(SETTINGS_CONTENT_WIDTH))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .w_full()
                                            .pl(px(40.0))
                                            .pr(px(28.0))
                                            .pt(px(60.0))
                                            .child(
                                                div()
                                                    .max_w(px(740.0))
                                                    .text_xl()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .mb_5()
                                                    .text_color(theme.text_primary)
                                                    .child(self.page_title(page, &locale)),
                                            ),
                                    )
                                    .child(
                                        if matches!(
                                            page,
                                            ClanSettingsPage::Emoji
                                                | ClanSettingsPage::ImageStickers
                                                | ClanSettingsPage::VoiceStickers
                                        ) {
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .w_full()
                                                .pl(px(40.0))
                                                .pr(px(28.0))
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .h_full()
                                                        .max_w(px(740.0))
                                                        .min_w(px(0.0))
                                                        .child(content),
                                                )
                                                .into_any_element()
                                        } else if fixed_content_layout {
                                            div()
                                                .id("clan-settings-scroll")
                                                .flex_1()
                                                .min_h_0()
                                                .overflow_hidden()
                                                .flex()
                                                .flex_col()
                                                .pb(px(28.0))
                                                .pl(px(40.0))
                                                .pr(px(28.0))
                                                .child(
                                                    v_flex()
                                                        .max_w(px(740.0))
                                                        .w_full()
                                                        .h_full()
                                                        .min_h_0()
                                                        .flex_1()
                                                        .child(content),
                                                )
                                                .into_any_element()
                                        } else {
                                            let mut scroll = div()
                                                .id("clan-settings-scroll")
                                                .flex_1()
                                                .min_h_0()
                                                .pb(px(28.0))
                                                .pl(px(40.0))
                                                .pr(px(28.0));
                                            if roles_edit_mode {
                                                scroll = scroll.overflow_hidden();
                                            } else {
                                                scroll = scroll
                                                    .overflow_y_scroll()
                                                    .track_scroll(&self.scroll);
                                            }
                                            scroll
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .w_full()
                                                        .max_w(px(740.0))
                                                        .items_stretch()
                                                        .when(roles_edit_mode, |el| {
                                                            el.h_full().min_h_0()
                                                        })
                                                        .child(content),
                                                )
                                                .into_any_element()
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .id("clan-settings-close-btn")
                                    .flex_shrink_0()
                                    .pt(px(94.0))
                                    .pl(px(20.0))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .p(px(10.0))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_secondary)
                                            .child(
                                                Icon::new(IconName::Close)
                                                    .size(px(18.0))
                                                    .text_color(theme.text_secondary),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_secondary)
                                            .child("ESC"),
                                    )
                                    .on_click(move |_, _, cx| {
                                        crate::router::go_back(cx);
                                    }),
                            ),
                    ),
            )
            .when_some(overview_save_bar, |panel, overview| {
                panel.child(deferred(render_clan_overview_save_bar(
                    overview, &locale, &theme, cx,
                )))
            })
            .when_some(role_save_bar, |panel, roles| {
                panel.child(deferred(render_role_save_bar(roles, &locale, &theme, cx)))
            })
            .when_some(role_icon_picker, |panel, roles| {
                panel.child(render_role_icon_picker_modal(
                    roles, &locale, &theme, window, cx,
                ))
            })
            .when_some(community_save_bar, |panel, community| {
                panel.child(deferred(render_community_save_bar(
                    community, &locale, &theme, cx,
                )))
            })
            .when_some(onboarding_save_bar, |panel, onboarding| {
                panel.child(deferred(render_onboarding_save_bar(
                    onboarding, &locale, &theme, cx,
                )))
            })
            .when_some(onboarding_setup_modal, |panel, onboarding| {
                panel.child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().alpha(0.72))
                        .child(onboarding),
                )
            })
            .when_some(onboarding_editor_modal, |panel, onboarding| {
                panel.child(render_onboarding_editor_modal(
                    onboarding, &locale, &theme, cx,
                ))
            })
    }
}
