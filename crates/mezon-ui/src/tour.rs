mod anchor;
mod launcher;
mod overlay;
mod state;
mod tracks;

use gpui::{App, AppContext as _, KeyBinding};

pub use anchor::{TourAnchor, probe};
pub use launcher::{TourLauncher, settings_entry_row};
pub use state::{
    TourState, TourStatus, auto_start_core, eligibility_undecided, layer, pending_core_track,
    shutdown,
};

pub struct McpAdvance {
    pub moved: bool,
    pub still_active: bool,
}

pub fn mcp_start(track: Option<&str>, cx: &mut App) -> anyhow::Result<Option<&'static str>> {
    let handle = crate::app::main_window::handle(cx)
        .ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    if let Some(id) = track
        && tracks::track(id).is_none()
    {
        anyhow::bail!("unknown tour track: {id}");
    }
    let requested = track.map(str::to_string);
    cx.update_window(handle, |_, window, cx| match requested.as_deref() {
        Some(id) => TourState::start_track(id, window, cx),
        None => {
            auto_start_core(window, cx);
        }
    })?;
    let running = TourState::try_global(cx).and_then(|entity| entity.read(cx).running_track());
    Ok(match (requested.as_deref(), running) {
        (Some(id), Some(started)) if started == id => Some(started),
        (Some(_), _) => None,
        (None, running) => running,
    })
}

pub fn auto_start_if_context_holds(expected: &'static str, cx: &mut App) -> bool {
    if pending_core_track(cx) != Some(expected) {
        return false;
    }
    let Some(handle) = crate::app::main_window::handle(cx) else {
        return false;
    };
    cx.update_window(handle, |_, window, cx| auto_start_core(window, cx))
        .unwrap_or(false)
}

pub fn mcp_advance(forward: bool, cx: &mut App) -> anyhow::Result<Option<McpAdvance>> {
    let handle = crate::app::main_window::handle(cx)
        .ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let Some(entity) = TourState::try_global(cx) else {
        return Ok(None);
    };
    if !entity.read(cx).is_active() {
        return Ok(None);
    }
    let advance = cx.update_window(handle, |_, window, cx| {
        entity.update(cx, |this, cx| this.advance(forward, window, cx))
    })?;
    Ok(Some(McpAdvance {
        moved: advance.moved,
        still_active: advance.still_active,
    }))
}

pub fn init(cx: &mut App) {
    TourState::init(cx);
    cx.bind_keys([
        KeyBinding::new("escape", ::menu::Cancel, Some("tour")),
        KeyBinding::new("right", state::TourNext, Some("tour")),
        KeyBinding::new("enter", state::TourNext, Some("tour")),
        KeyBinding::new("left", state::TourBack, Some("tour")),
    ]);
}
