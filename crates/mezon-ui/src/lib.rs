pub mod account_test_view;
pub mod base_view;
pub mod components;
pub mod login_view;
pub mod root;
pub mod router;
pub mod theme;
pub mod title_bar;
pub mod view_lifecycle;

pub use account_test_view::AccountTestView;
pub use base_view::BaseView;
pub use login_view::LoginView;
pub use root::RootView;
pub use router::{Route, Router};
pub use theme::Theme;
pub use view_lifecycle::{LifecycleSubscriptions, ViewLifecycle, ViewLifecycleContext};

pub fn init(cx: &mut gpui::App) {
    gpui_component::init(cx);
}
