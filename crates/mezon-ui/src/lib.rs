pub mod app;
pub mod auth;
pub mod canvas_navigation;
pub mod channel_app;
pub mod channel_navigation;
pub mod chat;
pub mod clan;
pub mod command_palette;
pub mod components;
pub mod dev;
pub mod gallery;
pub mod image_cache;
pub mod image_viewer;
pub mod invite;
pub mod pdf_viewer;
pub mod router;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod tour;
pub mod util;
pub mod window_layout;

pub use mezon_widgets::clipboard;

pub use app::root::RootView;
pub use app::shell::Shell;
pub use app::threads_toast::ThreadCreateToastBridge;
pub use app::title_bar::TitleBar;
pub use app::wallet_toast::WalletToastBridge;
pub use auth::login_view::LoginView;
pub use channel_app::launch_channel_app_from_store;
pub use chat::layout::ChatLayout;
pub use dev::gallery::DevGallery;
pub use gallery::GalleryModal;
pub use image_viewer::{OpenViewerRequest, open_image_viewer};
pub use pdf_viewer::{OpenPdfRequest, open_pdf_viewer};
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

pub(crate) const APP_SHELL_KEY_CONTEXT: &str = "AppShell";
pub(crate) const VIDEO_PLAYER_KEY_CONTEXT: &str = "VideoPlayer";
const MENU_KEY_CONTEXT: &str = "menu";
const MODAL_BACKDROP_KEY_CONTEXT: &str = "modal_backdrop";

