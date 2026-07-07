use gpui::{AnyWindowHandle, App, AppContext, Global};

struct MainWindowHandle(AnyWindowHandle);
impl Global for MainWindowHandle {}

pub fn register_main_window(handle: AnyWindowHandle, cx: &mut App) {
    if cx.try_global::<MainWindowHandle>().is_some() {
        tracing::warn!("register_main_window called more than once");
    }
    cx.set_global(MainWindowHandle(handle));
}

pub fn handle(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<MainWindowHandle>().map(|g| g.0)
}

pub fn activate_main_window(cx: &mut App) {
    let Some(handle) = handle(cx) else {
        return;
    };
    let _ = cx.update_window(handle, |_, window, _| window.activate_window());
}
