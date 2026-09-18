#[cfg(target_os = "macos")]
use gpui::AppContext as _;
use gpui::{ClipboardItem, Context, Window};
use std::sync::atomic::{AtomicBool, Ordering};

static OFF_MAIN_THREAD_READS: AtomicBool = AtomicBool::new(false);

pub fn enable_off_main_thread_reads() {
    OFF_MAIN_THREAD_READS.store(true, Ordering::Relaxed);
}

pub fn read_then<T: 'static>(
    this: &mut T,
    window: &mut Window,
    cx: &mut Context<T>,
    apply: impl FnOnce(&mut T, ClipboardItem, &mut Window, &mut Context<T>) + 'static,
) {
    #[cfg(target_os = "macos")]
    if OFF_MAIN_THREAD_READS.load(Ordering::Relaxed) && mac::advertises_plain_text_only() {
        let text = cx.background_spawn(async { mac::plain_text() });
        cx.spawn_in(window, async move |entity, cx| {
            let Some(text) = text.await else {
                return;
            };
            entity
                .update_in(cx, |this, window, cx| {
                    apply(this, ClipboardItem::new_string(text), window, cx)
                })
                .ok();
        })
        .detach();
        return;
    }

    let Some(item) = cx.read_from_clipboard() else {
        return;
    };
    apply(this, item, window, cx);
}

#[cfg(target_os = "macos")]
mod mac {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{CStr, CString};
    use std::ptr;

    const PLAIN_TEXT: &str = "public.utf8-plain-text";
    const FILENAMES: &str = "NSFilenamesPboardType";
    const ZED_TEXT_HASH: &str = "zed-text-hash";

    pub(super) fn advertises_plain_text_only() -> bool {
        unsafe {
            let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard.is_null() {
                return false;
            }
            let types: *mut Object = msg_send![pasteboard, types];
            if types.is_null() {
                return false;
            }
            contains(types, PLAIN_TEXT)
                && !contains(types, FILENAMES)
                && !contains(types, ZED_TEXT_HASH)
        }
    }

    pub(super) fn plain_text() -> Option<String> {
        unsafe {
            let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
            let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            let text = if pasteboard.is_null() {
                None
            } else {
                let kind = ns_string(PLAIN_TEXT);
                let value: *mut Object = msg_send![pasteboard, stringForType: kind];
                ns_string_to_rust(value)
            };
            let _: () = msg_send![pool, release];
            text
        }
    }

    unsafe fn contains(types: *mut Object, kind: &str) -> bool {
        unsafe {
            let needle = ns_string(kind);
            if needle.is_null() {
                return false;
            }
            msg_send![types, containsObject: needle]
        }
    }

    unsafe fn ns_string(value: &str) -> *mut Object {
        let Ok(raw) = CString::new(value) else {
            return ptr::null_mut();
        };
        unsafe { msg_send![class!(NSString), stringWithUTF8String: raw.as_ptr()] }
    }

    unsafe fn ns_string_to_rust(value: *mut Object) -> Option<String> {
        if value.is_null() {
            return None;
        }
        unsafe {
            let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
            if utf8.is_null() {
                return None;
            }
            Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    }
}
