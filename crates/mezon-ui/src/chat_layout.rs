use std::sync::Arc;

use gpui::{App, Context, Entity, Window, div, prelude::*, px};
use mezon_client::AppApi;
use mezon_store::{AuthState, Category, Channel, ChannelList, Clan, ClanList, Message, Settings};

use crate::chat_area::ChatArea;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::router::{Route, Router};
use crate::theme::{Theme, resolve_theme};
use crate::{ChannelSidebar, ClanSidebar};

/// Group flat channels into categories by `category_name`.
/// Channels with an empty `category_name` are placed into a "General" category.
fn group_channels_by_category(channels: Vec<Channel>) -> Vec<Category> {
    let mut map: std::collections::HashMap<String, Vec<Channel>> = std::collections::HashMap::new();

    for ch in channels {
        let cat_name = if ch.category_name.is_empty() {
            "General".to_string()
        } else {
            ch.category_name.clone()
        };
        map.entry(cat_name).or_default().push(ch);
    }

    let mut categories: Vec<Category> = map
        .into_iter()
        .map(|(name, chs)| {
            let clan_id = chs.first().map(|ch| ch.clan_id.clone()).unwrap_or_default();
            Category {
                clan_id,
                name,
                channels: chs,
            }
        })
        .collect();

    categories.sort_by(|a, b| a.name.cmp(&b.name));
    categories
}

