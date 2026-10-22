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
    pub topic_reply: Option<TopicReply>,
    /// Local file path to the sender-avatar image, shown as the notification icon.
    pub icon_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicReply {
    pub topic_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTarget {
    pub clan_id: Option<String>,
    pub channel_id: String,
    pub link: Option<String>,
    pub topic_reply: Option<TopicReply>,
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

static TOPIC_REPLY_BY_ID: std::sync::Mutex<Option<std::collections::HashMap<String, TopicReply>>> =
    std::sync::Mutex::new(None);

fn remember_topic_reply(identifier: &str, topic_reply: Option<&TopicReply>) {
    if let Ok(mut guard) = TOPIC_REPLY_BY_ID.lock() {
        let replies = guard.get_or_insert_with(std::collections::HashMap::new);
        match topic_reply {
            Some(reply) => replies.insert(identifier.to_owned(), reply.clone()),
            None => replies.remove(identifier),
        };
    }
}

fn build_target(identifier: &str) -> Option<NotificationTarget> {
    let mut target = decode_identifier(identifier)?;
    target.link = LINK_BY_ID
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.get(identifier).cloned()));
    target.topic_reply = TOPIC_REPLY_BY_ID
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
        topic_reply: None,
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

    if let Some(key) = replacement_key(notification) {
        if let Some(link) = notification.link.as_deref() {
            remember_link(&key, link);
        }
        remember_topic_reply(&key, notification.topic_reply.as_ref());
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
const PRESENT_BANNER_LIST_SOUND: usize = (1 << 4) | (1 << 3) | (1 << 1);

#[cfg(target_os = "macos")]
extern "C" fn will_present_notification(
    _this: &objc::runtime::Object,
    _sel: objc::runtime::Sel,
    _center: *mut objc::runtime::Object,
    _notification: *mut objc::runtime::Object,
    completion: *mut objc::runtime::Object,
) {
    tracing::debug!(target: "noti", "presenting a notification while the app is active");
    if completion.is_null() {
        return;
    }
    unsafe {
        let block = completion as *mut block::Block<(usize,), ()>;
        (*block).call((PRESENT_BANNER_LIST_SOUND,));
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
        let present: extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object) =
            will_present_notification;
        decl.add_method(
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
            present,
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
        tracing::warn!(
            target: "noti",
            "notification setup skipped: not running from an app bundle, so macOS will \
             refuse every notification this process posts"
        );
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
                tracing::info!(target: "noti", "macOS notification authorisation granted");
                NOTIFICATION_AUTH.store(AUTH_GRANTED, std::sync::atomic::Ordering::Relaxed);
            } else {
                tracing::warn!(
                    target: "noti",
                    "macOS notification authorisation DENIED — no notification will appear. \
                     Enable it in System Settings > Notifications > Mezon, then restart the app."
                );
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
        tracing::warn!(
            target: "noti",
            title = %n.title,
            "notification dropped: not running from an app bundle"
        );
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

        // A null completion handler throws the OS's verdict away; with one we learn
        // whether macOS actually accepted the request, which is otherwise invisible.
        let title = n.title.clone();
        let handler = block::ConcreteBlock::new(move |error: *mut Object| {
            if error.is_null() {
                tracing::info!(target: "noti", title = %title, "macOS accepted the notification");
            } else {
                let description: *mut Object = msg_send![error, localizedDescription];
                let reason = nsstring_to_string(description).unwrap_or_default();
                tracing::warn!(
                    target: "noti",
                    title = %title,
                    "macOS rejected the notification: {reason}"
                );
            }
        });
        let handler = handler.copy();
        let _: () = msg_send![
            center,
            addNotificationRequest: request
            withCompletionHandler: &*handler
        ];
        leak_for_async_objc_callback(handler);
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
fn running_packaged() -> bool {
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    let mut len = 0u32;
    unsafe { GetCurrentPackageFullName(&mut len, None) == ERROR_INSUFFICIENT_BUFFER }
}

#[cfg(target_os = "windows")]
fn init_windows() {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::HSTRING;

    if running_packaged() {
        tracing::debug!("packaged install detected; toasts use the package identity");
        return;
    }

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

    tracing::info!("wrote Start Menu shortcut with AppUserModelID for toast notifications");
    Ok(())
}

/// `ToastNotification::Activated` is an in-process COM callback, so it can only
/// arrive while the apartment that raised the toast is still alive.
///
/// Raising a toast from a thread that calls `CoInitializeEx(APARTMENTTHREADED)`
/// and then `CoUninitialize` before exiting tears that apartment down
/// immediately, which is why the click only dismissed the toast. Leaving the
/// thread apartment-less instead makes `windows-rs` fall back to
/// `CoIncrementMTAUsage` when it resolves the activation factory, and that
/// keeps an implicit MTA alive for the rest of the process, so the callback
/// still has somewhere to land after the thread is gone.
#[cfg(target_os = "windows")]
fn ensure_toast_apartment() {
    use windows::Win32::System::Com::CoIncrementMTAUsage;

    static MTA: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    MTA.get_or_init(|| unsafe {
        if let Err(e) = CoIncrementMTAUsage() {
            tracing::warn!("could not join the process MTA for toasts: {e}");
        }
    });
}

#[cfg(target_os = "windows")]
fn retain_toast(toast: windows::UI::Notifications::ToastNotification) {
    use windows::UI::Notifications::ToastNotification;

    const LIVE_TOAST_LIMIT: usize = 32;

    static LIVE: std::sync::Mutex<Vec<ToastNotification>> = std::sync::Mutex::new(Vec::new());

    let Ok(mut live) = LIVE.lock() else {
        return;
    };
    if live.len() >= LIVE_TOAST_LIMIT {
        live.remove(0);
    }
    live.push(toast);
}

#[cfg(target_os = "windows")]
fn show_windows(n: &Notification) {
    let title = n.title.clone();
    let body = n.body.clone();
    let tag = replacement_key(n);
    let icon_path = n.icon_path.clone();

    std::thread::spawn(move || {
        ensure_toast_apartment();
        match try_show_toast(&title, &body, tag.as_deref(), icon_path.as_deref()) {
            Ok(toast) => retain_toast(toast),
            Err(e) => tracing::warn!("Windows toast notification failed: {e}"),
        }
    });
}

#[cfg(target_os = "windows")]
fn try_show_toast(
    title: &str,
    body: &str,
    tag: Option<&str>,
    icon_path: Option<&str>,
) -> windows::core::Result<windows::UI::Notifications::ToastNotification> {
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

    let notifier = if running_packaged() {
        ToastNotificationManager::CreateToastNotifier()?
    } else {
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))?
    };
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    if let Some(tag) = tag {
        toast.SetTag(&HSTRING::from(windows_toast_tag(tag).as_str()))?;
        toast.SetGroup(&HSTRING::from(WINDOWS_TOAST_GROUP))?;
    }

    let activation_target = tag.and_then(build_target);
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
    Ok(toast)
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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LINUX_DESKTOP_ENTRY: &str = "mezon";

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LINUX_COMMAND_CAPACITY: usize = 256;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const FDO_NOTIFICATIONS: &str = "org.freedesktop.Notifications";

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct LinuxToast {
    title: String,
    body: String,
    icon: String,
    channel: Option<String>,
    target: Option<NotificationTarget>,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
enum LinuxCommand {
    Show(Box<LinuxToast>),
    CloseChannel(String),
    CloseAll,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct LiveToast {
    channel: Option<String>,
    target: Option<NotificationTarget>,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Default)]
struct LinuxToastBook {
    by_channel: std::collections::HashMap<String, u32>,
    by_id: std::collections::HashMap<u32, LiveToast>,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl LinuxToastBook {
    fn replaces_id(&self, channel: Option<&str>) -> u32 {
        channel
            .and_then(|channel| self.by_channel.get(channel).copied())
            .unwrap_or(0)
    }

    fn shown(
        &mut self,
        channel: Option<String>,
        id: u32,
        target: Option<NotificationTarget>,
    ) -> Option<u32> {
        let superseded = channel
            .as_ref()
            .and_then(|channel| self.by_channel.insert(channel.clone(), id))
            .filter(|previous| *previous != id);
        if let Some(previous) = superseded {
            self.by_id.remove(&previous);
        }
        self.by_id.insert(id, LiveToast { channel, target });
        superseded
    }

    fn take(&mut self, id: u32) -> Option<LiveToast> {
        let toast = self.by_id.remove(&id)?;
        if let Some(channel) = toast.channel.as_deref()
            && self.by_channel.get(channel) == Some(&id)
        {
            self.by_channel.remove(channel);
        }
        Some(toast)
    }

    fn take_channel(&mut self, channel: &str) -> Option<u32> {
        let id = self.by_channel.remove(channel)?;
        self.by_id.remove(&id);
        Some(id)
    }

    fn take_all(&mut self) -> Vec<u32> {
        self.by_channel.clear();
        self.by_id.drain().map(|(id, _)| id).collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_channel_key(channel_id: &str) -> Option<String> {
    let id = channel_id.trim();
    if id.is_empty() || id == "0" {
        None
    } else {
        Some(id.to_owned())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn notification_clicked_action(action: &str) -> bool {
    action.is_empty() || action == "default"
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
static LINUX_WORKER: std::sync::OnceLock<flume::Sender<LinuxCommand>> = std::sync::OnceLock::new();

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_worker() -> &'static flume::Sender<LinuxCommand> {
    LINUX_WORKER.get_or_init(|| {
        let (commands, inbox) = flume::bounded(LINUX_COMMAND_CAPACITY);
        let spawned = std::thread::Builder::new()
            .name("mezon-notifications".into())
            .spawn(move || zbus::block_on(run_linux_worker(inbox)));
        if let Err(e) = spawned {
            tracing::warn!("Linux notifications: worker thread failed to start: {e}");
        }
        commands
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn send_linux(worker: &flume::Sender<LinuxCommand>, command: LinuxCommand) {
    if let Err(e) = worker.try_send(command) {
        tracing::warn!("Linux notifications: command dropped: {e}");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct LinuxBus {
    connection: zbus::Connection,
    signals: zbus::MessageStream,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn open_linux_bus() -> Option<LinuxBus> {
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(e) => {
            tracing::warn!("Linux notifications: no D-Bus session: {e}");
            return None;
        }
    };
    let proxy = zbus::fdo::DBusProxy::new(&connection).await.ok()?;
    for member in ["ActionInvoked", "NotificationClosed"] {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(FDO_NOTIFICATIONS)
            .ok()?
            .member(member)
            .ok()?
            .build();
        if let Err(e) = proxy.add_match_rule(rule).await {
            tracing::warn!("Linux notifications: cannot watch {member}: {e}");
            return None;
        }
    }
    let signals = zbus::MessageStream::from(&connection);
    Some(LinuxBus {
        connection,
        signals,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
enum LinuxEvent {
    Command(LinuxCommand),
    Signal(zbus::Message),
    SignalError,
    BusClosed,
    WorkerClosed,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn next_linux_event(
    inbox: &flume::Receiver<LinuxCommand>,
    bus: Option<&mut LinuxBus>,
) -> LinuxEvent {
    use futures_lite::StreamExt as _;
    let command = async {
        inbox
            .recv_async()
            .await
            .map_or(LinuxEvent::WorkerClosed, LinuxEvent::Command)
    };
    let Some(bus) = bus else {
        return command.await;
    };
    let signal = async {
        match bus.signals.next().await {
            Some(Ok(message)) => LinuxEvent::Signal(message),
            Some(Err(_)) => LinuxEvent::SignalError,
            None => LinuxEvent::BusClosed,
        }
    };
    futures_lite::future::or(command, signal).await
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn run_linux_worker(inbox: flume::Receiver<LinuxCommand>) {
    let mut book = LinuxToastBook::default();
    let mut bus: Option<LinuxBus> = None;
    loop {
        match next_linux_event(&inbox, bus.as_mut()).await {
            LinuxEvent::Command(command) => {
                if bus.is_none() && matches!(command, LinuxCommand::Show(_)) {
                    bus = open_linux_bus().await;
                }
                if let Some(bus) = bus.as_ref() {
                    run_linux_command(&bus.connection, &mut book, command).await;
                }
            }
            LinuxEvent::Signal(message) => {
                if let Some(clicked) = handle_linux_signal(&mut book, &message)
                    && let Some(bus) = bus.as_ref()
                {
                    dbus_close_id(&bus.connection, clicked).await;
                }
            }
            LinuxEvent::SignalError => {}
            LinuxEvent::BusClosed => {
                bus = None;
                book = LinuxToastBook::default();
            }
            LinuxEvent::WorkerClosed => return,
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn run_linux_command(
    connection: &zbus::Connection,
    book: &mut LinuxToastBook,
    command: LinuxCommand,
) {
    match command {
        LinuxCommand::Show(toast) => {
            let replaces_id = book.replaces_id(toast.channel.as_deref());
            match dbus_notify(connection, &toast, replaces_id).await {
                Ok(id) if id > 0 => {
                    if let Some(superseded) = book.shown(toast.channel, id, toast.target) {
                        dbus_close_id(connection, superseded).await;
                    }
                }
                Ok(_) => tracing::warn!("Linux notification: daemon returned id=0"),
                Err(e) => tracing::warn!("Linux notification failed: {e}"),
            }
        }
        LinuxCommand::CloseChannel(channel) => {
            if let Some(id) = book.take_channel(&channel) {
                dbus_close_id(connection, id).await;
            }
        }
        LinuxCommand::CloseAll => {
            for id in book.take_all() {
                dbus_close_id(connection, id).await;
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn handle_linux_signal(book: &mut LinuxToastBook, message: &zbus::Message) -> Option<u32> {
    let header = message.header();
    if header.message_type() != zbus::message::Type::Signal
        || header
            .interface()
            .is_none_or(|interface| interface != FDO_NOTIFICATIONS)
    {
        return None;
    }
    match header.member().map(|member| member.as_str()) {
        Some("ActionInvoked") => {
            let (id, action) = message.body().deserialize::<(u32, String)>().ok()?;
            let toast = book.take(id)?;
            if notification_clicked_action(&action)
                && let Some(target) = toast.target
            {
                dispatch_click(target);
            }
            Some(id)
        }
        Some("NotificationClosed") => {
            if let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>() {
                book.take(id);
            }
            None
        }
        _ => None,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn dbus_notify(
    connection: &zbus::Connection,
    toast: &LinuxToast,
    replaces_id: u32,
) -> Result<u32, zbus::Error> {
    let actions: &[&str] = if toast.target.is_some() {
        &["default", "Open"]
    } else {
        &[]
    };
    let mut hints = std::collections::HashMap::<&str, zbus::zvariant::Value<'_>>::new();
    hints.insert(
        "desktop-entry",
        zbus::zvariant::Value::Str(LINUX_DESKTOP_ENTRY.into()),
    );
    if replaces_id == 0 {
        hints.insert(
            "sound-name",
            zbus::zvariant::Value::Str("message-new-instant".into()),
        );
    }
    connection
        .call_method(
            Some(FDO_NOTIFICATIONS),
            "/org/freedesktop/Notifications",
            Some(FDO_NOTIFICATIONS),
            "Notify",
            &(
                "Mezon",
                replaces_id,
                toast.icon.as_str(),
                toast.title.as_str(),
                toast.body.as_str(),
                actions,
                &hints,
                -1i32,
            ),
        )
        .await?
        .body()
        .deserialize()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn dbus_close_id(connection: &zbus::Connection, id: u32) {
    if let Err(e) = connection
        .call_method(
            Some(FDO_NOTIFICATIONS),
            "/org/freedesktop/Notifications",
            Some(FDO_NOTIFICATIONS),
            "CloseNotification",
            &(id,),
        )
        .await
    {
        tracing::debug!("Linux notification close failed: {e}");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn show_linux(n: &Notification) {
    let toast = LinuxToast {
        title: n.title.clone(),
        body: n.body.clone(),
        icon: n
            .icon_path
            .clone()
            .unwrap_or_else(|| LINUX_DESKTOP_ENTRY.to_owned()),
        channel: n.channel_id.as_deref().and_then(linux_channel_key),
        target: replacement_key(n).as_deref().and_then(build_target),
    };
    send_linux(linux_worker(), LinuxCommand::Show(Box::new(toast)));
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn channel_identifiers(clan_id: Option<&str>, channel_id: &str) -> Vec<String> {
    let channel = channel_id.trim();
    if channel.is_empty() || channel == "0" {
        return Vec::new();
    }
    let mut identifiers = vec![encode_identifier(None, channel)];
    if let Some(clan) = clan_id
        .map(str::trim)
        .filter(|clan| !clan.is_empty() && *clan != "0")
    {
        identifiers.push(encode_identifier(Some(&clan.to_owned()), channel));
    }
    identifiers
}

pub fn close_channel(clan_id: Option<&str>, channel_id: &str) {
    #[cfg(target_os = "macos")]
    remove_delivered_macos(Some(&channel_identifiers(clan_id, channel_id)));

    #[cfg(target_os = "windows")]
    send_windows_close(WindowsClose::Tags(
        channel_identifiers(clan_id, channel_id)
            .iter()
            .map(|identifier| windows_toast_tag(identifier))
            .collect(),
    ));

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = clan_id;
        if let (Some(worker), Some(channel)) = (LINUX_WORKER.get(), linux_channel_key(channel_id)) {
            send_linux(worker, LinuxCommand::CloseChannel(channel));
        }
    }
}

pub fn close_all() {
    #[cfg(target_os = "macos")]
    remove_delivered_macos(None);

    #[cfg(target_os = "windows")]
    send_windows_close(WindowsClose::All);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    if let Some(worker) = LINUX_WORKER.get() {
        send_linux(worker, LinuxCommand::CloseAll);
    }
}

#[cfg(target_os = "macos")]
fn remove_delivered_macos(identifiers: Option<&[String]>) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    if identifiers.is_some_and(<[String]>::is_empty) || !has_bundle_identifier() {
        return;
    }
    unsafe {
        let center: *mut Object =
            msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        let Some(identifiers) = identifiers else {
            let _: () = msg_send![center, removeAllDeliveredNotifications];
            return;
        };
        let strings: Vec<*mut Object> = identifiers.iter().map(|id| nsstring(id)).collect();
        let array: *mut Object = msg_send![
            class!(NSArray),
            arrayWithObjects: strings.as_ptr()
            count: strings.len()
        ];
        let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: array];
        for string in strings {
            let _: () = msg_send![string, release];
        }
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_TOAST_GROUP: &str = "mezon";

#[cfg(target_os = "windows")]
fn windows_toast_tag(identifier: &str) -> String {
    identifier.chars().take(64).collect()
}

#[cfg(target_os = "windows")]
enum WindowsClose {
    Tags(Vec<String>),
    All,
}

#[cfg(target_os = "windows")]
fn send_windows_close(close: WindowsClose) {
    static CLOSER: std::sync::OnceLock<flume::Sender<WindowsClose>> = std::sync::OnceLock::new();
    let closer = CLOSER.get_or_init(|| {
        let (closer, inbox) = flume::bounded::<WindowsClose>(256);
        let spawned = std::thread::Builder::new()
            .name("mezon-toast-close".into())
            .spawn(move || {
                ensure_toast_apartment();
                for close in inbox.iter() {
                    if let Err(e) = remove_windows_toasts(&close) {
                        tracing::debug!("Windows toast removal failed: {e}");
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("Windows toast closer failed to start: {e}");
        }
        closer
    });
    if let Err(e) = closer.try_send(close) {
        tracing::warn!("Windows toast removal dropped: {e}");
    }
}

#[cfg(target_os = "windows")]
fn remove_windows_toasts(close: &WindowsClose) -> windows::core::Result<()> {
    use windows::UI::Notifications::ToastNotificationManager;
    use windows::core::HSTRING;

    let history = ToastNotificationManager::History()?;
    let group = HSTRING::from(WINDOWS_TOAST_GROUP);
    let app = HSTRING::from(APP_USER_MODEL_ID);
    let packaged = running_packaged();
    match close {
        WindowsClose::Tags(tags) => {
            for tag in tags {
                let tag = HSTRING::from(tag.as_str());
                if packaged {
                    history.RemoveGroupedTag(&tag, &group)?;
                } else {
                    history.RemoveGroupedTagWithId(&tag, &group, &app)?;
                }
            }
        }
        WindowsClose::All => {
            if packaged {
                history.RemoveGroup(&group)?;
            } else {
                history.RemoveGroupWithId(&group, &app)?;
            }
        }
    }
    Ok(())
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
            topic_reply: None,
            icon_path: None,
        }
    }

    #[test]
    fn closing_a_channel_covers_the_clan_resolved_at_show_time_or_not() {
        assert_eq!(
            channel_identifiers(Some("77"), "12345"),
            vec![
                "mezon-notify-0-12345".to_owned(),
                "mezon-notify-77-12345".to_owned()
            ]
        );
        assert_eq!(
            channel_identifiers(None, " 12345 "),
            vec!["mezon-notify-0-12345".to_owned()]
        );
        assert_eq!(
            channel_identifiers(Some("0"), "12345"),
            vec!["mezon-notify-0-12345".to_owned()]
        );
        assert!(channel_identifiers(Some("77"), "0").is_empty());
        assert!(channel_identifiers(Some("77"), "  ").is_empty());
        for shown in [
            replacement_key(&notification(Some("12345"), Some("77"))),
            replacement_key(&notification(Some("12345"), None)),
        ] {
            assert!(channel_identifiers(Some("77"), "12345").contains(&shown.unwrap()));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn target(channel_id: &str) -> Option<NotificationTarget> {
        Some(NotificationTarget {
            clan_id: Some("77".into()),
            channel_id: channel_id.into(),
            link: None,
            topic_reply: None,
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_channel_key_ignores_the_clan_and_rejects_blank_ids() {
        assert_eq!(linux_channel_key(" 12345 ").as_deref(), Some("12345"));
        assert_eq!(linux_channel_key("0"), None);
        assert_eq!(linux_channel_key(""), None);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn a_channel_replaces_its_live_toast_in_place() {
        let mut book = LinuxToastBook::default();
        assert_eq!(book.replaces_id(Some("a")), 0);
        assert_eq!(book.shown(Some("a".into()), 41, target("a")), None);
        assert_eq!(book.replaces_id(Some("a")), 41);
        assert_eq!(book.shown(Some("a".into()), 41, target("a")), None);
        assert_eq!(book.shown(Some("b".into()), 77, target("b")), None);
        assert_eq!(book.replaces_id(Some("a")), 41);
        assert_eq!(book.replaces_id(Some("b")), 77);
        assert_eq!(book.replaces_id(None), 0);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn a_toast_shown_under_a_new_id_supersedes_the_old_one_and_its_click() {
        let mut book = LinuxToastBook::default();
        book.shown(Some("a".into()), 41, target("a"));
        assert_eq!(book.shown(Some("a".into()), 42, target("a")), Some(41));
        assert!(book.take(41).is_none());
        assert_eq!(book.replaces_id(Some("a")), 42);
        assert!(book.take(42).is_some_and(|toast| toast.target.is_some()));
        assert_eq!(book.replaces_id(Some("a")), 0);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn closing_a_channel_takes_only_its_toast() {
        let mut book = LinuxToastBook::default();
        book.shown(Some("a".into()), 41, target("a"));
        book.shown(Some("b".into()), 77, target("b"));
        book.shown(None, 90, None);
        assert_eq!(book.take_channel("a"), Some(41));
        assert_eq!(book.take_channel("a"), None);
        assert_eq!(book.replaces_id(Some("b")), 77);
        let mut rest = book.take_all();
        rest.sort_unstable();
        assert_eq!(rest, vec![77, 90]);
        assert_eq!(book.replaces_id(Some("b")), 0);
        assert!(book.take_all().is_empty());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn a_dismissed_toast_lets_the_next_one_in_its_channel_play_a_sound() {
        let mut book = LinuxToastBook::default();
        book.shown(Some("a".into()), 41, target("a"));
        assert!(book.take(41).is_some());
        assert_eq!(book.replaces_id(Some("a")), 0);
        assert!(book.take(41).is_none());
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
                topic_reply: None,
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
                topic_reply: None,
            })
        );
    }

    #[test]
    fn a_click_reopens_the_topic_reply_until_the_channel_posts_outside_it() {
        let reply = TopicReply {
            topic_id: "2088098374732484608".into(),
            message_id: "1840651254099873792".into(),
        };
        let key = "mezon-notify-77-424242";
        remember_topic_reply(key, Some(&reply));
        assert_eq!(build_target(key).unwrap().topic_reply, Some(reply));
        remember_topic_reply(key, None);
        assert_eq!(build_target(key).unwrap().topic_reply, None);
    }

    #[test]
    fn decode_identifier_rejects_foreign_and_malformed_ids() {
        assert_eq!(decode_identifier("some-other-app-1-2"), None);
        assert_eq!(decode_identifier("mezon-notify"), None);
        assert_eq!(decode_identifier("mezon-notify-77"), None);
        assert_eq!(decode_identifier("mezon-notify-77-"), None);
        assert_eq!(decode_identifier(""), None);
    }
}
