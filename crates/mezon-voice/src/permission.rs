use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaDevice {
    Microphone,
    Camera,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaPermission {
    #[default]
    Granted,
    Undetermined,
    Denied,
}

pub const MEDIA_DENIAL_IS_AUTHORITATIVE: bool = cfg!(target_os = "macos");

type ChangeChannel = (flume::Sender<MediaDevice>, flume::Receiver<MediaDevice>);

static CHANGES: LazyLock<ChangeChannel> = LazyLock::new(flume::unbounded);

pub fn media_permission(device: MediaDevice) -> MediaPermission {
    platform::status(device)
}

pub fn media_permission_changes() -> flume::Receiver<MediaDevice> {
    CHANGES.1.clone()
}

pub fn request_media_permission(device: MediaDevice) {
    platform::request(device, |_| {});
}

pub fn media_privacy_settings_url(device: MediaDevice) -> Option<&'static str> {
    platform::settings_url(device)
}

#[cfg(target_os = "macos")]
pub(crate) fn request_media_permission_blocking(
    device: MediaDevice,
    timeout: std::time::Duration,
) -> bool {
    match media_permission(device) {
        MediaPermission::Granted => return true,
        MediaPermission::Denied => return false,
        MediaPermission::Undetermined => {}
    }
    let (tx, rx) = flume::bounded(1);
    platform::request(device, move |granted| {
        let _ = tx.send(granted);
    });
    rx.recv_timeout(timeout).unwrap_or(false)
}

fn publish_change(device: MediaDevice) {
    let _ = CHANGES.0.send(device);
}

#[cfg(target_os = "macos")]
mod platform {
    use block::ConcreteBlock;
    use cocoa::base::{BOOL, NO, id, nil};
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    use super::{MediaDevice, MediaPermission, publish_change};

    const NOT_DETERMINED: i64 = 0;
    const AUTHORIZED: i64 = 3;

    fn media_type_name(device: MediaDevice) -> &'static str {
        match device {
            MediaDevice::Microphone => "soun",
            MediaDevice::Camera => "vide",
        }
    }

    fn capture_device_class() -> Option<&'static Class> {
        Class::get("AVCaptureDevice")
    }

    pub(super) fn status(device: MediaDevice) -> MediaPermission {
        let Some(cls) = capture_device_class() else {
            return MediaPermission::Granted;
        };
        let status: i64 = unsafe {
            let media_type: id = NSString::alloc(nil).init_str(media_type_name(device));
            let status: i64 = msg_send![cls, authorizationStatusForMediaType: media_type];
            let _: () = msg_send![media_type, release];
            status
        };
        match status {
            AUTHORIZED => MediaPermission::Granted,
            NOT_DETERMINED => MediaPermission::Undetermined,
            _ => MediaPermission::Denied,
        }
    }

    pub(super) fn request(device: MediaDevice, on_done: impl Fn(bool) + Send + 'static) {
        let Some(cls) = capture_device_class() else {
            on_done(true);
            return;
        };
        let handler = ConcreteBlock::new(move |granted: BOOL| {
            on_done(granted != NO);
            publish_change(device);
        })
        .copy();
        unsafe {
            let media_type: id = NSString::alloc(nil).init_str(media_type_name(device));
            let _: () =
                msg_send![cls, requestAccessForMediaType: media_type completionHandler: &*handler];
            let _: () = msg_send![media_type, release];
        }
    }

    pub(super) fn settings_url(device: MediaDevice) -> Option<&'static str> {
        Some(match device {
            MediaDevice::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            MediaDevice::Camera => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
            }
        })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::LazyLock;

    use windows::Win32::Foundation::NO_ERROR;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::{PCWSTR, w};

    use super::{MediaDevice, MediaPermission, publish_change};

    static PACKAGED: LazyLock<bool> = LazyLock::new(|| {
        std::env::current_exe().is_ok_and(|exe| {
            exe.to_string_lossy()
                .to_ascii_lowercase()
                .contains("\\windowsapps\\")
        })
    });

    fn device_key(device: MediaDevice) -> PCWSTR {
        match device {
            MediaDevice::Microphone => w!(
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone"
            ),
            MediaDevice::Camera => w!(
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\webcam"
            ),
        }
    }

    fn desktop_apps_key(device: MediaDevice) -> PCWSTR {
        match device {
            MediaDevice::Microphone => w!(
                r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged"
            ),
            MediaDevice::Camera => w!(
                r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\webcam\NonPackaged"
            ),
        }
    }

    fn consent_denied(root: HKEY, key: PCWSTR) -> bool {
        let mut buffer = [0u16; 16];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        let result = unsafe {
            RegGetValueW(
                root,
                key,
                w!("Value"),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if result != NO_ERROR {
            return false;
        }
        let len = (size as usize / 2).saturating_sub(1).min(buffer.len());
        String::from_utf16_lossy(&buffer[..len]).eq_ignore_ascii_case("Deny")
    }

    pub(super) fn status(device: MediaDevice) -> MediaPermission {
        let denied = consent_denied(HKEY_LOCAL_MACHINE, device_key(device))
            || (!*PACKAGED && consent_denied(HKEY_CURRENT_USER, desktop_apps_key(device)));
        if denied {
            MediaPermission::Denied
        } else {
            MediaPermission::Granted
        }
    }

    pub(super) fn request(device: MediaDevice, on_done: impl Fn(bool) + Send + 'static) {
        on_done(status(device) == MediaPermission::Granted);
        publish_change(device);
    }

    pub(super) fn settings_url(device: MediaDevice) -> Option<&'static str> {
        Some(match device {
            MediaDevice::Microphone => "ms-settings:privacy-microphone",
            MediaDevice::Camera => "ms-settings:privacy-webcam",
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::{MediaDevice, MediaPermission, publish_change};

    pub(super) fn status(_device: MediaDevice) -> MediaPermission {
        MediaPermission::Granted
    }

    pub(super) fn request(device: MediaDevice, on_done: impl Fn(bool) + Send + 'static) {
        on_done(true);
        publish_change(device);
    }

    pub(super) fn settings_url(_device: MediaDevice) -> Option<&'static str> {
        None
    }
}
