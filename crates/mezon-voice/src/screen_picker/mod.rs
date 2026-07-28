use scap::Target;

#[cfg(target_os = "linux")]
use crate::screen_targets::ScreenShareKind;

#[derive(Debug, Clone)]
pub enum PickedScreen {
    Target(Target),
    #[cfg(target_os = "linux")]
    LinuxPortal(ScreenShareKind),
}

pub(crate) fn pick_is_window(pick: &PickedScreen) -> bool {
    #[cfg(target_os = "linux")]
    if let PickedScreen::LinuxPortal(kind) = pick {
        return matches!(kind, ScreenShareKind::Window);
    }
    matches!(pick, PickedScreen::Target(Target::Window(_)))
}

pub(crate) fn portal_source_types_for_pick(pick: &PickedScreen) -> Option<u32> {
    match pick {
        PickedScreen::Target(Target::Window(_)) => Some(2),
        PickedScreen::Target(Target::Display(_)) => Some(1),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal(ScreenShareKind::Window) => Some(2),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal(ScreenShareKind::Display) => Some(1),
    }
}

pub(crate) fn scap_target_for_pick(pick: PickedScreen) -> Result<Option<Target>, String> {
    match pick {
        PickedScreen::Target(target) => Ok(Some(resolve_target(&target)?)),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal(_) => Ok(None),
    }
}

fn resolve_target(target: &Target) -> Result<Target, String> {
    let targets = scap::get_all_targets().map_err(|e| e.to_string())?;
    match target {
        Target::Window(window) => targets
            .into_iter()
            .find(|candidate| match candidate {
                Target::Window(candidate) => {
                    #[cfg(target_os = "linux")]
                    {
                        xcb::Xid::resource_id(&candidate.raw_handle)
                            == xcb::Xid::resource_id(&window.raw_handle)
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        candidate.id == window.id
                    }
                }
                _ => false,
            })
            .ok_or_else(|| format!("window \"{}\" is no longer available", window.title)),
        Target::Display(display) => targets
            .into_iter()
            .find(|candidate| match candidate {
                Target::Display(candidate) => candidate.id == display.id,
                _ => false,
            })
            .ok_or_else(|| format!("display \"{}\" is no longer available", display.title)),
    }
}
