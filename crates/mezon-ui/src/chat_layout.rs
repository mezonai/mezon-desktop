use std::sync::Arc;

use gpui::{App, Context, Entity, FontWeight, Window, div, prelude::*, px};
use mezon_client::AppApi;
use mezon_store::{AuthState, Category, Channel, ChannelList, Clan, ClanList};

use crate::chat_area::ChatArea;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::router::{Route, Router};
use crate::theme::Theme;
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
    channel_list: Entity<ChannelList>,
    pub chat_area: ChatArea,
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
    user_info_bar: UserInfoBar,
    /// Guard: spawn data fetchers only on the first render call.
    fetchers_spawned: bool,
    api: Arc<AppApi>,
    clan_list: Entity<ClanList>,
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

        Self {
            router,
            channel_list,
            chat_area: ChatArea::new(),
            clan_sidebar,
            channel_sidebar,
            user_info_bar,
            fetchers_spawned: false,
            api,
            clan_list,
        }
    }
}

impl Render for ChatLayout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(channel_header)
                    .child(content),
            )
    }
}

impl ChatLayout {
    fn render_content(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = Theme::dark();

        // Use channel_list.active_channel_id to detect channel selection instead
        // of self.router.route(), because the router clone in ChatLayout is stale
        // (only the RootView's router gets updated on navigation).
        if self.channel_list.read(cx).active_channel_id.is_some() {
            return self
                .chat_area
                .render(&theme, cx.entity())
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
            Route::Settings | Route::NotFound { .. } => {
                // Handled by RootView, not rendered here
                div().into_any_element()
            }
            _ => unreachable!(),
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
