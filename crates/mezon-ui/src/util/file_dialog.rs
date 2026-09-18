//! What the user sees when a native file dialog does not open.
//!
//! [`mezon_store::dialog`] decides *what happened*; this decides what to show. The
//! split exists because mezon-store cannot depend on mezon-ui, and the store needs
//! the same reading of a dialog result for the download and call-recording paths.

use futures::channel::oneshot;
use gpui::{App, AsyncApp};
use mezon_store::Settings;
use mezon_store::dialog::{DialogOutcome, classify_dialog};

use crate::app::shell::Shell;

pub(crate) const TOAST_KEY: &str = "file-dialog-unavailable";

/// Await a file dialog, returning what the user picked.
///
/// Cancelling gives `None` in silence. A dialog that never opened gives `None` too,
/// but logs the platform's own reason and shows the user one translated sentence —
/// the failure that used to be dropped on the floor.
pub(crate) async fn resolve<T>(
    receiver: oneshot::Receiver<anyhow::Result<Option<T>>>,
    cx: &AsyncApp,
) -> Option<T> {
    match classify_dialog(receiver.await) {
        DialogOutcome::Picked(picked) => Some(picked),
        DialogOutcome::Cancelled => None,
        DialogOutcome::Lost => {
            // The request was dropped without an answer, which happens while the app
            // is shutting down. The user is not waiting on anything, and touching the
            // app from here can outlive it, so log and stop.
            tracing::warn!("the file dialog went away before answering");
            None
        }
        DialogOutcome::Unavailable(failure) => {
            tracing::warn!("file dialog unavailable: {}", failure.reason);
            let portal_missing = failure.portal_missing;
            cx.update(|cx| {
                let message = unavailable_message(cx, portal_missing);
                if let Some(shell) = Shell::try_global(cx) {
                    shell.update(cx, |shell, cx| shell.error_once(TOAST_KEY, message, cx));
                }
            });
            None
        }
    }
}

pub(crate) fn unavailable_message(cx: &App, portal_missing: bool) -> &'static str {
    let locale = Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default();
    message_for(&locale, portal_missing)
}

/// Only name the package when a missing portal is the actual cause. A dialog that
/// timed out or was refused is not fixed by installing anything, and the platform
/// this runs on is not the question — the reason is.
fn message_for(locale: &str, portal_missing: bool) -> &'static str {
    let key = if portal_missing {
        "file.dialogPortalMissing"
    } else {
        "file.dialogUnavailable"
    };
    mezon_i18n::t(locale, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [&str; 16] = [
        "vi", "en", "ru", "ukr", "es", "tt", "de", "it", "pt", "jpn", "pl", "kr", "swe", "blr",
        "fr", "nl",
    ];

    #[test]
    fn the_toast_is_one_translated_sentence_with_no_raw_platform_text() {
        for locale in ["vi", "en", "jpn", "does-not-exist"] {
            for portal_missing in [true, false] {
                let message = message_for(locale, portal_missing);
                assert!(
                    !message.contains("xdg-desktop-portal implementation"),
                    "{locale} leaks gpui's raw sentence into a toast that already says it: {message}"
                );
                assert!(
                    !message.contains(": Couldn't") && !message.contains(": Could not"),
                    "{locale} states the same failure twice: {message}"
                );
                assert!(
                    !message.ends_with(':'),
                    "{locale} ends mid-sentence: {message}"
                );
            }
        }
    }

    #[test]
    fn every_locale_translates_both_messages() {
        for locale in LOCALES {
            for portal_missing in [true, false] {
                let message = message_for(locale, portal_missing);
                assert!(
                    !message.starts_with("file.dialog"),
                    "locale {locale} falls through to the raw key: {message}"
                );
            }
        }
    }

    #[test]
    fn a_missing_portal_names_the_package_to_install() {
        let message = message_for("en", true);
        assert!(
            message.contains("xdg-desktop-portal"),
            "the only fix is installing a portal backend, so name it: {message}"
        );
    }

    #[test]
    fn any_other_failure_does_not_send_the_user_after_a_package() {
        for locale in LOCALES {
            let message = message_for(locale, false);
            assert!(
                !message.contains("xdg-desktop-portal"),
                "{locale} blames a portal for a failure that has nothing to do with one: {message}"
            );
        }
    }
}
