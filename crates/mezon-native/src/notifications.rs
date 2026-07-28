/// Desktop notification support.
///
/// macOS  : `UNUserNotificationCenter` via raw `objc` runtime calls.
///          Authorisation is requested once from `init`, not per notification.
/// Windows: Windows Runtime `ToastNotification` via the `windows` crate.
///          `init` registers the process AUMID and a Start Menu shortcut carrying
///          the same AUMID — without that shortcut the OS drops every toast.
/// Linux  : `notify-rust` (wraps `libnotify` / D-Bus `org.freedesktop.Notifications`).

#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Optional channel ID — used as the per-channel replacement key.
    pub channel_id: Option<String>,
    pub clan_id: Option<String>,
    /// Server-rendered navigation path (React `extras.link`); the click target.
    pub link: Option<String>,
    /// Local file path to the sender-avatar image, shown as the notification icon.
    pub icon_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTarget {
    pub clan_id: Option<String>,
    pub channel_id: String,
    pub link: Option<String>,
}

type ClickHandler = Box<dyn Fn(NotificationTarget) + Send + Sync + 'static>;

static CLICK_HANDLER: std::sync::OnceLock<ClickHandler> = std::sync::OnceLock::new();

/// Maps a notification's replacement identifier to its server-rendered link, so a
/// click can navigate exactly where React would (`extras.link`) instead of
/// reconstructing a route from ids.
static LINK_BY_ID: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> =
    std::sync::Mutex::new(None);

fn remember_link(identifier: &str, link: &str) {
    if link.is_empty() {
        return;
    }
    if let Ok(mut guard) = LINK_BY_ID.lock() {
        guard
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(identifier.to_owned(), link.to_owned());
    }
}

fn build_target(identifier: &str) -> Option<NotificationTarget> {
    let mut target = decode_identifier(identifier)?;
    target.link = LINK_BY_ID
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.get(identifier).cloned()));
    Some(target)
}

pub fn set_click_handler(handler: ClickHandler) {
    if CLICK_HANDLER.set(handler).is_err() {
        tracing::warn!("notification click handler already registered");
    }
}

fn dispatch_click(target: NotificationTarget) {
    match CLICK_HANDLER.get() {
        Some(handler) => handler(target),
        None => tracing::debug!("notification clicked but no handler registered"),
    }
}

const IDENTIFIER_PREFIX: &str = "mezon-notify";

fn encode_identifier(clan_id: Option<&String>, channel_id: &str) -> String {
    let clan = clan_id
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("0");
    format!("{IDENTIFIER_PREFIX}-{clan}-{channel_id}")
}

fn decode_identifier(identifier: &str) -> Option<NotificationTarget> {
    let rest = identifier
        .strip_prefix(IDENTIFIER_PREFIX)?
        .strip_prefix('-')?;
    let (clan, channel) = rest.split_once('-')?;
    if channel.is_empty() {
        return None;
    }
    Some(NotificationTarget {
        clan_id: Some(clan.to_owned()).filter(|c| c != "0" && !c.is_empty()),
        link: None,
        channel_id: channel.to_owned(),
    })
}

#[cfg(target_os = "windows")]
const APP_USER_MODEL_ID: &str = "ai.mezon.Mezon";

pub fn init() {
    #[cfg(target_os = "macos")]
    init_macos();

    #[cfg(target_os = "windows")]
    init_windows();
}

/// Show a desktop notification.  Fire-and-forget; errors are logged but not propagated.
pub fn show(notification: &Notification) {
    tracing::debug!(
        channel_id = ?notification.channel_id,
        "Showing desktop notification"
    );

    if let (Some(key), Some(link)) = (replacement_key(notification), notification.link.as_deref()) {
        remember_link(&key, link);
    }

    #[cfg(target_os = "macos")]
    show_macos(notification);

    #[cfg(target_os = "windows")]
    show_windows(notification);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    show_linux(notification);
}

