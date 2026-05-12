pub mod account_test_view;
pub mod base_view;
pub mod components;
pub mod login_view;
pub mod main_layout;
pub mod root;
pub mod router;
pub mod theme;
pub mod title_bar;

pub use account_test_view::AccountTestView;
pub use base_view::BaseView;
pub use login_view::LoginView;
pub use main_layout::MainLayout;
pub use root::RootView;
pub use router::{Route, Router};
pub use theme::Theme;

pub fn init(cx: &mut gpui::App) {
    gpui_component::init(cx);
}
