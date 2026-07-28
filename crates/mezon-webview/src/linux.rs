use std::cell::RefCell;
use std::ffi::c_ulong;
use std::mem;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use gtk::prelude::WidgetExtManual;
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XcbWindowHandle, XlibWindowHandle,
};
use webkit2gtk::WebViewExt;
use wry::{WebContext, WebViewBuilder, WebViewExtUnix};

use crate::webview::ChannelAppWebView;

static GTK_INIT: Once = Once::new();
static ACTIVE_WEBVIEW_COUNT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static SHARED_WEB_CONTEXT: RefCell<Option<WebContext>> = RefCell::new(None);
}

pub fn active_webview_count() -> usize {
    ACTIVE_WEBVIEW_COUNT.load(Ordering::Relaxed)
}

pub fn with_shared_web_context<R>(f: impl FnOnce(&mut WebContext) -> Result<R>) -> Result<R> {
    init_gtk()?;
    SHARED_WEB_CONTEXT.with(|cell| {
        let mut context = cell.borrow_mut();
        if context.is_none() {
            *context = Some(WebContext::new(None));
        }
        f(context.as_mut().expect("shared web context initialized"))
    })
}

pub fn init_gtk() -> Result<()> {
    GTK_INIT.call_once(|| unsafe {
        std::env::set_var("GDK_BACKEND", "x11");
    });
    if gtk::is_initialized() {
        return Ok(());
    }
    gtk::init().context("gtk init failed")?;
    Ok(())
}

pub fn pump_gtk_events() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

pub fn destroy_webview(webview: ChannelAppWebView) {
    let gtk_webview = webview.webview();
    gtk_webview.try_close();
    if let Err(error) = webview.set_visible(false) {
        tracing::warn!("channel app webview hide failed: {error:#}");
    }
    pump_gtk_events();

    let gtk_window = wry_gtk_window(&gtk_webview);
    unsafe {
        gtk_webview.destroy();
    }
    pump_gtk_events();

    if let Some(gtk_window) = gtk_window {
        unsafe {
            gtk_window.destroy();
        }
        pump_gtk_events();
    }

    sync_x11_display();

    // Manual GTK destroy above already tore down the native widgets. Dropping the
    // wry WebView would destroy them again and crash; leak the Rust shell instead.
    // Tradeoff: small Rust allocation leak per close vs. double-free abort.
    let _ = ACTIVE_WEBVIEW_COUNT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_sub(1))
    });
    mem::forget(webview);
}

fn wry_gtk_window(gtk_webview: &webkit2gtk::WebView) -> Option<gtk::Window> {
    use gtk::prelude::{Cast, WidgetExt};

    let mut widget = gtk_webview.clone().upcast::<gtk::Widget>();
    while let Some(parent) = widget.parent() {
        if let Ok(window) = parent.clone().downcast::<gtk::Window>() {
            return Some(window);
        }
        widget = parent;
    }
    None
}

fn sync_x11_display() {
    if let Some(display) = gtk::gdk::Display::default() {
        display.sync();
    }
}

pub fn create(
    parent: &impl HasWindowHandle,
    builder: WebViewBuilder<'_>,
) -> Result<ChannelAppWebView> {
    init_gtk()?;
    pump_gtk_events();

    let handle = parent.window_handle()?.as_raw();
    let webview = match handle {
        RawWindowHandle::Xcb(xcb) => create_x11_child(&xcb, builder)?,
        RawWindowHandle::Xlib(_) => builder
            .build_as_child(parent)
            .map(ChannelAppWebView::new)
            .context("Failed to create channel app webview")?,
        RawWindowHandle::Wayland(_) => bail!(
            "Channel app webviews require the X11 backend on Linux. \
             Restart with DISPLAY set and unset WAYLAND_DISPLAY."
        ),
        _ => bail!("Failed to create channel app webview: the window handle kind is not supported"),
    };
    ACTIVE_WEBVIEW_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(webview)
}

fn create_x11_child(
    xcb: &XcbWindowHandle,
    builder: WebViewBuilder<'_>,
) -> Result<ChannelAppWebView> {
    let xlib_parent = XlibParentWindow::from_xcb(xcb);
    builder
        .build_as_child(&xlib_parent)
        .map(ChannelAppWebView::new)
        .context("Failed to create channel app webview")
}

struct XlibParentWindow {
    handle: XlibWindowHandle,
}

impl XlibParentWindow {
    fn from_xcb(xcb: &XcbWindowHandle) -> Self {
        let mut handle = XlibWindowHandle::new(xcb.window.get() as c_ulong);
        if let Some(visual_id) = xcb.visual_id {
            handle.visual_id = visual_id.get() as c_ulong;
        }
        Self { handle }
    }
}

impl HasWindowHandle for XlibParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Xlib(self.handle)) })
    }
}