fn replacement_key(n: &Notification) -> Option<String> {
    let channel = n
        .channel_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty() && *id != "0")?;
    Some(encode_identifier(n.clan_id.as_ref(), channel))
}

// ─── macOS ────────────────────────────────────────────────────────────────────
//
// UNUserNotificationCenter is available on macOS 10.14+.
// We drive it through the raw `objc` runtime to avoid linking against the
// UserNotifications framework header directly.

#[cfg(target_os = "macos")]
static NOTIFICATION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "macos")]
const AUTH_UNKNOWN: u8 = 0;
#[cfg(target_os = "macos")]
const AUTH_GRANTED: u8 = 1;
#[cfg(target_os = "macos")]
const AUTH_DENIED: u8 = 2;
#[cfg(target_os = "macos")]
static NOTIFICATION_AUTH: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(AUTH_UNKNOWN);

/// Whether the OS permits notifications. Returns `false` only when authorisation was
/// explicitly denied; an as-yet-undetermined state (async grant pending) returns `true`.
pub fn notifications_permitted() -> bool {
    #[cfg(target_os = "macos")]
    {
        NOTIFICATION_AUTH.load(std::sync::atomic::Ordering::Relaxed) != AUTH_DENIED
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
fn leak_for_async_objc_callback<T>(block: T) {
    std::mem::forget(block);
}

#[cfg(target_os = "macos")]
fn has_bundle_identifier() -> bool {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let bundle: *mut Object = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return false;
        }
        let bundle_id: *mut Object = msg_send![bundle, bundleIdentifier];
        !bundle_id.is_null()
    }
}

