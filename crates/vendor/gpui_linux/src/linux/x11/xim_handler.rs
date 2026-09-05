use std::default::Default;
use std::ops::Range;

use x11rb::protocol::{Event, xproto};
use xim::{
    AHashMap, AttributeName, Client, ClientError, ClientHandler, InputStyle, InputStyleList,
    Reader, XimRead,
};

use super::im_frontend::caret_utf16_range;

pub enum XimCallbackEvent {
    XimXEvent(x11rb::protocol::Event),
    XimPreeditEvent(xproto::Window, String, Option<Range<usize>>),
    XimCommitEvent(xproto::Window, String),
}

fn apply_preedit_draw(
    preedit: &mut Vec<char>,
    chg_first: i32,
    chg_len: i32,
    status: xim::PreeditDrawStatus,
    replacement: &str,
) -> String {
    let raw = if status.contains(xim::PreeditDrawStatus::NO_STRING) {
        Vec::new()
    } else {
        replacement.chars().collect::<Vec<_>>()
    };
    let len = preedit.len();
    let start = usize::try_from(chg_first).unwrap_or(0).min(len);
    let delete_len = if chg_len < 0 {
        len.saturating_sub(start)
    } else {
        usize::try_from(chg_len)
            .unwrap_or(0)
            .min(len.saturating_sub(start))
    };
    preedit.splice(start..start + delete_len, raw);
    preedit.iter().collect()
}

fn pick_input_style(attributes: &AHashMap<AttributeName, Vec<u8>>) -> InputStyle {
    let styles = attributes
        .get(&AttributeName::QueryInputStyle)
        .and_then(|bytes| {
            let mut reader = Reader::new(bytes);
            InputStyleList::read(&mut reader).ok()
        })
        .map(|list| list.styles)
        .unwrap_or_default();

    let preferred = [
        InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
        InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NONE,
        InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_CALLBACKS,
        InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NOTHING,
        InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NONE,
        InputStyle::PREEDIT_POSITION | InputStyle::STATUS_AREA,
        InputStyle::PREEDIT_NOTHING | InputStyle::STATUS_NOTHING,
    ];
    for want in preferred {
        if styles.iter().any(|style| *style == want) {
            return want;
        }
    }
    for style in &styles {
        if style.contains(InputStyle::PREEDIT_CALLBACKS) {
            return InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING;
        }
    }
    for style in &styles {
        if style.contains(InputStyle::PREEDIT_POSITION) {
            return InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NOTHING;
        }
    }
    InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING
}

fn usable_locale(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "C" && value != "POSIX" && !value.starts_with("C.")
}

fn xim_locale_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if usable_locale(trimmed) && !candidates.iter().any(|c| c == trimmed) {
                candidates.push(trimmed.to_string());
            }
        }
    }
    for fallback in ["en_US.UTF-8", "C"] {
        if !candidates.iter().any(|c| c == fallback) {
            candidates.push(fallback.to_string());
        }
    }
    candidates
}

pub struct XimHandler {
    pub im_id: u16,
    pub ic_id: u16,
    pub connected: bool,
    pub styles_ready: bool,
    pub opened: bool,
    pub ic_pending: bool,
    pub created_at: std::time::Instant,
    locale_candidates: Vec<String>,
    locale_attempt: usize,
    pub window: xproto::Window,
    pub input_style: InputStyle,
    pub callback_events: Vec<XimCallbackEvent>,
    pub needs_spot_refresh: bool,
    preedit: Vec<char>,
    last_applied: Option<(String, Option<std::ops::Range<usize>>)>,
}

impl XimHandler {
    pub fn new() -> Self {
        Self {
            im_id: Default::default(),
            ic_id: Default::default(),
            connected: false,
            styles_ready: false,
            opened: false,
            ic_pending: false,
            created_at: std::time::Instant::now(),
            locale_candidates: xim_locale_candidates(),
            locale_attempt: 0,
            window: Default::default(),
            input_style: InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            callback_events: Vec::new(),
            needs_spot_refresh: false,
            preedit: Vec::new(),
            last_applied: None,
        }
    }

