pub mod app;
pub mod auth;
pub mod chat;
pub mod clan;
pub mod command_palette;
pub mod components;
pub mod dev;
pub mod gallery;
pub mod image_cache;
pub mod image_viewer;
pub mod router;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod util;
pub mod window_layout;

pub use app::root::RootView;
pub use app::shell::Shell;
pub use app::title_bar::TitleBar;
pub use auth::login_view::LoginView;
pub use chat::layout::ChatLayout;
pub use dev::gallery::DevGallery;
pub use gallery::GalleryModal;
pub use image_viewer::{OpenViewerRequest, open_image_viewer};
pub use router::{Route, Router};
pub use settings::SettingsScreen;
pub use sidebar::channel_sidebar::ChannelSidebar;
pub use sidebar::clan_sidebar::ClanSidebar;
pub use sidebar::direct_sidebar::DirectSidebar;
pub use theme::Theme;
pub use theme::tokens::ThemeTokens;
pub use window_layout::{
    MAIN_WINDOW_DEFAULT_HEIGHT, MAIN_WINDOW_DEFAULT_WIDTH, MAIN_WINDOW_MIN_HEIGHT,
    MAIN_WINDOW_MIN_WIDTH,
};

pub(crate) const SHOW_UNREAD_BADGE_COUNT: bool = true;

gpui::actions!(
    mezon,
    [
        ToggleInspector,
        Quit,
        HideWindow,
        MinimizeWindow,
        HideApp,
        OpenCommandPalette
    ]
);

#[macro_export]
macro_rules! trace_render {
    ($name:expr) => {
        $crate::trace_render!("{}", $name)
    };
    ($fmt:expr, $($arg:tt)+) => {{
        #[cfg(debug_assertions)]
        {
            static __RENDER_N: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
            ::tracing::trace!(
                target: "render",
                "{} #{}",
                ::std::format_args!($fmt, $($arg)+),
                __RENDER_N.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed)
            );
        }
    }};
}

pub fn init(cx: &mut gpui::App) {
    ::theme::init(::theme::LoadThemes::JustBase, cx);
    theme::init_theme_settings_provider(cx);
    cx.bind_keys([gpui::KeyBinding::new(
        "escape",
        ::menu::Cancel,
        Some("menu"),
    )]);
    #[cfg(debug_assertions)]
    cx.bind_keys([gpui::KeyBinding::new("cmd-alt-i", ToggleInspector, None)]);
    cx.bind_keys([gpui::KeyBinding::new(
        "secondary-k",
        OpenCommandPalette,
        None,
    )]);
    cx.on_action(|_: &OpenCommandPalette, cx: &mut gpui::App| {
        command_palette::CommandPaletteModal::try_toggle_authenticated(cx);
    });
    components::primitives::init_input(cx);
    chat::mention_input::init(cx);
    chat::message_search::init(cx);
    command_palette::init(cx);
    router::Router::init(cx);
    init_menus(cx);
}

/// macOS menu bar and standard shortcuts. Edit items reuse the input component's clipboard actions.
fn init_menus(cx: &mut gpui::App) {
    use crate::components::primitives::input::{Copy, Cut, Paste, SelectAll};

    #[cfg(target_os = "macos")]
    init_macos_menu_actions(cx);

    #[cfg(not(target_os = "macos"))]
    {
        use gpui::KeyBinding;

        cx.on_action(|_: &Quit, cx: &mut gpui::App| cx.quit());
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
    }

    cx.set_menus(app_menus(Cut, Copy, Paste, SelectAll));
}

#[cfg(target_os = "macos")]
fn init_macos_menu_actions(cx: &mut gpui::App) {
    use crate::app::window_controls::macos;
    use gpui::{App, KeyBinding};

    macos::install_shortcuts(cx);

    // Menu clicks dispatch GPUI actions; keyboard shortcuts are handled in `install_shortcuts`.
    // Global handlers satisfy macOS menu validation (`is_action_available`).
    cx.on_action(|_: &HideWindow, cx: &mut App| macos::hide_active_window(cx));
    cx.on_action(|_: &MinimizeWindow, cx: &mut App| macos::minimize_active_window(cx));
    cx.on_action(|_: &HideApp, cx: &mut App| cx.hide());
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());

    // Key bindings label menu items; shortcuts are handled natively on macOS.
    cx.bind_keys([
        KeyBinding::new("cmd-w", HideWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

fn app_menus(
    cut: crate::components::primitives::input::Cut,
    copy: crate::components::primitives::input::Copy,
    paste: crate::components::primitives::input::Paste,
    select_all: crate::components::primitives::input::SelectAll,
) -> Vec<gpui::Menu> {
    use gpui::{Menu, MenuItem};

    let edit = Menu::new("Edit").items(edit_menu_items(cut, copy, paste, select_all));

    #[cfg(target_os = "macos")]
    {
        vec![
            Menu::new("Mezon").items([
                MenuItem::action("Hide Mezon", HideApp),
                MenuItem::separator(),
                MenuItem::action("Quit Mezon", Quit),
            ]),
            edit,
            Menu::new("Window").items([
                MenuItem::action("Minimize", MinimizeWindow),
                MenuItem::action("Close Window", HideWindow),
            ]),
        ]
    }

    #[cfg(not(target_os = "macos"))]
    {
        vec![
            Menu::new("Mezon").items([MenuItem::action("Quit Mezon", Quit)]),
            edit,
        ]
    }
}

fn edit_menu_items(
    cut: crate::components::primitives::input::Cut,
    copy: crate::components::primitives::input::Copy,
    paste: crate::components::primitives::input::Paste,
    select_all: crate::components::primitives::input::SelectAll,
) -> [gpui::MenuItem; 5] {
    use gpui::{MenuItem, OsAction};

    [
        MenuItem::os_action("Cut", cut, OsAction::Cut),
        MenuItem::os_action("Copy", copy, OsAction::Copy),
        MenuItem::os_action("Paste", paste, OsAction::Paste),
        MenuItem::separator(),
        MenuItem::os_action("Select All", select_all, OsAction::SelectAll),
    ]
}