#[cfg(target_os = "macos")]
extern "C" fn did_receive_notification_response(
    _this: &objc::runtime::Object,
    _sel: objc::runtime::Sel,
    _center: *mut objc::runtime::Object,
    response: *mut objc::runtime::Object,
    completion: *mut objc::runtime::Object,
) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        if !response.is_null() {
            let notification: *mut Object = msg_send![response, notification];
            if !notification.is_null() {
                let request: *mut Object = msg_send![notification, request];
                if !request.is_null() {
                    let identifier: *mut Object = msg_send![request, identifier];
                    if let Some(identifier) = nsstring_to_string(identifier)
                        && let Some(target) = build_target(&identifier)
                    {
                        dispatch_click(target);
                    }
                }
            }
        }
        if !completion.is_null() {
            let block = completion as *mut block::Block<(), ()>;
            (*block).call(());
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn nsstring_to_string(s: *mut objc::runtime::Object) -> Option<String> {
    use objc::{msg_send, sel, sel_impl};

    if s.is_null() {
        return None;
    }
    unsafe {
        let bytes: *const std::os::raw::c_char = msg_send![s, UTF8String];
        if bytes.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(bytes)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

#[cfg(target_os = "macos")]
fn install_notification_delegate(center: *mut objc::runtime::Object) {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

    let delegate = *DELEGATE.get_or_init(|| unsafe {
        let superclass = class!(NSObject);
        let Some(mut decl) = ClassDecl::new("MezonNotificationDelegate", superclass) else {
            return 0;
        };
        let callback: extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object) =
            did_receive_notification_response;
        decl.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            callback,
        );
        let cls: &Class = decl.register();
        let instance: *mut Object = msg_send![cls, new];
        instance as usize
    });

    if delegate == 0 {
        tracing::warn!("could not register notification delegate class");
        return;
    }
    unsafe {
        let _: () = msg_send![center, setDelegate: delegate as *mut Object];
    }
}

#[cfg(target_os = "macos")]
fn init_macos() {
    use block::ConcreteBlock;
    use objc::runtime::{BOOL, Object, YES};
    use objc::{class, msg_send, sel, sel_impl};

    if !has_bundle_identifier() {
        tracing::debug!("skipping notification setup: not running from an app bundle");
        return;
    }

    unsafe {
        let center_cls = class!(UNUserNotificationCenter);
        let center: *mut Object = msg_send![center_cls, currentNotificationCenter];

        install_notification_delegate(center);

        const AUTH_OPTIONS_BADGE_SOUND_ALERT: usize = 0b111;
        let options: usize = AUTH_OPTIONS_BADGE_SOUND_ALERT;
        let handler = ConcreteBlock::new(move |granted: BOOL, _error: *mut Object| {
            if granted == YES {
                tracing::info!("notification authorisation granted");
                NOTIFICATION_AUTH.store(AUTH_GRANTED, std::sync::atomic::Ordering::Relaxed);
            } else {
                tracing::warn!("notification authorisation denied; notifications will not appear");
                NOTIFICATION_AUTH.store(AUTH_DENIED, std::sync::atomic::Ordering::Relaxed);
            }
        });
        let handler = handler.copy();
        let _: () = msg_send![
            center,
            requestAuthorizationWithOptions: options
            completionHandler: &*handler
        ];
        leak_for_async_objc_callback(handler);
    }
}

#[cfg(target_os = "macos")]
fn show_macos(n: &Notification) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    if !has_bundle_identifier() {
        tracing::debug!("skipping notification: not running from an app bundle");
        return;
    }

    unsafe {
        let center_cls = class!(UNUserNotificationCenter);
        let center: *mut Object = msg_send![center_cls, currentNotificationCenter];

        let content_cls = class!(UNMutableNotificationContent);
        let content: *mut Object = msg_send![content_cls, new];

        let ns_title = nsstring(&n.title);
        let _: () = msg_send![content, setTitle: ns_title];
        let _: () = msg_send![ns_title, release];

        let ns_body = nsstring(&n.body);
        let _: () = msg_send![content, setBody: ns_body];
        let _: () = msg_send![ns_body, release];

        // Default alert sound, matching React's `sound: 'default'`; without this a
        // UNUserNotificationCenter notification is delivered silently.
        let sound_cls = class!(UNNotificationSound);
        let sound: *mut Object = msg_send![sound_cls, defaultSound];
        let _: () = msg_send![content, setSound: sound];

        if let Some(icon_path) = n.icon_path.as_deref() {
            attach_icon_macos(content, icon_path);
        }

        let key = replacement_key(n);
        if let Some(key) = key.as_deref() {
            let ns_thread = nsstring(key);
            let _: () = msg_send![content, setThreadIdentifier: ns_thread];
            let _: () = msg_send![ns_thread, release];
        }

        let identifier = key.unwrap_or_else(|| {
            let counter = NOTIFICATION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("mezon-{}-{counter}", std::process::id())
        });
        let ns_identifier = nsstring(&identifier);
        let request_cls = class!(UNNotificationRequest);
        let request: *mut Object = msg_send![
            request_cls,
            requestWithIdentifier: ns_identifier
            content: content
            trigger: std::ptr::null::<Object>()
        ];
        let _: () = msg_send![ns_identifier, release];
        let _: () = msg_send![content, release];

        let _: () = msg_send![
            center,
            addNotificationRequest: request
            withCompletionHandler: std::ptr::null::<Object>()
        ];
    }
}

/// Create an Objective-C NSString from a Rust &str.
#[cfg(target_os = "macos")]
unsafe fn nsstring(s: &str) -> *mut objc::runtime::Object {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let cls = class!(NSString);
    let obj: *mut Object = msg_send![cls, alloc];
    msg_send![
        obj,
        initWithBytes: s.as_ptr()
        length: s.len()
        encoding: 4u64 // NSUTF8StringEncoding
    ]
}

