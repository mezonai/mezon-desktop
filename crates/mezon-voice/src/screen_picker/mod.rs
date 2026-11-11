use scap::Target;

#[derive(Debug, Clone)]
pub enum PickedScreen {
    Target(Target),
    #[cfg(target_os = "linux")]
    LinuxPortal,
}

pub fn system_screen_share_pick() -> Option<PickedScreen> {
    #[cfg(target_os = "linux")]
    if crate::linux_session::is_wayland_session() {
        return Some(PickedScreen::LinuxPortal);
    }
    None
}

pub(crate) fn pick_polls_at_capture_fps(pick: &PickedScreen) -> bool {
    cfg!(any(target_os = "linux", target_os = "freebsd")) && !pick_uses_portal(pick)
}

pub(crate) fn pick_uses_portal(pick: &PickedScreen) -> bool {
    #[cfg(target_os = "linux")]
    {
        matches!(pick, PickedScreen::LinuxPortal)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pick;
        false
    }
}

pub(crate) fn portal_parent_window_for_pick(pick: &PickedScreen) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        match pick {
            PickedScreen::LinuxPortal => crate::linux_session::portal_parent_window(),
            PickedScreen::Target(_) => None,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pick;
        None
    }
}

pub(crate) fn pick_is_window(pick: &PickedScreen) -> bool {
    matches!(pick, PickedScreen::Target(Target::Window(_)))
}

pub(crate) fn portal_source_types_for_pick(pick: &PickedScreen) -> Option<u32> {
    match pick {
        PickedScreen::Target(Target::Window(_)) => Some(2),
        PickedScreen::Target(Target::Display(_)) => Some(1),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal => Some(3),
    }
}

pub(crate) fn scap_target_for_pick(pick: PickedScreen) -> Result<Option<Target>, String> {
    match pick {
        PickedScreen::Target(target) => Ok(Some(resolve_target(&target)?)),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal => Ok(None),
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