gpui::actions!(
    mezon,
    [
        ToggleInspector,
        Quit,
        HideWindow,
        MinimizeWindow,
        HideApp,
        OpenCommandPalette,
        GoBack,
        GoForward,
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
    cx.bind_keys([gpui::KeyBinding::new(
        "escape",
        ::menu::Cancel,
        Some("modal_backdrop"),
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
    install_history_navigation(cx);
    components::primitives::init_text_input(cx);
    components::primitives::init_focus_cycle(cx);
    chat::mention_input::init(cx);
    mezon_canvas::init(cx);
    canvas_navigation::init(cx);
    chat::message_search::init(cx);
    command_palette::init(cx);
    tour::init(cx);
    router::Router::init(cx);
    init_menus(cx);
}

fn history_keystrokes() -> &'static [(&'static str, bool)] {
    #[cfg(target_os = "macos")]
    {
        &[
            ("cmd-left", true),
            ("cmd-right", false),
            ("cmd-[", true),
            ("cmd-]", false),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        &[("alt-left", true), ("alt-right", false)]
    }
}

fn history_navigation_scope() -> String {
    format!(
        "{APP_SHELL_KEY_CONTEXT} && !{MENU_KEY_CONTEXT} && !{MODAL_BACKDROP_KEY_CONTEXT} && !{} && !{}",
        crate::command_palette::KEY_CONTEXT,
        crate::tour::KEY_CONTEXT,
    )
}

fn history_navigation_bindings() -> Vec<gpui::KeyBinding> {
    let scope = history_navigation_scope();
    let canvas = mezon_canvas::CANVAS_EDITING_KEY_CONTEXT;
    let mut bindings = Vec::with_capacity(history_keystrokes().len() * 2);
    for &(key, back) in history_keystrokes() {
        if back {
            bindings.push(gpui::KeyBinding::new(key, GoBack, Some(scope.as_str())));
        } else {
            bindings.push(gpui::KeyBinding::new(key, GoForward, Some(scope.as_str())));
        }
        bindings.push(gpui::KeyBinding::new(key, gpui::NoAction, Some(canvas)));
    }
    bindings
}

fn install_history_navigation(cx: &mut gpui::App) {
    cx.bind_keys(history_navigation_bindings());
    cx.on_action(|_: &GoBack, cx: &mut gpui::App| {
        router::go_back(cx);
    });
    cx.on_action(|_: &GoForward, cx: &mut gpui::App| {
        router::go_forward(cx);
    });
}

/// macOS menu bar and standard shortcuts. Edit items reuse the input component's clipboard actions.
fn init_menus(cx: &mut gpui::App) {
    use crate::components::primitives::text_actions::{Copy, Cut, Paste, SelectAll};

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
    cut: crate::components::primitives::text_actions::Cut,
    copy: crate::components::primitives::text_actions::Copy,
    paste: crate::components::primitives::text_actions::Paste,
    select_all: crate::components::primitives::text_actions::SelectAll,
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
    cut: crate::components::primitives::text_actions::Cut,
    copy: crate::components::primitives::text_actions::Copy,
    paste: crate::components::primitives::text_actions::Paste,
    select_all: crate::components::primitives::text_actions::SelectAll,
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

#[cfg(test)]
mod history_navigation_tests {
    use super::{
        APP_SHELL_KEY_CONTEXT, GoBack, GoForward, MENU_KEY_CONTEXT, MODAL_BACKDROP_KEY_CONTEXT,
        VIDEO_PLAYER_KEY_CONTEXT, history_navigation_bindings,
    };
    use crate::command_palette::KEY_CONTEXT as COMMAND_PALETTE_KEY_CONTEXT;
    use crate::tour::KEY_CONTEXT as TOUR_KEY_CONTEXT;
    use gpui::{Action, KeyContext, Keymap, Keystroke};
    use mezon_canvas::CANVAS_EDITING_KEY_CONTEXT;
    use mezon_widgets::text_actions::{TEXT_INPUT_CONTEXT, text_input_bindings};

    fn matches(keymap: &Keymap, key: &str, contexts: &[KeyContext], action: &dyn Action) -> bool {
        let keystroke = Keystroke::parse(key).unwrap_or_else(|_| panic!("parse {key}"));
        let (bindings, pending) =
            keymap.bindings_for_input(std::slice::from_ref(&keystroke), contexts);
        assert!(!pending, "{key} must not wait for another key");
        bindings
            .first()
            .is_some_and(|binding| binding.action().partial_eq(action))
    }

    fn context_with(identifiers: &[&str]) -> KeyContext {
        let mut context = KeyContext::default();
        for identifier in identifiers {
            context.add(*identifier);
        }
        context
    }

    fn app_shell_context() -> KeyContext {
        context_with(&[APP_SHELL_KEY_CONTEXT])
    }

    fn text_context() -> Vec<KeyContext> {
        vec![app_shell_context(), context_with(&[TEXT_INPUT_CONTEXT])]
    }

    fn video_player_context() -> Vec<KeyContext> {
        vec![
            app_shell_context(),
            context_with(&[VIDEO_PLAYER_KEY_CONTEXT]),
        ]
    }

    fn keymap_after_text_editing() -> Keymap {
        let mut keymap = Keymap::new(history_navigation_bindings());
        keymap.add_bindings(text_input_bindings());
        keymap
    }

    #[test]
    fn browser_history_keys_navigate_within_the_app_shell() {
        let keymap = Keymap::new(history_navigation_bindings());
        let app_shell = &[app_shell_context()];

        #[cfg(target_os = "macos")]
        {
            assert!(matches(&keymap, "cmd-left", app_shell, &GoBack));
            assert!(matches(&keymap, "cmd-right", app_shell, &GoForward));
            assert!(matches(&keymap, "cmd-[", app_shell, &GoBack));
            assert!(matches(&keymap, "cmd-]", app_shell, &GoForward));
            assert!(!matches(&keymap, "alt-left", app_shell, &GoBack));
            assert!(!matches(&keymap, "alt-right", app_shell, &GoForward));
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert!(matches(&keymap, "alt-left", app_shell, &GoBack));
            assert!(matches(&keymap, "alt-right", app_shell, &GoForward));
            assert!(!matches(&keymap, "ctrl-left", app_shell, &GoBack));
            assert!(!matches(&keymap, "ctrl-right", app_shell, &GoForward));
        }
    }

    #[test]
    fn history_keys_never_fire_in_detached_viewer_windows() {
        let keymap = Keymap::new(history_navigation_bindings());
        let detached_window: &[KeyContext] = &[];

        #[cfg(target_os = "macos")]
        {
            assert!(!matches(&keymap, "cmd-left", detached_window, &GoBack));
            assert!(!matches(&keymap, "cmd-right", detached_window, &GoForward));
            assert!(!matches(&keymap, "cmd-[", detached_window, &GoBack));
            assert!(!matches(&keymap, "cmd-]", detached_window, &GoForward));
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert!(!matches(&keymap, "alt-left", detached_window, &GoBack));
            assert!(!matches(&keymap, "alt-right", detached_window, &GoForward));
        }
    }

    #[test]
    fn history_shortcuts_still_navigate_while_a_video_is_focused() {
        let keymap = Keymap::new(history_navigation_bindings());
        let video = video_player_context();

        #[cfg(target_os = "macos")]
        {
            assert!(matches(&keymap, "cmd-[", &video, &GoBack));
            assert!(matches(&keymap, "cmd-]", &video, &GoForward));
            assert!(!matches(&keymap, "left", &video, &GoBack));
            assert!(!matches(&keymap, "right", &video, &GoForward));
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert!(matches(&keymap, "alt-left", &video, &GoBack));
            assert!(matches(&keymap, "alt-right", &video, &GoForward));
            assert!(!matches(&keymap, "left", &video, &GoBack));
            assert!(!matches(&keymap, "right", &video, &GoForward));
        }
    }

    #[test]
    fn history_keys_do_not_change_route_behind_an_overlay() {
        let keymap = Keymap::new(history_navigation_bindings());
        let key = if cfg!(target_os = "macos") {
            "cmd-["
        } else {
            "alt-left"
        };
        for context in [
            MENU_KEY_CONTEXT,
            MODAL_BACKDROP_KEY_CONTEXT,
            COMMAND_PALETTE_KEY_CONTEXT,
            TOUR_KEY_CONTEXT,
            CANVAS_EDITING_KEY_CONTEXT,
        ] {
            let stack = vec![app_shell_context(), context_with(&[context])];
            assert!(
                !matches(&keymap, key, &stack, &GoBack),
                "{key} must not go back inside {context}"
            );
        }
    }

    #[test]
    fn history_keys_follow_browser_text_editing_exceptions() {
        let keymap = keymap_after_text_editing();
        let text = text_context();

        #[cfg(target_os = "macos")]
        {
            use mezon_widgets::text_actions::{End, Home};
            assert!(matches(&keymap, "cmd-left", &text, &Home));
            assert!(matches(&keymap, "cmd-right", &text, &End));
            assert!(!matches(&keymap, "cmd-left", &text, &GoBack));
            assert!(!matches(&keymap, "cmd-right", &text, &GoForward));
            assert!(matches(&keymap, "cmd-[", &text, &GoBack));
            assert!(matches(&keymap, "cmd-]", &text, &GoForward));
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert!(matches(&keymap, "alt-left", &text, &GoBack));
            assert!(matches(&keymap, "alt-right", &text, &GoForward));
        }
    }
}
