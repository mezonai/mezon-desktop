mod tool_bars;
mod webview;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use tool_bars::{current_url, go_back, go_forward, reload};
pub use webview::{
    ChannelAppWebView, ChannelAppWebViewBounds, WEBVIEW_BOTTOM_OFFSET, WEBVIEW_TOP_OFFSET,
    create_as_window, destroy_webview, resize_webview, validate_http_url,
};

#[cfg(target_os = "windows")]
pub use windows::{create_for_hwnd as create_for_win32_hwnd, win32_parent_hwnd};

#[cfg(target_os = "linux")]
pub use linux::{active_webview_count, init_gtk, pump_gtk_events};
