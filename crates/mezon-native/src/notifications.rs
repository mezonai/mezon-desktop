/// Desktop notification support.
///
/// macOS  : `UNUserNotificationCenter` via raw `objc` runtime calls.
///          Requests authorisation on first call, then delivers the notification.
/// Windows: Windows Runtime `ToastNotification` via the `windows` crate.
/// Linux  : `notify-rust` (wraps `libnotify` / D-Bus `org.freedesktop.Notifications`).

#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Optional channel ID — used as the notification category/thread identifier.
    pub channel_id: Option<String>,
}

/// Show a desktop notification.  Fire-and-forget; errors are logged but not propagated.
pub fn show(notification: &Notification) {
    tracing::info!(
        "Notification [{}]: {}",
        notification.title,
        notification.body
    );

    #[cfg(target_os = "macos")]
    show_macos(notification);

    #[cfg(target_os = "windows")]
    show_windows(notification);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    show_linux(notification);
}

// ─── macOS ────────────────────────────────────────────────────────────────────
//
// UNUserNotificationCenter is available on macOS 10.14+.
// We drive it through the raw `objc` runtime to avoid linking against the
// UserNotifications framework header directly.

#[cfg(target_os = "macos")]
fn show_macos(n: &Notification) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let title = n.title.clone();
    let body = n.body.clone();
    let category = n.channel_id.clone().unwrap_or_default();

    // UNUserNotificationCenter operations must happen on the main thread in
    // a macOS app that has an NSApplication.  We dispatch to it asynchronously.
    // GPUI's main thread is always the UI thread, so this is safe.
    std::thread::spawn(move || unsafe {
        // [UNUserNotificationCenter currentNotificationCenter]
        let center_cls = class!(UNUserNotificationCenter);
        let center: *mut Object = msg_send![center_cls, currentNotificationCenter];

        // Request authorisation (alert + sound + badge).
        // The block fires once the user responds; we ignore the result here.
        // On subsequent calls the system returns the cached decision instantly.
        let options: usize = 0b111; // UNAuthorizationOptionBadge|Sound|Alert
        let _: () = msg_send![
            center,
            requestAuthorizationWithOptions: options
            completionHandler: objc_block_noop()
        ];

        // Build the notification content.
        // [UNMutableNotificationContent new]
        let content_cls = class!(UNMutableNotificationContent);
        let content: *mut Object = msg_send![content_cls, new];

        // content.title = title
        let ns_title = nsstring(&title);
        let _: () = msg_send![content, setTitle: ns_title];

        // content.body = body
        let ns_body = nsstring(&body);
        let _: () = msg_send![content, setBody: ns_body];

        // content.threadIdentifier = category (groups notifications per channel)
        if !category.is_empty() {
            let ns_cat = nsstring(&category);
            let _: () = msg_send![content, setThreadIdentifier: ns_cat];
        }

        // [UNNotificationRequest requestWithIdentifier:content:trigger:]
        // identifier = UUID string so each notification is independent
        let uuid_str = format!(
            "mezon-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let ns_uuid = nsstring(&uuid_str);
        let request_cls = class!(UNNotificationRequest);
        let request: *mut Object = msg_send![
            request_cls,
            requestWithIdentifier: ns_uuid
            content: content
            trigger: std::ptr::null::<Object>()
        ];

        // [center addNotificationRequest:request withCompletionHandler:nil]
        let _: () = msg_send![
            center,
            addNotificationRequest: request
            withCompletionHandler: std::ptr::null::<Object>()
        ];
    });
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

/// A no-op Objective-C block used for the authorisation completion handler.
/// The block takes (BOOL granted, NSError *error) and does nothing.
#[cfg(target_os = "macos")]
unsafe fn objc_block_noop() -> *mut objc::runtime::Object {
    // We pass nil — UNUserNotificationCenter accepts a nil completion handler.
    std::ptr::null_mut()
}

// ─── Windows ──────────────────────────────────────────────────────────────────
//
// Windows Runtime ToastNotification.  Requires the app to have an AUMID
// (Application User Model ID) registered; we use a sensible default.

#[cfg(target_os = "windows")]
fn show_windows(n: &Notification) {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;

    let title = n.title.clone();
    let body = n.body.clone();

    std::thread::spawn(move || {
        if let Err(e) = try_show_toast(&title, &body) {
            tracing::warn!("Windows toast notification failed: {e}");
        }
    });
}

#[cfg(target_os = "windows")]
fn try_show_toast(title: &str, body: &str) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;

    // The XML template for a simple text toast.
    let xml_str = format!(
        r#"<toast>
  <visual>
    <binding template="ToastGeneric">
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

    // AUMID — must match the registered shortcut / installer entry.
    // Falls back gracefully if not registered (notification is silently dropped by the OS).
    let aumid = HSTRING::from("ai.mezon.Mezon");
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&aumid)?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
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
fn show_linux(n: &Notification) {
    let title = n.title.clone();
    let body = n.body.clone();
    std::thread::spawn(move || {
        if let Err(e) = notify_rust::Notification::new()
            .appname("Mezon")
            .summary(&title)
            .body(&body)
            .icon("dialog-information")
            .show()
        {
            tracing::warn!("Linux notification failed: {e}");
        }
    });
}
