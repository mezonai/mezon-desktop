use std::sync::Arc;

use gpui::{Context, Entity, FontWeight, Window, div, prelude::*, px};
use mezon_client::AppApi;
use mezon_store::{AuthState, ChannelList, Clan, ClanList};

use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::router::{Route, Router};
use crate::theme::Theme;
use crate::{ChannelSidebar, ClanSidebar};

pub struct ChatLayout {
    router: Router,
    channel_list: Entity<ChannelList>,
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
    user_info_bar: UserInfoBar,
}

impl ChatLayout {
    pub fn new(
        router: Router,
        auth_state: Entity<AuthState>,
        api: Arc<AppApi>,
        navigate: crate::components::NavigateFn,
        cx: &mut Context<Self>,
    ) -> Self {
        let clan_list = cx.new(|_| ClanList::new());
        let channel_list = cx.new(|_| ChannelList::new());

        let on_navigate: Option<crate::components::NavigateFn> = {
            let nav = navigate.clone();
            Some(Arc::new(move |path, cx| nav(path, cx)))
        };

        let on_settings: Option<crate::components::NavigateFn> = {
            let nav = navigate.clone();
            Some(Arc::new(move |path, cx| nav(path, cx)))
        };

        let clan_sidebar = cx.new(|cx| ClanSidebar::new(clan_list.clone(), cx));
        let channel_sidebar = cx.new(|cx| {
            ChannelSidebar::new(clan_list.clone(), channel_list.clone(), on_navigate, cx)
        });

        let user_info_bar = UserInfoBar::new(auth_state.clone(), on_settings);

        let _ = cx.observe(&auth_state, |_, _, cx| cx.notify());
        let _ = cx.observe(&channel_list, |_, _, cx| cx.notify());
        let _ = cx.observe(&clan_list, |_, _, cx| cx.notify());

        let api_clone = api.clone();
        let clan_list_clone = clan_list.clone();
        cx.spawn(async move |_, cx| {
            // Wait for connection to be fully ready (TCP open + handshake)
            loop {
                if api_clone.is_open().await && api_clone.ping_roundtrip().await.is_ok() {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1000))
                    .await;
            }

            tracing::info!("🔍 Investigation: Fetching real clan list...");
            match api_clone.list_clan_descs().await {
                Ok(clans) => {
                    tracing::info!("✅ Investigation: Fetched {} clans", clans.len());
                    if !clans.is_empty() {
                        let store_clans: Vec<Clan> = clans.into_iter().map(Clan::from).collect();

                        clan_list_clone.update(cx, |model, cx| {
                            model.update_clans(store_clans);
                            cx.notify();
                        });
                        tracing::info!("✅ Updated ClanList with real data");
                    }
                }
                Err(e) => {
                    tracing::error!("❌ Investigation: Failed to fetch clan list: {}", e);
                }
            }
        })
        .detach();

        Self {
            router,
            channel_list,
            clan_sidebar,
            channel_sidebar,
            user_info_bar,
        }
    }
}

impl Render for ChatLayout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let channels = self.channel_list.read(cx);

        let active_channel_name = channels.active_channel().cloned();

        let channel_header = div()
            .flex()
            .items_center()
            .h(px(44.0))
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg_tertiary)
            .text_sm()
            .text_color(theme.text_primary)
            .child(
                active_channel_name
                    .as_ref()
                    .map(|ch| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().font_weight(FontWeight::BOLD).child(ch.name.clone()))
                            .into_any_element()
                    })
                    .unwrap_or_else(|| div().child("Select a channel").into_any_element()),
            );

        let content = self.render_content(cx);

        div()
            .flex()
            .flex_row()
            .flex_1()
            .size_full()
            .bg(theme.bg_primary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(312.0))
                    .h_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .child(div().w(px(72.0)).h_full().child(self.clan_sidebar.clone()))
                            .child(
                                div()
                                    .w(px(240.0))
                                    .h_full()
                                    .child(self.channel_sidebar.clone()),
                            ),
                    )
                    .child(self.user_info_bar.render(&theme, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .bg(theme.bg_secondary)
                    .child(channel_header)
                    .child(content),
            )
    }
}

impl ChatLayout {
    fn render_content(&self, _cx: &Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let route = self.router.route();
        let current_path = self.router.current_path().to_string();

        let placeholder = match route {
            Route::Chat => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Inbox,
                "Chat",
                &current_path,
            ),
            Route::Direct => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::CircleUser,
                "Direct Messages",
                &current_path,
            ),
            Route::DirectMessage {
                direct_id,
                message_type: _,
            } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::CircleUser,
                &format!("Direct {direct_id}"),
                &current_path,
            ),
            Route::Channel {
                clan_id: _,
                channel_id,
            } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::FolderOpen,
                &format!("#{channel_id}"),
                &current_path,
            ),
            Route::SettingsAccount
            | Route::SettingsProfile
            | Route::SettingsDevices
            | Route::SettingsAppearance
            | Route::SettingsActivity
            | Route::SettingsNotifications
            | Route::SettingsLanguage
            | Route::SettingsVoice
            | Route::SettingsAdvanced
            | Route::NotFound { .. } => {
                // Handled by RootView, not rendered here
                div().into_any_element()
            }
        };

        div().flex_1().min_h_0().p_6().child(placeholder)
    }

    fn render_placeholder(
        &self,
        theme: Theme,
        icon: crate::components::primitives::IconName,
        title: &str,
        _path: &str,
    ) -> gpui::AnyElement {
        use crate::components::primitives::Icon;

        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .flex_col()
            .gap_4()
            .child(Icon::new(icon).size_8().text_color(theme.text_muted))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(title.to_string()),
            )
            .into_any_element()
    }
}
