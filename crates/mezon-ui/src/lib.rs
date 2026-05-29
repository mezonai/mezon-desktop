pub mod account_test_view;
pub mod base_view;
pub mod channel_sidebar;
pub mod chat_layout;
pub mod clan_sidebar;
pub mod components;
pub mod login_view;
pub mod main_layout;
pub mod root;
pub mod router;
pub mod settings;
pub mod text_utils;
pub mod theme;
pub mod title_bar;

pub use account_test_view::AccountTestView;
pub use base_view::BaseView;
pub use channel_sidebar::ChannelSidebar;
pub use chat_layout::ChatLayout;
pub use clan_sidebar::ClanSidebar;
pub use login_view::LoginView;
pub use root::RootView;
pub use router::{Route, Router};
pub use settings::SettingsScreen;
pub use theme::Theme;

pub fn init(cx: &mut gpui::App) {
    gpui_component::init(cx);
}
