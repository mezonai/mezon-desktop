use gpui::{Context, Entity, Window, div, prelude::*, px};
use mezon_store::{ChannelsModel, ClansModel};

use crate::channel_sidebar::ChannelSidebar;
use crate::clan_sidebar::ClanSidebar;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::theme::Theme;

pub struct MainLayout {
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
}

impl MainLayout {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let clans = cx.new(|_cx| ClansModel::with_dummy_data());
        let channels = cx.new(|_cx| ChannelsModel::with_dummy_data());
        let clan_sidebar = cx.new(|cx| ClanSidebar::new(clans.clone(), cx));
        let channel_sidebar = cx.new(|cx| ChannelSidebar::new(clans, channels, cx));

        Self {
            clan_sidebar,
            channel_sidebar,
        }
    }
}

impl Render for MainLayout {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

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
                    .child(UserInfoBar::new("player1").render(&theme)),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .bg(theme.bg_secondary)
                    .child("ContentArea placeholder"),
            )
    }
}
