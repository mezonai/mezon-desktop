mod native;
mod overlay;
mod toolbar;

use gpui::{App, Entity};
use mezon_store::VoiceStore;

pub(crate) fn desktop_annotation_supported(store: &VoiceStore) -> bool {
    native::share_target(store).is_some()
}

#[cfg(target_os = "linux")]
pub(crate) fn draw_unavailable(store: &VoiceStore) -> bool {
    matches!(
        store.shared_screen(),
        Some(mezon_store::PickedScreen::LinuxPortal)
    ) && !desktop_annotation_supported(store)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn draw_unavailable(_store: &VoiceStore) -> bool {
    false
}

pub(crate) fn toggle_desktop_annotation(voice: &Entity<VoiceStore>, cx: &mut App) {
    if voice.read(cx).desktop_draw().is_some() {
        voice.update(cx, |store, cx| store.end_desktop_draw(cx));
        return;
    }
    let Some(target) = native::share_target(voice.read(cx)) else {
        return;
    };

    let draw = voice.update(cx, |store, cx| {
        store.start_desktop_draw(target.is_window(), cx)
    });
    let Some(opened) = overlay::open(voice.clone(), draw.clone(), target, cx) else {
        voice.update(cx, |store, cx| store.end_desktop_draw_for(&draw, cx));
        return;
    };
    draw.update(cx, |draw, _| draw.attach_window(opened.handle));

    if let Some(handle) = toolbar::open(
        voice.clone(),
        draw.clone(),
        opened.rect,
        target.is_window(),
        opened.native,
        cx,
    ) {
        draw.update(cx, |draw, _| draw.attach_window(handle));
    }
}