/// Attach a local image file to the notification content as its icon. The system
/// copies the file into its own store, so the caller's temp file may be deleted.
#[cfg(target_os = "macos")]
unsafe fn attach_icon_macos(content: *mut objc::runtime::Object, path: &str) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let ns_path = nsstring(path);
        let url_cls = class!(NSURL);
        let file_url: *mut Object = msg_send![url_cls, fileURLWithPath: ns_path];
        let _: () = msg_send![ns_path, release];
        if file_url.is_null() {
            return;
        }

        let ns_id = nsstring("");
        let attachment_cls = class!(UNNotificationAttachment);
        let mut error: *mut Object = std::ptr::null_mut();
        let attachment: *mut Object = msg_send![
            attachment_cls,
            attachmentWithIdentifier: ns_id
            URL: file_url
            options: std::ptr::null::<Object>()
            error: &mut error
        ];
        let _: () = msg_send![ns_id, release];
        if attachment.is_null() {
            return;
        }

        let array_cls = class!(NSArray);
        let array: *mut Object = msg_send![array_cls, arrayWithObject: attachment];
        let _: () = msg_send![content, setAttachments: array];
    }
}

// ─── Windows ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn init_windows() {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::HSTRING;

    unsafe {
        if let Err(e) = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_USER_MODEL_ID)) {
            tracing::warn!("failed to set AppUserModelID: {e}");
        }
    }

    if let Err(e) = ensure_start_menu_shortcut() {
        tracing::warn!("failed to create Start Menu shortcut; toasts may not appear: {e}");
    }
}

#[cfg(target_os = "windows")]
fn ensure_start_menu_shortcut() -> anyhow::Result<()> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::{
        InitPropVariantFromStringAsVector, PROPVARIANT,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        IPersistFile,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};

    let Some(appdata) = dirs::data_dir() else {
        anyhow::bail!("no APPDATA directory");
    };
    let shortcut = appdata
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Mezon.lnk");
    if shortcut.exists() {
        return Ok(());
    }
    if let Some(parent) = shortcut.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let exe = std::env::current_exe()?;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }

        let store: IPropertyStore = link.cast()?;
        let value: PROPVARIANT =
            InitPropVariantFromStringAsVector(&HSTRING::from(APP_USER_MODEL_ID))?;
        store.SetValue(&PKEY_AppUserModel_ID, &value)?;
        store.Commit()?;

        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(shortcut.as_os_str()), true)?;
    }

    tracing::info!("created Start Menu shortcut for toast notifications");
    Ok(())
}

#[cfg(target_os = "windows")]
fn show_windows(n: &Notification) {
    let title = n.title.clone();
    let body = n.body.clone();
    let tag = replacement_key(n);
    let icon_path = n.icon_path.clone();

    std::thread::spawn(move || {
        if let Err(e) = try_show_toast(&title, &body, tag.as_deref(), icon_path.as_deref()) {
            tracing::warn!("Windows toast notification failed: {e}");
        }
    });
}

#[cfg(target_os = "windows")]
fn try_show_toast(
    title: &str,
    body: &str,
    tag: Option<&str>,
    icon_path: Option<&str>,
) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;

    let image_xml = match icon_path {
        Some(path) => format!(
            r#"<image placement="appLogoOverride" hint-crop="circle" src="file:///{}"/>"#,
            escape_xml(&path.replace('\\', "/"))
        ),
        None => String::new(),
    };
    let xml_str = format!(
        r#"<toast>
  <visual>
    <binding template="ToastGeneric">
      {image_xml}
      <text>{}</text>
      <text>{}</text>
    </binding>
  </visual>
</toast>"#,
        escape_xml(title),
        escape_xml(body)
    );

    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml_str))?;

    let notifier =
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    if let Some(tag) = tag {
        let tag: String = tag.chars().take(64).collect();
        toast.SetTag(&HSTRING::from(tag.as_str()))?;
        toast.SetGroup(&HSTRING::from("mezon"))?;
    }

    let activation_target = tag.and_then(|t| build_target(&t));
    toast.Activated(&windows::Foundation::TypedEventHandler::<
        ToastNotification,
        windows::core::IInspectable,
    >::new(move |_, _| {
        if let Some(target) = activation_target.clone() {
            dispatch_click(target);
        }
        Ok(())
    }))?;

    notifier.Show(&toast)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ─── Linux ────────────────────────────────────────────────────────────────────
