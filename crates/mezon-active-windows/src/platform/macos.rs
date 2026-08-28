use crate::info::ActiveWindowInfo;
use core_foundation::array::CFArrayGetValueAtIndex;
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::{CFNumber, CFNumberRef};
use core_foundation::string::CFStringRef;
use std::ffi::c_void;
use std::os::raw::c_void as RawCVoid;

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const K_CG_NULL_WINDOW_ID: u32 = 0;

struct WindowEntry {
    owner: String,
    pid: String,
    layer: i64,
}

pub fn get_active_window() -> anyhow::Result<ActiveWindowInfo> {
    let list_options =
        K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS;
    let window_list_ptr = unsafe { CGWindowListCopyWindowInfo(list_options, K_CG_NULL_WINDOW_ID) };
    if window_list_ptr.is_null() {
        return Err(anyhow::anyhow!("Failed to copy window info list"));
    }

    let window_list: core_foundation::array::CFArray =
        unsafe { TCFType::wrap_under_create_rule(window_list_ptr as _) };
    let entries = collect_window_entries(&window_list);
    let frontmost = entries
        .iter()
        .find(|entry| entry.layer == 0 && !entry.owner.is_empty())
        .ok_or_else(|| anyhow::anyhow!("No foreground window found"))?;

    Ok(ActiveWindowInfo {
        os: "macos".to_string(),
        window_class: frontmost.owner.clone(),
        window_name: String::new(),
        window_desktop: "0".to_string(),
        window_type: "0".to_string(),
        window_pid: frontmost.pid.clone(),
        idle_time: get_idle_time().to_string(),
    })
}

fn collect_window_entries(window_list: &core_foundation::array::CFArray) -> Vec<WindowEntry> {
    let count = window_list.len();
    let mut entries = Vec::with_capacity(count as usize);
    for index in 0..count {
        let dict_ref = unsafe { CFArrayGetValueAtIndex(window_list.as_concrete_TypeRef(), index) };
        if dict_ref.is_null() {
            continue;
        }
        let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(dict_ref as _) };
        let owner = dictionary_string(&dict, "kCGWindowOwnerName");
        if owner.is_empty() || is_ignored_owner(&owner) {
            continue;
        }
        entries.push(WindowEntry {
            owner,
            pid: dictionary_i64(&dict, "kCGWindowOwnerPID")
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "0".to_string()),
            layer: dictionary_i64(&dict, "kCGWindowLayer").unwrap_or(-1),
        });
    }
    entries
}

fn is_ignored_owner(owner: &str) -> bool {
    matches!(
        owner,
        "Window Server" | "Dock" | "SystemUIServer" | "Control Center"
    )
}

fn dictionary_string(dict: &CFDictionary, key: &str) -> String {
    let key = core_foundation::string::CFString::new(key);
    let Some(value_ref) = dict.find(key.as_concrete_TypeRef() as *const RawCVoid) else {
        return String::new();
    };
    let cf_str = unsafe {
        core_foundation::string::CFString::wrap_under_get_rule(*value_ref as CFStringRef)
    };
    cf_str.to_string()
}

fn dictionary_i64(dict: &CFDictionary, key: &str) -> Option<i64> {
    let key = core_foundation::string::CFString::new(key);
    let value_ref = dict.find(key.as_concrete_TypeRef() as *const RawCVoid)?;
    let number = unsafe { CFNumber::wrap_under_get_rule(*value_ref as CFNumberRef) };
    number.to_i64()
}

fn get_idle_time() -> u64 {
    let idle = unsafe { CGEventSourceSecondsSinceLastEventType(0, !0) };
    if idle < 0.0 { 0 } else { idle as u64 }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
    fn CGEventSourceSecondsSinceLastEventType(source_state_id: i32, event_type: u32) -> f64;
}
