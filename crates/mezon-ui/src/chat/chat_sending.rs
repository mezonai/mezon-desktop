use gpui::{App, Entity};
use mezon_store::{AuthState, MessagesStore, OutgoingAttachment, OutgoingContent, OutgoingOgp};

pub struct ChatSending;

impl ChatSending {
    fn current_user(auth_state: &Entity<AuthState>, cx: &App) -> (String, String) {
        match auth_state.read(cx) {
            AuthState::Authenticated(session) => {
                (session.user_id.clone(), session.username.clone())
            }
            _ => (String::new(), String::new()),
        }
    }

    pub fn send_text(
        content: impl Into<String>,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        ogp: Option<OutgoingOgp>,
        auth_state: &Entity<AuthState>,
        cx: &mut App,
    ) {
        let content = content.into();
        if content.is_empty() && content_tokens.is_empty() && attachments.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_message(content, uid, uname, content_tokens, attachments, ogp, cx);
        });
    }

    pub fn send_ephemeral(
        receiver_id: i64,
        content: impl Into<String>,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        cx: &mut App,
    ) {
        let content = content.into();
        if content.is_empty() && content_tokens.is_empty() && attachments.is_empty() {
            return;
        }
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_ephemeral_message(receiver_id, content, content_tokens, attachments, cx);
        });
    }

    pub fn send_sticker(
        url: String,
        filename: String,
        auth_state: &Entity<AuthState>,
        cx: &mut App,
    ) {
        if url.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_sticker(url, filename, uid, uname, cx);
        });
    }

    pub fn send_gif(
        url: String,
        width: u32,
        height: u32,
        auth_state: &Entity<AuthState>,
        cx: &mut App,
    ) {
        if url.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_gif(url, width as i32, height as i32, uid, uname, cx);
        });
    }

    pub fn send_sound(url: String, filename: String, auth_state: &Entity<AuthState>, cx: &mut App) {
        if url.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_sound(url, filename, uid, uname, cx);
        });
    }

    pub fn send_buzz(content: impl Into<String>, cx: &mut App) {
        let content = content.into();
        if content.trim().is_empty() {
            return;
        }
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_buzz_message(content, cx);
        });
    }

    pub fn send_location(latitude: f64, longitude: f64, cx: &mut App) {
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_location_message(latitude, longitude, cx);
        });
    }
}