    fn push_callback(&mut self, event: XimCallbackEvent) {
        self.callback_events.push(event);
    }

    pub fn take_callbacks(&mut self) -> Vec<XimCallbackEvent> {
        std::mem::take(&mut self.callback_events)
    }

    fn emit_preedit(&mut self, text: String, caret: i32) {
        let caret = caret_utf16_range(&text, caret);
        self.push_callback(XimCallbackEvent::XimPreeditEvent(self.window, text, caret));
    }

    fn clear_preedit(&mut self) {
        self.preedit.clear();
    }

    pub fn should_skip_apply(&self, text: &str, caret: Option<&std::ops::Range<usize>>) -> bool {
        self.last_applied
            .as_ref()
            .is_some_and(|(applied, applied_caret)| {
                applied == text && applied_caret.as_ref() == caret
            })
    }

    pub fn remember_applied(&mut self, text: &str, caret: Option<std::ops::Range<usize>>) {
        self.last_applied = Some((text.to_string(), caret));
    }

    pub fn clear_applied(&mut self) {
        self.last_applied = None;
    }

    pub fn try_reopen_next_locale<C: Client>(&mut self, client: &mut C) -> bool {
        if self.opened {
            return false;
        }
        self.locale_attempt += 1;
        let Some(locale) = self.locale_candidates.get(self.locale_attempt) else {
            return false;
        };
        client.open(locale).is_ok()
    }
}

impl<C: Client<XEvent = xproto::KeyPressEvent>> ClientHandler<C> for XimHandler {
    fn handle_connect(&mut self, client: &mut C) -> Result<(), ClientError> {
        let locale = self
            .locale_candidates
            .get(self.locale_attempt)
            .cloned()
            .unwrap_or_else(|| "en_US.UTF-8".into());
        client.open(&locale)
    }

    fn handle_open(&mut self, client: &mut C, input_method_id: u16) -> Result<(), ClientError> {
        self.im_id = input_method_id;
        self.opened = true;

        client.get_im_values(input_method_id, &[AttributeName::QueryInputStyle])
    }

    fn handle_get_im_values(
        &mut self,
        client: &mut C,
        input_method_id: u16,
        attributes: AHashMap<AttributeName, Vec<u8>>,
    ) -> Result<(), ClientError> {
        self.input_style = pick_input_style(&attributes);
        self.styles_ready = true;
        if self.window != 0 && self.ic_id == 0 && !self.ic_pending {
            self.ic_pending = true;
            let ic_attributes = client
                .build_ic_attributes()
                .push(AttributeName::InputStyle, self.input_style)
                .push(AttributeName::ClientWindow, self.window)
                .push(AttributeName::FocusWindow, self.window)
                .build();
            client.create_ic(input_method_id, ic_attributes)?;
        }
        Ok(())
    }

    fn handle_create_ic(
        &mut self,
        client: &mut C,
        input_method_id: u16,
        input_context_id: u16,
    ) -> Result<(), ClientError> {
        self.connected = true;
        self.ic_pending = false;
        self.ic_id = input_context_id;
        self.needs_spot_refresh = true;
        client.set_focus(input_method_id, input_context_id)?;
        Ok(())
    }