//
// Uses the `notify-rust` crate which wraps D-Bus `org.freedesktop.Notifications`.

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LINUX_DESKTOP_ENTRY: &str = "mezon";

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_replace_id(key: &str) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as u32).max(1)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn show_linux(n: &Notification) {
    let title = n.title.clone();
    let body = n.body.clone();
    let key = replacement_key(n);
    let replace_id = key.as_deref().map(linux_replace_id);
    let activation_target = key.as_deref().and_then(build_target);
    // Prefer the downloaded sender avatar; fall back to the installed app icon.
    let icon = n
        .icon_path
        .clone()
        .unwrap_or_else(|| LINUX_DESKTOP_ENTRY.to_owned());

    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification
            .appname("Mezon")
            .summary(&title)
            .body(&body)
            .icon(&icon)
            .sound_name("message-new-instant")
            .hint(notify_rust::Hint::DesktopEntry(
                LINUX_DESKTOP_ENTRY.to_owned(),
            ));
        if let Some(id) = replace_id {
            notification.id(id);
        }
        if activation_target.is_some() {
            notification.action("default", "Open");
        }
        match notification.show() {
            Ok(handle) => {
                if let Some(target) = activation_target {
                    handle.wait_for_action(|action| {
                        if action == "default" {
                            dispatch_click(target);
                        }
                    });
                }
            }
            Err(e) => tracing::warn!("Linux notification failed: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(channel_id: Option<&str>, clan_id: Option<&str>) -> Notification {
        Notification {
            title: "t".into(),
            body: "b".into(),
            channel_id: channel_id.map(str::to_owned),
            clan_id: clan_id.map(str::to_owned),
            link: None,
            icon_path: None,
        }
    }

    #[test]
    fn replacement_key_is_stable_per_channel() {
        let a = replacement_key(&notification(Some("12345"), Some("77")));
        assert_eq!(a.as_deref(), Some("mezon-notify-77-12345"));
        assert_eq!(a, replacement_key(&notification(Some("12345"), Some("77"))));
        assert_ne!(a, replacement_key(&notification(Some("12346"), Some("77"))));
    }

    #[test]
    fn replacement_key_rejects_absent_blank_and_zero_channel_ids() {
        assert_eq!(replacement_key(&notification(None, Some("77"))), None);
        assert_eq!(replacement_key(&notification(Some(""), Some("77"))), None);
        assert_eq!(
            replacement_key(&notification(Some("   "), Some("77"))),
            None
        );
        assert_eq!(replacement_key(&notification(Some("0"), Some("77"))), None);
    }

    #[test]
    fn identifier_round_trips_clan_and_channel() {
        let key = replacement_key(&notification(Some("12345"), Some("77"))).unwrap();
        assert_eq!(
            decode_identifier(&key),
            Some(NotificationTarget {
                clan_id: Some("77".into()),
                channel_id: "12345".into(),
                link: None,
            })
        );
    }

    #[test]
    fn identifier_round_trips_dm_without_clan() {
        let key = replacement_key(&notification(Some("12345"), None)).unwrap();
        assert_eq!(key, "mezon-notify-0-12345");
        assert_eq!(
            decode_identifier(&key),
            Some(NotificationTarget {
                clan_id: None,
                channel_id: "12345".into(),
                link: None,
            })
        );
    }

    #[test]
    fn decode_identifier_rejects_foreign_and_malformed_ids() {
        assert_eq!(decode_identifier("some-other-app-1-2"), None);
        assert_eq!(decode_identifier("mezon-notify"), None);
        assert_eq!(decode_identifier("mezon-notify-77"), None);
        assert_eq!(decode_identifier("mezon-notify-77-"), None);
        assert_eq!(decode_identifier(""), None);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_replace_id_is_stable_and_never_zero() {
        assert_eq!(
            linux_replace_id("mezon-channel-1"),
            linux_replace_id("mezon-channel-1")
        );
        assert_ne!(
            linux_replace_id("mezon-channel-1"),
            linux_replace_id("mezon-channel-2")
        );
        assert!(linux_replace_id("mezon-channel-1") > 0);
    }
}
