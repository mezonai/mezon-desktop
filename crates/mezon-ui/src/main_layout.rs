use gpui::{Context, Entity, FontWeight, Window, div, prelude::*, px};
use mezon_store::{AuthState, ChannelList};

use crate::theme::Theme;

pub struct MainLayout {
    auth_state: Entity<AuthState>,
    channel_list: Entity<ChannelList>,
}

impl MainLayout {
    pub fn new(
        auth_state: Entity<AuthState>,
        channel_list: Entity<ChannelList>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Observe the channel list so we re-render when channel selection changes
        let _ = cx.observe(&channel_list, |_, _, cx| cx.notify());

        Self {
            auth_state,
            channel_list,
        }
    }
}

impl Render for MainLayout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let _username = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => session.username.clone(),
            _ => "Unknown".to_string(),
        };

        div()
            .flex()
            .flex_row()
            .flex_1()
            .bg(theme.bg_primary)
            .child(
                div().flex().flex_col().w(px(312.0)).h_full().child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        // .child(div().w(px(72.0)).h_full().child(self.clan_sidebar.clone()))
                        .child(div().w(px(240.0)).h_full()),
                ),
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
                                    let channels = self.channel_list.read(cx);
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
