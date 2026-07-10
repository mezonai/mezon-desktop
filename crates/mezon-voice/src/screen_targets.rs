use std::time::{Duration, Instant};

use parking_lot::Mutex;
use scap::Target;

use crate::screen_picker::PickedScreen;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenShareKind {
    Window,
    Display,
}

#[derive(Clone, Debug)]
pub struct ScreenShareOption {
    pub id: u32,
    pub title: String,
    pub kind: ScreenShareKind,
    pub pick: PickedScreen,
}

impl ScreenShareOption {
    pub fn is_portal(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            matches!(self.pick, PickedScreen::LinuxPortal)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenShareListError {
    PermissionDenied,
    Unavailable(String),
}

const CACHE_TTL: Duration = Duration::from_secs(15);

static CACHE: Mutex<Option<(Instant, Vec<ScreenShareOption>)>> = Mutex::new(None);

pub fn peek_screen_share_options() -> Option<Vec<ScreenShareOption>> {
    let cache = CACHE.lock();
    let (fetched_at, options) = cache.as_ref()?;
    if fetched_at.elapsed() >= CACHE_TTL {
        return None;
    }
    Some(options.clone())
}

pub fn list_screen_share_options() -> Result<Vec<ScreenShareOption>, ScreenShareListError> {
    if let Some(options) = peek_screen_share_options() {
        return Ok(options);
    }

    let options = fetch_screen_share_options()?;
    *CACHE.lock() = Some((Instant::now(), options.clone()));
    Ok(options)
}

fn fetch_screen_share_options() -> Result<Vec<ScreenShareOption>, ScreenShareListError> {
    if !scap::is_supported() {
        return Err(ScreenShareListError::Unavailable(
            "screen capture not supported".into(),
        ));
    }
    if !scap::has_permission() && !scap::request_permission() {
        return Err(ScreenShareListError::PermissionDenied);
    }

    #[cfg(target_os = "macos")]
    {
        list_macos_options().map_err(ScreenShareListError::Unavailable)
    }

    #[cfg(not(target_os = "macos"))]
    {
        list_scap_options().map_err(ScreenShareListError::Unavailable)
    }
}

#[cfg(not(target_os = "macos"))]
fn list_scap_options() -> Result<Vec<ScreenShareOption>, String> {
    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        return Ok(vec![ScreenShareOption {
            id: 0,
            title: String::new(),
            kind: ScreenShareKind::Display,
            pick: PickedScreen::LinuxPortal,
        }]);
    }

    let mut options = Vec::new();
    for target in scap::get_all_targets().map_err(|e| e.to_string())? {
        match target {
            Target::Window(window) => {
                if window.title.trim().is_empty() {
                    continue;
                }
                options.push(ScreenShareOption {
                    id: window.id,
                    title: window.title.clone(),
                    kind: ScreenShareKind::Window,
                    pick: PickedScreen::Target(Target::Window(window)),
                });
            }
            Target::Display(display) => {
                options.push(ScreenShareOption {
                    id: display.id,
                    title: display.title.clone(),
                    kind: ScreenShareKind::Display,
                    pick: PickedScreen::Target(Target::Display(display)),
                });
            }
        }
    }
    Ok(options)
}

#[cfg(target_os = "macos")]
fn list_macos_options() -> Result<Vec<ScreenShareOption>, String> {
    use core_graphics_helmer_fork::display::{CGDirectDisplayID, CGDisplay};
    use core_graphics_helmer_fork::window::CGWindowID;
    use scap::{Display, Window};
    use screencapturekit::sc_shareable_content::SCShareableContent;

    let content = SCShareableContent::current();
    let mut options = Vec::new();

    for display in content.displays {
        let id: CGDirectDisplayID = display.display_id;
        let title = macos_display_name(id);
        options.push(ScreenShareOption {
            id,
            title: title.clone(),
            kind: ScreenShareKind::Display,
            pick: PickedScreen::Target(Target::Display(Display {
                id,
                title,
                raw_handle: CGDisplay::new(id),
            })),
        });
    }

    for window in content.windows {
        if !is_shareable_macos_window(&window) {
            continue;
        }
        let Some(title) = window
            .title
            .as_ref()
            .map(|title| title.trim())
            .filter(|title| !title.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        let id = window.window_id;
        options.push(ScreenShareOption {
            id,
            title: title.clone(),
            kind: ScreenShareKind::Window,
            pick: PickedScreen::Target(Target::Window(Window {
                id,
                title,
                raw_handle: id as CGWindowID,
            })),
        });
    }

    Ok(options)
}

#[cfg(target_os = "macos")]
fn is_shareable_macos_window(window: &screencapturekit::sc_window::SCWindow) -> bool {
    if !window.is_on_screen {
        return false;
    }
    if window.window_layer != 0 {
        return false;
    }
    if window.width < 120 || window.height < 80 {
        return false;
    }
    let app = match &window.owning_application {
        Some(app) => app,
        None => return false,
    };
    let app_name = app
        .application_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if app_name.is_none() {
        return false;
    }
    let bundle_id = app
        .bundle_identifier
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or_default();
    !matches!(
        bundle_id,
        "com.apple.dock"
            | "com.apple.controlcenter"
            | "com.apple.WindowManager"
            | "com.apple.notificationcenterui"
    )
}

#[cfg(target_os = "macos")]
fn macos_display_name(display_id: core_graphics_helmer_fork::display::CGDirectDisplayID) -> String {
    use cocoa::appkit::NSScreen;
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::rc::{StrongPtr, autoreleasepool};
    use objc::{msg_send, sel, sel_impl};

    autoreleasepool(|| unsafe {
        let screen_number_key = StrongPtr::new(NSString::alloc(nil).init_str("NSScreenNumber"));
        let screens: id = NSScreen::screens(nil);
        let count: u64 = msg_send![screens, count];

        for index in 0..count {
            let screen: id = msg_send![screens, objectAtIndex: index];
            let device_description: id = msg_send![screen, deviceDescription];
            let display_id_number: id =
                msg_send![device_description, objectForKey: *screen_number_key];
            let display_id_number: u32 = msg_send![display_id_number, unsignedIntValue];

            if display_id_number == display_id {
                let localized_name: id = msg_send![screen, localizedName];
                let name: *const i8 = msg_send![localized_name, UTF8String];
                return std::ffi::CStr::from_ptr(name)
                    .to_string_lossy()
                    .into_owned();
            }
        }

        format!("Display {display_id}")
    })
}