    fn handle_commit(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        text: &str,
    ) -> Result<(), ClientError> {
        self.clear_preedit();
        self.push_callback(XimCallbackEvent::XimCommitEvent(
            self.window,
            text.to_string(),
        ));
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        _flag: xim::ForwardEventFlag,
        xev: C::XEvent,
    ) -> Result<(), ClientError> {
        match xev.response_type {
            x11rb::protocol::xproto::KEY_PRESS_EVENT => {
                self.push_callback(XimCallbackEvent::XimXEvent(Event::KeyPress(xev)));
            }
            x11rb::protocol::xproto::KEY_RELEASE_EVENT => {
                self.push_callback(XimCallbackEvent::XimXEvent(Event::KeyRelease(xev)));
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_close(&mut self, client: &mut C, _input_method_id: u16) -> Result<(), ClientError> {
        client.disconnect()
    }

    fn handle_preedit_start(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
    ) -> Result<(), ClientError> {
        Ok(())
    }

    fn handle_preedit_draw(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        caret: i32,
        chg_first: i32,
        chg_len: i32,
        status: xim::PreeditDrawStatus,
        preedit_string: &str,
        _feedbacks: Vec<xim::Feedback>,
    ) -> Result<(), ClientError> {
        let text = apply_preedit_draw(
            &mut self.preedit,
            chg_first,
            chg_len,
            status,
            preedit_string,
        );
        self.emit_preedit(text, caret);
        Ok(())
    }

    fn handle_preedit_caret(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        position: &mut i32,
        _direction: xim::CaretDirection,
        _style: xim::CaretStyle,
    ) -> Result<(), ClientError> {
        let text: String = self.preedit.iter().collect();
        self.emit_preedit(text, *position);
        Ok(())
    }

    fn handle_preedit_done(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
    ) -> Result<(), ClientError> {
        self.clear_preedit();
        self.emit_preedit(String::new(), 0);
        Ok(())
    }

    fn handle_reset_ic(
        &mut self,
        _client: &mut C,
        _input_method_id: u16,
        _input_context_id: u16,
        _preedit_text: &str,
    ) -> Result<(), ClientError> {
        self.clear_preedit();
        self.clear_applied();
        self.emit_preedit(String::new(), 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_preedit_draw, caret_utf16_range};
    use xim::PreeditDrawStatus;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn draw(preedit: &mut Vec<char>, chg_first: i32, chg_len: i32, replacement: &str) -> String {
        apply_preedit_draw(
            preedit,
            chg_first,
            chg_len,
            PreeditDrawStatus::empty(),
            replacement,
        )
    }

    #[test]
    fn preedit_draw_appends_characters() {
        let mut preedit = Vec::new();
        assert_eq!(draw(&mut preedit, 0, 0, "h"), "h");
        assert_eq!(draw(&mut preedit, 1, 0, "o"), "ho");
        assert_eq!(draw(&mut preedit, 2, 0, "a"), "hoa");
    }

    #[test]
    fn preedit_draw_replaces_full_string_like_fcitx5() {
        let mut preedit = Vec::new();
        assert_eq!(draw(&mut preedit, 0, 0, "hoa"), "hoa");
        assert_eq!(draw(&mut preedit, 0, 3, "hóa"), "hóa");
    }

    #[test]
    fn preedit_draw_replaces_a_mid_string_vowel() {
        let mut preedit = Vec::new();
        assert_eq!(draw(&mut preedit, 0, 0, "hoa"), "hoa");
        assert_eq!(draw(&mut preedit, 2, 1, "á"), "hoá");
    }

    #[test]
    fn preedit_draw_no_string_deletes_the_range() {
        let mut preedit = chars("hoa");
        let text = apply_preedit_draw(&mut preedit, 0, 3, PreeditDrawStatus::NO_STRING, "ignored");
        assert_eq!(text, "");
        assert!(preedit.is_empty());
    }

    #[test]
    fn preedit_draw_negative_chg_len_deletes_through_the_end() {
        let mut preedit = chars("hoas");
        assert_eq!(draw(&mut preedit, 2, -1, "á"), "hoá");
    }

    #[test]
    fn preedit_draw_clamps_an_out_of_range_change() {
        let mut preedit = chars("a");
        assert_eq!(draw(&mut preedit, 8, 2, "x"), "ax");
    }

    #[test]
    fn caret_utf16_range_uses_utf16_units() {
        assert_eq!(caret_utf16_range("hóa", 3), Some(3..3));
        assert_eq!(caret_utf16_range("", 0), None);
        assert_eq!(caret_utf16_range("á", -1), Some(1..1));
    }

    #[test]
    fn skip_apply_requires_same_text_and_caret() {
        let mut handler = super::XimHandler::new();
        handler.remember_applied("hoa", Some(2..2));
        assert!(handler.should_skip_apply("hoa", Some(&(2..2))));
        assert!(!handler.should_skip_apply("hoa", Some(&(3..3))));
        assert!(!handler.should_skip_apply("hoas", Some(&(2..2))));
    }
}
