//! One reading of what a native file dialog just did.
//!
//! `prompt_for_paths`/`prompt_for_new_path` hand back a nested
//! `Result<anyhow::Result<Option<T>>, Canceled>` whose four states mean four very
//! different things to a user. Every call site used to flatten them into "no path,
//! give up", which is how a Linux box without an `xdg-desktop-portal` backend turned
//! every file button into a dead one. This module is the single place that tells them
//! apart, so the UI layer only has to decide what to *show*.

use futures::channel::oneshot;

/// Text gpui puts in the error when the portal itself is missing
/// (`FILE_PICKER_PORTAL_MISSING`). It is the one failure the user can act on, so it
/// gets its own message; everything else on the same platform is a different problem
/// and must not be reported as a missing package.
const PORTAL_HINT: &str = "xdg-desktop-portal";

pub struct DialogFailure {
    /// The platform's own sentence. For the log, never for a toast: it is English and
    /// often says what our own translated message already says.
    pub reason: String,
    /// The dialog could not run because no desktop portal is installed.
    pub portal_missing: bool,
}

pub enum DialogOutcome<T> {
    Picked(T),
    /// The user dismissed the dialog. Nothing to report.
    Cancelled,
    /// The request was dropped without an answer — the app is shutting down, or the
    /// dialog went away with it. Worth a log line, but the user asked for nothing and
    /// there is nothing actionable to tell them.
    Lost,
    /// The dialog never opened.
    Unavailable(DialogFailure),
}

pub fn classify_dialog<T>(
    result: Result<anyhow::Result<Option<T>>, oneshot::Canceled>,
) -> DialogOutcome<T> {
    match result {
        Ok(Ok(Some(picked))) => DialogOutcome::Picked(picked),
        Ok(Ok(None)) => DialogOutcome::Cancelled,
        Ok(Err(error)) => {
            let reason = error.to_string();
            let portal_missing = reason.contains(PORTAL_HINT);
            DialogOutcome::Unavailable(DialogFailure {
                reason,
                portal_missing,
            })
        }
        Err(oneshot::Canceled) => DialogOutcome::Lost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const PORTAL_MISSING: &str =
        "Couldn't open file picker due to missing xdg-desktop-portal implementation.";

    type DialogResult = Result<anyhow::Result<Option<PathBuf>>, oneshot::Canceled>;

    #[test]
    fn a_chosen_path_comes_back_untouched() {
        let chosen = PathBuf::from("/home/u/Downloads/ca.crt");
        match classify_dialog(Ok(Ok(Some(chosen.clone()))) as DialogResult) {
            DialogOutcome::Picked(path) => assert_eq!(path, chosen),
            _ => panic!("picking a destination must not read as a failure"),
        }
    }

    #[test]
    fn dismissing_the_dialog_is_an_answer_not_a_failure() {
        match classify_dialog(Ok(Ok(None)) as DialogResult) {
            DialogOutcome::Cancelled => {}
            _ => panic!("changing your mind must stay silent"),
        }
    }

    #[test]
    fn a_missing_portal_is_named_as_such() {
        let failed: DialogResult = Ok(Err(anyhow::anyhow!(PORTAL_MISSING)));
        match classify_dialog(failed) {
            DialogOutcome::Unavailable(failure) => {
                assert!(failure.portal_missing);
                assert_eq!(failure.reason, PORTAL_MISSING);
            }
            _ => panic!("a picker that could not run must be reported"),
        }
    }

    #[test]
    fn another_failure_on_the_same_platform_is_not_blamed_on_the_portal() {
        let failed: DialogResult = Ok(Err(anyhow::anyhow!("Connection timed out")));
        match classify_dialog(failed) {
            DialogOutcome::Unavailable(failure) => assert!(
                !failure.portal_missing,
                "installing a package fixes nothing here, so do not ask for one"
            ),
            _ => panic!("a timed-out dialog still failed"),
        }
    }

    #[test]
    fn losing_the_channel_is_not_something_to_show_the_user() {
        let (sender, mut receiver) = oneshot::channel::<anyhow::Result<Option<PathBuf>>>();
        drop(sender);
        let dropped = receiver
            .try_recv()
            .map(|value| value.expect("a dropped sender delivers nothing"));
        match classify_dialog(dropped) {
            DialogOutcome::Lost => {}
            DialogOutcome::Unavailable(failure) => panic!(
                "a dropped request is not a broken portal, so do not tell the user to install one: {}",
                failure.reason
            ),
            _ => panic!("losing the channel is neither a pick nor a cancel"),
        }
    }
}
