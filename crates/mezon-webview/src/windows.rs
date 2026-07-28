use anyhow::{Context, Result, bail};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle};
use std::num::NonZeroIsize;
use wry::WebViewBuilder;

use crate::webview::{
    ChannelAppWebView, ChannelAppWebViewBounds, configure_builder, webview_bounds,
};

struct Win32Parent(isize);

impl HasWindowHandle for Win32Parent {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        let raw = Win32WindowHandle::new(hwnd).into();
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

pub fn win32_parent_hwnd(parent: &impl HasWindowHandle) -> Option<isize> {
    match parent.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as isize),
        _ => None,
    }
}

pub(crate) fn create(
    parent: &impl HasWindowHandle,
    builder: WebViewBuilder<'_>,
) -> Result<ChannelAppWebView> {
    match parent.window_handle()?.as_raw() {
        RawWindowHandle::Win32(_) => builder
            .build_as_child(parent)
            .map(ChannelAppWebView::new)
            .context("Failed to create channel app webview"),
        _ => bail!("Failed to create channel app webview: the window handle kind is not supported"),
    }
}

/// Create a child webview from a raw Win32 HWND.
/// Win32 message loop and can re-enter GPUI window handlers.
pub fn create_for_hwnd(
    hwnd: isize,
    url: &str,
    bounds: ChannelAppWebViewBounds,
) -> Result<ChannelAppWebView> {
    if hwnd == 0 {
        bail!("Failed to create channel app webview: parent HWND is null");
    }
    let builder = configure_builder(url, webview_bounds(bounds.width, bounds.height))?;
    create(&Win32Parent(hwnd), builder)
}
