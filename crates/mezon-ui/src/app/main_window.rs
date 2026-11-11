use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, DisplayId, Entity, Global, Window, WindowBounds,
    WindowHandle, WindowKind, px, size,
};
use mezon_store::{AutoUpdateStore, Settings};

pub const BASE_REM_SIZE: f32 = 16.0;
const MIN_ZOOM_FACTOR: f32 = 0.8;
const MAX_ZOOM_FACTOR: f32 = 1.5;

struct MainWindowHandle(AnyWindowHandle);
impl Global for MainWindowHandle {}

pub fn apply_ui_zoom(window: &mut Window, zoom_factor: f32) {
    let zoom = zoom_factor.clamp(MIN_ZOOM_FACTOR, MAX_ZOOM_FACTOR);
    window.set_rem_size(px(BASE_REM_SIZE * zoom));
}

pub fn install_ui_zoom_observer(settings: Entity<Settings>, window: &mut Window, cx: &mut App) {
    apply_ui_zoom(window, settings.read(cx).zoom_factor);
    let handle = window.window_handle();
    cx.observe(&settings, move |settings, cx| {
        let zoom = settings.read(cx).zoom_factor;
        let _ = handle.update(cx, |_, window, _| apply_ui_zoom(window, zoom));
    })
    .detach();
}

pub fn register_main_window(handle: AnyWindowHandle, cx: &mut App) {
    if cx.try_global::<MainWindowHandle>().is_some() {
        tracing::warn!("register_main_window called more than once");
    }
    cx.set_global(MainWindowHandle(handle));
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    register_portal_parent_window(handle, cx);
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn register_portal_parent_window(handle: AnyWindowHandle, cx: &mut App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let xid = cx.update_window(handle, |_, window, _| -> Option<u32> {
        match HasWindowHandle::window_handle(&*window).ok()?.as_raw() {
            RawWindowHandle::Xcb(xcb) => Some(xcb.window.get()),
            RawWindowHandle::Xlib(xlib) => u32::try_from(xlib.window).ok(),
            _ => None,
        }
    });
    if let Ok(Some(xid)) = xid {
        mezon_voice::set_portal_parent_window(xid);
    }
}

pub fn handle(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<MainWindowHandle>().map(|g| g.0)
}

/// Read a placement straight off a live `Window`. Callers that already hold the main window —
/// every click handler does — must use this rather than [`window_placement_for`]: reaching for
/// the main window through `cx.update_window` while that same window is mid-update fails, and the
/// resulting `None` silently downgrades an overlay to [`overlay_fallback_placement`].
pub fn overlay_placement_from_window(
    window: &Window,
    cx: &App,
) -> (WindowBounds, Option<DisplayId>) {
    let display_id = window.display(cx).map(|d| d.id());
    let bounds = window.bounds();
    let window_bounds = if window.is_fullscreen() {
        WindowBounds::Fullscreen(bounds)
    } else if window.is_maximized() {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    };
    (window_bounds, display_id)
}

pub fn window_placement_for(
    handle: AnyWindowHandle,
    cx: &mut App,
) -> Option<(WindowBounds, Option<DisplayId>)> {
    cx.update_window(handle, |_, window, cx| {
        overlay_placement_from_window(window, cx)
    })
    .ok()
}

pub fn main_window_placement(cx: &mut App) -> Option<(WindowBounds, Option<DisplayId>)> {
    let handle = handle(cx)?;
    window_placement_for(handle, cx)
}

pub fn activate_main_window(cx: &mut App) {
    let Some(handle) = handle(cx) else {
        return;
    };
    cx.activate(true);
    let _ = cx.update_window(handle, |_, window, _| window.activate_window());
    if let Some(store) = AutoUpdateStore::try_global(cx) {
        store.update(cx, |s, cx| s.set_main_window_hidden(false, cx));
    }
}

/// Wayland has no client-side positioning, so an overlay can only be tied to the main window by
/// asking the compositor to treat it as a transient child. GPUI parents `Floating` windows to the
/// keyboard-focused window, which is the main window whenever the user clicks an image.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn uses_wayland_overlay_parent() -> bool {
    gpui::guess_compositor() == "Wayland"
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn uses_wayland_overlay_parent() -> bool {
    false
}

pub fn overlay_fallback_placement(cx: &mut App) -> (WindowBounds, Option<DisplayId>) {
    (
        WindowBounds::Windowed(Bounds::centered(None, size(px(1100.0), px(740.0)), cx)),
        None,
    )
}

pub fn overlay_placement_for_main(cx: &mut App) -> (WindowBounds, Option<DisplayId>) {
    if let Some(main) = handle(cx)
        && let Some(placement) = window_placement_for(main, cx)
    {
        return placement;
    }
    overlay_fallback_placement(cx)
}

pub fn overlay_window_kind() -> WindowKind {
    if uses_wayland_overlay_parent() {
        WindowKind::Floating
    } else {
        WindowKind::Normal
    }
}

pub fn apply_overlay_bounds(window: &mut Window, bounds: WindowBounds) {
    match bounds {
        WindowBounds::Windowed(bounds) => window.set_bounds(bounds),
        WindowBounds::Maximized(_) => {
            if !window.is_maximized() {
                window.zoom_window();
            }
        }
        WindowBounds::Fullscreen(_) => {
            if !window.is_fullscreen() {
                window.toggle_fullscreen();
            }
        }
    }
}

pub fn sync_overlay_to_main<W: 'static>(
    overlay: WindowHandle<W>,
    main_app: AnyWindowHandle,
    cx: &mut App,
) {
    let Some((main_bounds, _)) = window_placement_for(main_app, cx) else {
        return;
    };
    let _ = overlay.update(cx, |_, window, _| {
        apply_overlay_bounds(window, main_bounds);
    });
}

#[cfg(test)]
mod tests {
    use super::{BASE_REM_SIZE, MAX_ZOOM_FACTOR, MIN_ZOOM_FACTOR};

    #[test]
    fn zoom_factor_bounds_match_settings_schema() {
        assert_eq!(MIN_ZOOM_FACTOR, 0.8);
        assert_eq!(MAX_ZOOM_FACTOR, 1.5);
        assert_eq!(BASE_REM_SIZE, 16.0);
    }
}