fn spawn_clan_list_fetcher(api: Arc<AppApi>, clan_list: Entity<ClanList>, cx: &mut App) {
    cx.spawn(async move |cx| {
        tracing::info!("Fetching clan list...");
        match api.list_clan_descs().await {
            Ok(clans) => {
                tracing::info!("Fetched {} clans", clans.len());
                if !clans.is_empty() {
                    let store_clans: Vec<Clan> = clans.into_iter().map(Clan::from).collect();
                    clan_list.update(cx, |model, cx| {
                        model.update_clans(store_clans);
                        cx.notify();
                    });
                    tracing::info!("Updated ClanList with real data");
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch clan list: {}", e);
            }
        }
    })
    .detach();
}

fn spawn_channel_list_fetcher(
    api: Arc<AppApi>,
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let mut last_clan_id: Option<String> = None;
        let mut error_count: u32 = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;
        loop {
            let current_clan_id: Option<String> =
                cx.update(|app| clan_list.read(app).active_clan_id.clone());
            if current_clan_id.is_some() && current_clan_id != last_clan_id {
                if let Some(ref clan_id) = current_clan_id {
                    match api.list_channel_by_user_id().await {
                        Ok(api_channels) => {
                            error_count = 0;
                            let clan_channels: Vec<Channel> = api_channels
                                .into_iter()
                                .filter(|c| c.clan_id == *clan_id)
                                .map(Channel::from)
                                .collect();
                            let categories = group_channels_by_category(clan_channels);
                            channel_list.update(cx, |list, cx| {
                                list.categories = categories;
                                cx.notify();
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to fetch channels: {}", e);
                            error_count += 1;
                            if error_count >= MAX_CONSECUTIVE_ERRORS {
                                tracing::error!(
                                    "Too many consecutive channel fetch failures, stopping watcher"
                                );
                                break;
                            }
                        }
                    }
                }
                last_clan_id = current_clan_id;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
        }
    })
    .detach();
}

pub struct ChatLayout {
    router: Router,
    settings: Entity<Settings>,
    channel_list: Entity<ChannelList>,
    pub chat_area: ChatArea,
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
    user_info_bar: UserInfoBar,
    /// Guard: spawn data fetchers only on the first render call.
    fetchers_spawned: bool,
    api: Arc<AppApi>,
    clan_list: Entity<ClanList>,
    auth_state: Entity<AuthState>,
    last_fetched_channel_id: Option<String>,
}

pub struct ChatLayoutParams {
    pub router: Router,
    pub clan_list: Entity<ClanList>,
    pub channel_list: Entity<ChannelList>,
    pub auth_state: Entity<AuthState>,
    pub api: Arc<AppApi>,
    pub navigate: crate::components::NavigateFn,
    pub settings: Entity<Settings>,
}

impl ChatLayout {
    pub fn new(params: ChatLayoutParams, cx: &mut Context<Self>) -> Self {
        let ChatLayoutParams {
            router,
            clan_list,
            channel_list,
            auth_state,
            api,
            navigate,
            settings,
        } = params;

        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());

        let on_navigate: Option<crate::components::NavigateFn> = {
            let nav = navigate.clone();
            Some(Arc::new(move |op, cx| nav(op, cx)))
        };

        let on_settings: Option<crate::components::NavigateFn> = {
            let nav = navigate.clone();
            Some(Arc::new(move |op, cx| nav(op, cx)))
        };

        let clan_list_for_sidebar = clan_list.clone();
        let settings_for_clan = settings.clone();
        let clan_sidebar =
            cx.new(move |cx| ClanSidebar::new(clan_list_for_sidebar, settings_for_clan, cx));

        let clan_list_for_channel = clan_list.clone();
        let channel_list_for_channel = channel_list.clone();
        let settings_for_channel = settings.clone();
        let channel_sidebar = cx.new(move |cx| {
            ChannelSidebar::new(
                clan_list_for_channel,
                channel_list_for_channel,
                on_navigate,
                settings_for_channel,
                cx,
            )
        });

        let user_info_bar = UserInfoBar::new(auth_state.clone(), on_settings);

        let _ = cx.observe(&auth_state, |_, _, cx| cx.notify());
        let _ = cx.observe(&channel_list, |_, _, cx| cx.notify());
        let _ = cx.observe(&clan_list, |_, _, cx| cx.notify());

        Self {
            router,
            settings,
            channel_list,
            chat_area: ChatArea::new(),
            clan_sidebar,
            channel_sidebar,
            user_info_bar,
            fetchers_spawned: false,
            api,
            clan_list,
            auth_state,
            last_fetched_channel_id: None,
        }
    }
}

impl Render for ChatLayout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);

        if !self.fetchers_spawned {
            self.fetchers_spawned = true;
            spawn_clan_list_fetcher(self.api.clone(), self.clan_list.clone(), cx);
            spawn_channel_list_fetcher(
                self.api.clone(),
                self.clan_list.clone(),
                self.channel_list.clone(),
                cx,
            );
        }

        let active_ch = self.channel_list.read(cx).active_channel().cloned();
        if let Some(ref ch) = active_ch {
            let prev_id = self.last_fetched_channel_id.clone();
            if Some(&ch.id) != prev_id.as_ref() {
                self.last_fetched_channel_id = Some(ch.id.clone());
                let api = self.api.clone();
                let ch_id = ch.id.clone();
                let cl_id = ch.clan_id.clone();
                cx.spawn(
                    async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| match api
                        .list_channel_messages(&cl_id, &ch_id, 20)
                        .await
                    {
                        Ok(msgs) => {
                            tracing::info!("Fetched {} messages for channel {}", msgs.len(), ch_id);
                            let mut store_msgs: Vec<Message> = msgs
                                .into_iter()
                                .map(|m| {
                                    Message::new(
                                        m.message_id,
                                        m.content,
                                        m.sender_id,
                                        m.sender_name,
                                        m.create_time,
                                    )
                                })
                                .collect();
                            store_msgs.sort_by_key(|m| m.create_time);
                            let fetched_ch_id = ch_id.clone();
                            let _ = this.update(cx, |this, cx| {
                                if this.last_fetched_channel_id.as_deref() != Some(&fetched_ch_id) {
                                    return;
                                }
                                this.chat_area.messages = store_msgs;
                                cx.notify();
                            });
                        }
                        Err(e) => tracing::error!("Failed to fetch messages for {ch_id}: {e}"),
                    },
                )
                .detach();
            }
        }

        self.chat_area.ensure_input(_window, cx);
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
                    .child(content),
            )
    }
}

impl ChatLayout {
    fn render_content(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);

        let session_user_id = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => session.user_id.clone(),
            _ => String::new(),
        };

        // Use channel_list.active_channel_id to detect channel selection instead
        // of self.router.route(), because the router clone in ChatLayout is stale
        // (only the RootView's router gets updated on navigation).
        let channels = self.channel_list.read(cx);
        if let Some(ch) = channels.active_channel() {
            return self
                .chat_area
                .render(&theme, cx.entity(), &ch.name, &session_user_id)
                .into_any_element();
        }

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

        div()
            .flex_1()
            .min_h_0()
            .p_6()
            .child(placeholder)
            .into_any_element()
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
