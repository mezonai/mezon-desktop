use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString, Window, deferred,
    div, point, prelude::*, px,
};
use mezon_store::{
    ChannelList, ClanId, ClanList, ClanSettingsPermissions, PermissionStore, Settings,
};

use super::overview_setting_page::{OverviewSettingPage, render_clan_overview_save_bar};
use crate::theme::{ActiveTheme, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    EnableCommunity,
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
            "enable-community" => Self::EnableCommunity,
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
            Self::EnableCommunity => "enable-community",
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
            Self::EnableCommunity => "clanSettings.sidebar.items.enableCommunity",
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
            Self::Overview
            | Self::Roles
            | Self::AuditLog
            | Self::ArchivedChannels
            | Self::Onboarding
            | Self::EnableCommunity => perms.has_manage_clan,
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
            ClanSettingsPage::EnableCommunity,
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
            }
            self.reset_content_scroll();
        }
        self.current_page = resolved;
        self.activate_page(resolved, cx);
        self.focus_on_show = true;
        cx.notify();
    }

    fn release_page(&mut self, page: ClanSettingsPage, cx: &mut Context<Self>) {
        if page == ClanSettingsPage::Overview
            && let Some(entity) = self.overview_page.take()
        {
            entity.update(cx, |page, _| page.release());
        }
    }

    fn reset_content_scroll(&mut self) {
        self.scroll.set_offset(point(px(0.0), px(0.0)));
    }

    fn activate_page(&mut self, page: ClanSettingsPage, cx: &mut Context<Self>) {
        if page == ClanSettingsPage::Overview && self.overview_page.is_none() {
            let clan_list = self.clan_list.clone();
            let channel_list = self.channel_list.clone();
            let settings = self.settings.clone();
            let clan_id = self.clan_id;
            self.overview_page = Some(cx.new(|cx| {
                OverviewSettingPage::new(clan_id, clan_list, channel_list, settings, cx)
            }));
            if let Some(overview) = &self.overview_page {
                cx.observe(overview, |_, _, cx| cx.notify()).detach();
            }
        }
    }

    fn page_title(&self, page: ClanSettingsPage, locale: &str) -> SharedString {
        mezon_i18n::t(locale, page.i18n_key()).into()
    }

    fn current_page_view(&self) -> Option<gpui::AnyElement> {
        match self.current_page {
            ClanSettingsPage::Overview => self
                .overview_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            _ => None,
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
        let hide_page_title = matches!(
            page,
            ClanSettingsPage::Integrations | ClanSettingsPage::AuditLog
        );

        let content = self.current_page_view().unwrap_or_else(|| {
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

        fn nav_item(
            id: &str,
            label: SharedString,
            is_active: bool,
            theme: &Theme,
            path: String,
        ) -> impl IntoElement {
            let id = id.to_string();
            div()
                .id(id)
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
                .child(label)
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
                    item.slug(),
                    self.page_title(item, &locale),
                    page == item,
                    &theme,
                    path,
                ));
            }
            nav = nav.child(div().mt(px(4.0)).border_b_1().border_color(theme.border));
        }

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
                    .text_color(theme.status_dnd)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .child(mezon_i18n::t(&locale, "clanSettings.sidebar.deleteClan"))
                    .on_click(move |_, window, cx| {
                        let title =
                            mezon_i18n::t(&locale_for_delete, "clanSettings.sidebar.deleteClan")
                                .to_string();
                        crate::app::shell::Shell::global(cx).update(cx, |shell, cx| {
                            shell.show_coming_soon(title, &locale_for_delete, window, cx);
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
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .w_full()
                                    .child(
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
                                    .when(!hide_page_title, |panel| {
                                        panel.child(
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
                                    })
                                    .child(
                                        div()
                                            .id("clan-settings-scroll")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.scroll)
                                            .pb(px(28.0))
                                            .pl(px(40.0))
                                            .pr(px(28.0))
                                            .when(hide_page_title, |el| el.pt(px(60.0)))
                                            .child(div().max_w(px(740.0)).child(content)),
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
    }
}
