use gpui::{Context, Entity, FontWeight, Window, div, prelude::*, px};
use mezon_store::{AuthState, ChannelType, ChannelsModel, ClansModel};

use crate::channel_sidebar::ChannelSidebar;
use crate::clan_sidebar::ClanSidebar;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

pub struct MainLayout {
    auth_state: Entity<AuthState>,
    channels_model: Entity<ChannelsModel>,
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
}

impl MainLayout {
    pub fn new(
        auth_state: Entity<AuthState>,
        channels_model: Entity<ChannelsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let clans = cx.new(|_cx| ClansModel::with_dummy_data());
        let clan_sidebar = cx.new(|cx| ClanSidebar::new(clans.clone(), cx));
        let channel_sidebar = cx.new(|cx| ChannelSidebar::new(clans, channels_model.clone(), cx));

        // Observe the channels model so we re-render when channel selection changes
        let _ = cx.observe(&channels_model, |_, _, cx| cx.notify());

        Self {
            auth_state,
            channels_model,
            clan_sidebar,
            channel_sidebar,
        }
    }
}

impl Render for MainLayout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let username = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => session.username.clone(),
            _ => "Unknown".to_string(),
        };

        div()
            .flex()
            .flex_row()
            .flex_1()
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
                    .child(UserInfoBar::new(&username).render(&theme)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .bg(theme.bg_secondary)
                    .child(
                        // CHANNEL HEADER BAR
                        div()
                            .flex()
                            .items_center()
                            .h(px(44.0)) // Consistent with other header heights
                            .px_4() // Standard horizontal padding
                            .border_b_1() // Bottom border for separation
                            .border_color(theme.border)
                            .bg(theme.bg_tertiary) // Slightly different bg for header
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(
                                // Get channel info
                                {
                                    let channels = self.channels_model.read(cx);
                                    channels
                                        .active_channel_id
                                        .as_ref()
                                        .and_then(|id| channels.find_channel(id))
                                        .cloned()
                                }
                                .map_or_else(
                                    || div().child("Select a channel").into_any_element(),
                                    |channel| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(if channel.channel_type == ChannelType::Text {
                                                div().child("#")
                                            } else {
                                                // Voice channel - use Speaker icon
                                                div().child(
                                                    Icon::new(IconName::Speaker).render(&theme),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .font_weight(FontWeight::BOLD)
                                                    .child(channel.name),
                                            )
                                            .into_any_element()
                                    },
                                ),
                            ),
                    )
                    .child(
                        // BIG EMPTY BOX FOR FUTURE CONTENT
                        div().flex_1(), // Take all remaining vertical space
                    ),
            )
    }
}
