/// Named icon enum + SVG path data ported from Mezon's `libs/ui/src/lib/Icons/icons.tsx`.
///
/// Each icon is rendered via GPUI's `svg()` element with `path()` set to the SVG source.
/// Icons are embedded as const strings — no runtime asset loading.
use gpui::{Rgba, prelude::*, svg};

use crate::theme::Theme;

// ─── Icon names ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    // Channel types
    Hashtag,
    HashtagLocked,
    Speaker,
    Thread,
    // Navigation / arrows
    ArrowRight,
    ArrowDown,
    ArrowLeft,
    ChevronDown,
    // Common actions
    Close,
    Search,
    Add,
    Check,
    PenEdit,
    Delete,
    Download,
    Copy,
    Link,
    Pin,
    Reply,
    // Users / members
    UserIcon,
    MemberList,
    // Media / voice / input
    Attachment,
    Emoji,
    Mic,
    Deafen,
    Video,
    // App chrome
    Settings,
    Bell,
    // Thread / misc
    React,
}

// ─── SVG source strings ───────────────────────────────────────────────────────

impl IconName {
    /// Returns a complete inline SVG string for this icon.
    pub fn svg_str(self) -> &'static str {
        match self {
            IconName::Hashtag => SVG_HASHTAG,
            IconName::HashtagLocked => SVG_HASHTAG_LOCKED,
            IconName::Speaker => SVG_SPEAKER,
            IconName::Thread => SVG_THREAD,
            IconName::ArrowRight => SVG_ARROW_RIGHT,
            IconName::ArrowDown => SVG_ARROW_DOWN,
            IconName::ArrowLeft => SVG_ARROW_LEFT,
            IconName::ChevronDown => SVG_CHEVRON_DOWN,
            IconName::Close => SVG_CLOSE,
            IconName::Search => SVG_SEARCH,
            IconName::Add => SVG_ADD,
            IconName::Check => SVG_CHECK,
            IconName::PenEdit => SVG_PEN_EDIT,
            IconName::Delete => SVG_DELETE,
            IconName::Download => SVG_DOWNLOAD,
            IconName::Copy => SVG_COPY,
            IconName::Link => SVG_LINK,
            IconName::Pin => SVG_PIN,
            IconName::Reply => SVG_REPLY,
            IconName::UserIcon => SVG_USER_ICON,
            IconName::MemberList => SVG_MEMBER_LIST,
            IconName::Attachment => SVG_ATTACHMENT,
            IconName::Emoji => SVG_EMOJI,
            IconName::Mic => SVG_MIC,
            IconName::Deafen => SVG_DEAFEN,
            IconName::Video => SVG_VIDEO,
            IconName::Settings => SVG_SETTINGS,
            IconName::Bell => SVG_BELL,
            IconName::React => SVG_REACT,
        }
    }
}

// ─── Icon builder ─────────────────────────────────────────────────────────────

pub struct Icon {
    name: IconName,
    size: f32,
    color: Option<Rgba>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: 20.0,
            color: None,
        }
    }

    pub fn size(mut self, px: f32) -> Self {
        self.size = px;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn muted(self, theme: &Theme) -> Self {
        self.color(theme.text_muted)
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let color = self.color.unwrap_or(theme.text_secondary);
        let sz = gpui::px(self.size);
        svg().path(self.name.svg_str()).size(sz).text_color(color)
    }
}

// ─── SVG path data ────────────────────────────────────────────────────────────

const SVG_HASHTAG: &str = r#"<svg viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M17.4393 9.4715L17.6833 8.6585H11.7483C11.5857 8.6585 11.4231 8.49589 11.4231 8.33329V7.76419H9.14667L10.2036 3.86175H8.41496L7.35805 7.76419H3.37431L2.8865 9.4715H6.87024L6.05724 12.3983H2.1548L1.66699 14.1056H5.65073L4.51252 18.3333H6.30114L7.43935 14.1056H10.5288L9.39057 18.3333H11.1792L12.3174 14.1056H16.3011L16.7889 12.3983H12.8052L13.6182 9.4715H17.4393ZM10.9353 12.3983H7.84585L8.65886 9.4715H11.7483L10.9353 12.3983Z"/></svg>"#;

const SVG_HASHTAG_LOCKED: &str = r#"<svg viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M17.4393 9.4715L17.6833 8.6585H11.7483C11.5857 8.6585 11.4231 8.49589 11.4231 8.33329V7.76419H9.14667L10.2036 3.86175H8.41496L7.35805 7.76419H3.37431L2.8865 9.4715H6.87024L6.05724 12.3983H2.1548L1.66699 14.1056H5.65073L4.51252 18.3333H6.30114L7.43935 14.1056H10.5288L9.39057 18.3333H11.1792L12.3174 14.1056H16.3011L16.7889 12.3983H12.8052L13.6182 9.4715H17.4393ZM10.9353 12.3983H7.84585L8.65886 9.4715H11.7483L10.9353 12.3983Z"/><path fill="currentColor" d="M13.2117 3.45524V3.13004C13.2117 2.31703 13.8621 1.66663 14.6751 1.66663C15.4881 1.66663 16.1385 2.31703 16.1385 3.13004V3.45524H16.4637C16.7889 3.45524 17.0328 3.69915 17.0328 4.02435V6.95118C17.0328 7.27638 16.7889 7.52028 16.4637 7.52028H12.8865C12.5613 7.52028 12.3174 7.27638 12.3174 6.95118V4.02435C12.3174 3.69915 12.5613 3.45524 12.8865 3.45524H13.2117ZM14.6751 4.83736C14.4312 4.83736 14.2686 4.99996 14.2686 5.24386V5.81297C14.2686 6.05687 14.4312 6.21947 14.6751 6.21947C14.919 6.21947 15.0816 6.05687 15.0816 5.81297V5.24386C15.0816 4.99996 14.919 4.83736 14.6751 4.83736Z"/></svg>"#;

const SVG_SPEAKER: &str = r#"<svg viewBox="0 0 18 17" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M10.1977 0.750516C10.4846 0.889971 10.6667 1.18099 10.6667 1.50001V16.5C10.6667 16.819 10.4846 17.1101 10.1977 17.2495C9.91077 17.389 9.56941 17.3524 9.31856 17.1553L4.24217 13.1667H1.50008C1.03984 13.1667 0.666748 12.7936 0.666748 12.3333V5.66668C0.666748 5.20644 1.03984 4.83334 1.50008 4.83334H4.24217L9.31856 0.844747C9.56941 0.64765 9.91077 0.611061 10.1977 0.750516Z"/><path fill="currentColor" d="M14.5893 3.41075C14.2639 3.08531 13.7363 3.08531 13.4108 3.41075C13.0854 3.73619 13.0854 4.26382 13.4108 4.58926L13.6968 4.87521C15.9748 7.15327 15.9748 10.8467 13.6968 13.1248L13.4108 13.4107C13.0854 13.7362 13.0854 14.2638 13.4108 14.5893C13.7363 14.9147 14.2639 14.9147 14.5893 14.5893L14.8753 14.3033C17.8042 11.3744 17.8042 6.62563 14.8753 3.6967L14.5893 3.41075Z"/><path fill="currentColor" d="M12.9227 5.91075C12.5972 5.58531 12.0696 5.58531 11.7442 5.91075C11.4187 6.23619 11.4187 6.76382 11.7442 7.08926L11.8871 7.23224C12.8634 8.20855 12.8634 9.79146 11.8871 10.7678L11.7442 10.9107C11.4187 11.2362 11.4187 11.7638 11.7442 12.0893C12.0696 12.4147 12.5972 12.4147 12.9227 12.0893L13.0656 11.9463C14.6928 10.3191 14.6928 7.68091 13.0656 6.05373L12.9227 5.91075Z"/></svg>"#;

const SVG_THREAD: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 2.81a1 1 0 0 1 0-1.41l.36-.36a1 1 0 0 1 1.41 0l9.2 9.2a1 1 0 0 1 0 1.4l-.7.7a1 1 0 0 1-1.3.13l-9.54-6.72a1 1 0 0 1-.08-1.58l1-1L12 2.8ZM12 21.2a1 1 0 0 1 0 1.41l-.35.35a1 1 0 0 1-1.41 0l-9.2-9.19a1 1 0 0 1 0-1.41l.7-.7a1 1 0 0 1 1.3-.12l9.54 6.72a1 1 0 0 1 .07 1.58l-1 1 .35.36ZM15.66 16.8a1 1 0 0 1-1.38.28l-8.49-5.66A1 1 0 1 1 6.9 9.76l8.49 5.65a1 1 0 0 1 .27 1.39ZM17.1 14.25a1 1 0 1 0 1.11-1.66L9.73 6.93a1 1 0 0 0-1.11 1.66l8.49 5.66Z"/></svg>"#;

const SVG_ARROW_RIGHT: &str = r#"<svg viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M5.91083 3.41075C6.23626 3.08531 6.7639 3.08531 7.08934 3.41075L12.0893 8.41075C12.4148 8.73619 12.4148 9.26382 12.0893 9.58926L7.08934 14.5893C6.7639 14.9147 6.23626 14.9147 5.91083 14.5893C5.58539 14.2638 5.58539 13.7362 5.91083 13.4107L10.3215 9.00001L5.91083 4.58926C5.58539 4.26382 5.58539 3.73619 5.91083 3.41075Z"/></svg>"#;

const SVG_ARROW_DOWN: &str = r#"<svg viewBox="0 0 20 17" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M14.5892 5.91075C14.9147 6.23619 14.9147 6.76382 14.5892 7.08926L9.58924 12.0893C9.2638 12.4147 8.73616 12.4147 8.41072 12.0893L3.41072 7.08926C3.08529 6.76382 3.08529 6.23619 3.41072 5.91075C3.73616 5.58531 4.2638 5.58531 4.58924 5.91075L8.99998 10.3215L13.4107 5.91075C13.7362 5.58531 14.2638 5.58531 14.5892 5.91075Z"/></svg>"#;

const SVG_ARROW_LEFT: &str = r#"<svg viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M14.0893 3.41075C14.4147 3.73619 14.4147 4.26382 14.0893 4.58926L9.67854 9.00001L14.0893 13.4107C14.4147 13.7362 14.4147 14.2638 14.0893 14.5893C13.7638 14.9147 13.2362 14.9147 12.9107 14.5893L7.91072 9.58926C7.58529 9.26382 7.58529 8.73619 7.91072 8.41075L12.9107 3.41075C13.2362 3.08531 13.7638 3.08531 14.0893 3.41075Z"/></svg>"#;

const SVG_CHEVRON_DOWN: &str = r#"<svg viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M3.293 5.293a1 1 0 0 1 1.414 0L8 8.586l3.293-3.293a1 1 0 1 1 1.414 1.414l-4 4a1 1 0 0 1-1.414 0l-4-4a1 1 0 0 1 0-1.414Z"/></svg>"#;

const SVG_CLOSE: &str = r#"<svg viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M2.29289 16.2929C1.90237 16.6834 1.90237 17.3166 2.29289 17.7071C2.68342 18.0976 3.31658 18.0976 3.70711 17.7071L10 11.4142L16.2929 17.7071C16.6834 18.0976 17.3166 18.0976 17.7071 17.7071C18.0976 17.3166 18.0976 16.6834 17.7071 16.2929L11.4142 10L17.7071 3.70711C18.0976 3.31658 18.0976 2.68342 17.7071 2.29289C17.3166 1.90237 16.6834 1.90237 16.2929 2.29289L10 8.58579L3.70711 2.2929C3.31658 1.90237 2.68342 1.90237 2.29289 2.2929C1.90237 2.68342 1.90237 3.31658 2.29289 3.70711L8.58579 10L2.29289 16.2929Z"/></svg>"#;

const SVG_SEARCH: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" d="M15.62 17.03a9 9 0 1 1 1.41-1.41l4.68 4.67a1 1 0 0 1-1.42 1.42l-4.67-4.68ZM17 10a7 7 0 1 1-14 0 7 7 0 0 1 14 0Z"/></svg>"#;

const SVG_ADD: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M13 5a1 1 0 1 0-2 0v6H5a1 1 0 1 0 0 2h6v6a1 1 0 1 0 2 0v-6h6a1 1 0 1 0 0-2h-6V5Z"/></svg>"#;

const SVG_CHECK: &str = r#"<svg viewBox="0 0 16 15.2" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M7.4 11.17L4 8.62l1-1.36 2 1.53L10.64 4 12 5z"/></svg>"#;

const SVG_PEN_EDIT: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="m13.96 5.46 4.58 4.58a1 1 0 0 0 1.42 0l1.38-1.38a2 2 0 0 0 0-2.82l-3.18-3.18a2 2 0 0 0-2.82 0l-1.38 1.38a1 1 0 0 0 0 1.42ZM2.11 20.16l.73-4.22a3 3 0 0 1 .83-1.61l7.87-7.87a1 1 0 0 1 1.42 0l4.58 4.58a1 1 0 0 1 0 1.42l-7.87 7.87a3 3 0 0 1-1.6.83l-4.23.73a1.5 1.5 0 0 1-1.73-1.73Z"/></svg>"#;

const SVG_DELETE: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M14.25 1C14.25 0.585786 13.9142 0.25 13.5 0.25H10.5C10.0858 0.25 9.75 0.585786 9.75 1V2.25H5C4.58579 2.25 4.25 2.58579 4.25 3C4.25 3.41421 4.58579 3.75 5 3.75H19C19.4142 3.75 19.75 3.41421 19.75 3C19.75 2.58579 19.4142 2.25 19 2.25H14.25V1ZM6 6.25C5.58579 6.25 5.25 6.58579 5.25 7C5.25 7.41421 5.58579 7.75 6 7.75V19C6 20.6569 7.34315 22 9 22H15C16.6569 22 18 20.6569 18 19V7.75C18.4142 7.75 18.75 7.41421 18.75 7C18.75 6.58579 18.4142 6.25 18 6.25H6ZM9.75 9C9.75 8.58579 9.41421 8.25 9 8.25C8.58579 8.25 8.25 8.58579 8.25 9V18C8.25 18.4142 8.58579 18.75 9 18.75C9.41421 18.75 9.75 18.4142 9.75 18V9ZM15 8.25C14.5858 8.25 14.25 8.58579 14.25 9V18C14.25 18.4142 14.5858 18.75 15 18.75C15.4142 18.75 15.75 18.4142 15.75 18V9C15.75 8.58579 15.4142 8.25 15 8.25Z"/></svg>"#;

const SVG_DOWNLOAD: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 2a1 1 0 0 1 1 1v10.59l3.3-3.3a1 1 0 1 1 1.4 1.42l-5 5a1 1 0 0 1-1.4 0l-5-5a1 1 0 1 1 1.4-1.42l3.3 3.3V3a1 1 0 0 1 1-1ZM3 20a1 1 0 1 0 0 2h18a1 1 0 1 0 0-2H3Z"/></svg>"#;

const SVG_COPY: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M16 1H4C2.9 1 2 1.9 2 3v14h2V3h12V1zm3 4H8C6.9 5 6 5.9 6 7v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z"/></svg>"#;

const SVG_LINK: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M13.06 8.11l1.415 1.415a7 7 0 0 1 0 9.9l-.354.353a7 7 0 0 1-9.9-9.9l1.415 1.415a5 5 0 1 0 7.071 7.071l.354-.354a5 5 0 0 0 0-7.07l-1.415-1.415 1.415-1.414zm6.718 6.011l-1.414-1.414a5 5 0 1 0-7.071-7.071l-.354.354a5 5 0 0 0 0 7.07l1.415 1.415-1.415 1.414-1.414-1.414a7 7 0 0 1 0-9.9l.354-.353a7 7 0 0 1 9.9 9.9z"/></svg>"#;

const SVG_PIN: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M22 12l-9.899-9.899-1.415 1.413 1.415 1.415-4.95 4.949v.002L5.736 8.465 4.322 9.88l4.243 4.242-5.657 5.656 1.414 1.414 5.657-5.656 4.243 4.242 1.414-1.414-1.414-1.414L19.171 12h.001l1.414 1.414L22 12z"/></svg>"#;

const SVG_REPLY: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" d="M5 4C5.55228 4 6 4.44772 6 5V12C6 13.1046 6.89543 14 8 14H17.5858L15.2929 11.7071C14.9024 11.3166 14.9024 10.6834 15.2929 10.2929C15.6834 9.90237 16.3166 9.90237 16.7071 10.2929L20.7071 14.2929C20.8946 14.4804 21 14.7348 21 15C21 15.2652 20.8946 15.5196 20.7071 15.7071L16.7071 19.7071C16.3166 20.0976 15.6834 20.0976 15.2929 19.7071C14.9024 19.3166 14.9024 18.6834 15.2929 18.2929L17.5858 16H8C5.79086 16 4 14.2091 4 12V5C4 4.44772 4.44772 4 5 4Z"/></svg>"#;

const SVG_USER_ICON: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10Zm0 2c-5.33 0-8 2.67-8 4v2h16v-2c0-1.33-2.67-4-8-4Z"/></svg>"#;

const SVG_MEMBER_LIST: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M14.5 8a3 3 0 1 0-2.7-4.3c-.2.4.06.86.44 1.12a5 5 0 0 1 2.14 3.08c.01.06.06.1.12.1ZM18.44 17.27c.15.43.54.73 1 .73h1.06c.83 0 1.5-.67 1.5-1.5a7.5 7.5 0 0 0-6.5-7.43c-.55-.08-.99.38-1.1.92-.06.3-.15.6-.26.87-.23.58-.05 1.3.47 1.63a9.53 9.53 0 0 1 3.83 4.78ZM12.5 9a3 3 0 1 1-6 0 3 3 0 0 1 6 0ZM2 20.5a7.5 7.5 0 0 1 15 0c0 .83-.67 1.5-1.5 1.5a.2.2 0 0 1-.2-.16c-.2-.96-.56-1.87-.88-2.54-.1-.23-.42-.15-.42.1v2.1a.5.5 0 0 1-.5.5h-8a.5.5 0 0 1-.5-.5v-2.1c0-.25-.31-.33-.42-.1-.32.67-.67 1.58-.88 2.54a.2.2 0 0 1-.2.16A1.5 1.5 0 0 1 2 20.5Z"/></svg>"#;

const SVG_ATTACHMENT: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 2a7 7 0 0 1 7 7v8a5 5 0 0 1-10 0V8a3 3 0 0 1 6 0v8a1 1 0 1 1-2 0V8a1 1 0 1 0-2 0v9a3 3 0 0 0 6 0V9a5 5 0 0 0-10 0v9a7 7 0 0 0 14 0V9a1 1 0 1 1 2 0v9a9 9 0 0 1-18 0V9a7 7 0 0 1 7-7Z"/></svg>"#;

const SVG_EMOJI: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 23a11 11 0 1 0 0-22 11 11 0 0 0 0 22ZM6.5 13a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Zm11 0a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Zm-9.8 1.17a1 1 0 0 1 1.38-.33 3.5 3.5 0 0 0 3.84 0 1 1 0 0 1 1.05 1.7 5.5 5.5 0 0 1-6.6 0 1 1 0 0 1-.33-1.37Z"/></svg>"#;

const SVG_MIC: &str = r#"<svg viewBox="0 0 20 21" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M3.04222 2.36714C2.71482 2.08598 2.2209 2.1005 1.9107 2.4107C1.58527 2.73614 1.58527 3.26378 1.9107 3.58921L7.08329 8.7618V10.5C7.08329 12.1108 8.38913 13.4166 9.99996 13.4166C10.5023 13.4166 10.9749 13.2897 11.3875 13.0661L12.2978 13.9763C11.6391 14.4126 10.8492 14.6666 9.99996 14.6666C7.69877 14.6666 5.83329 12.8011 5.83329 10.5C5.83329 10.0397 5.4602 9.66663 4.99996 9.66663C4.53972 9.66663 4.16663 10.0397 4.16663 10.5C4.16663 13.4394 6.34076 15.871 9.16867 16.2745V17.1666H7.49996C7.03972 17.1666 6.66663 17.5397 6.66663 18C6.66663 18.4602 7.03972 18.8333 7.49996 18.8333H12.5C12.9602 18.8333 13.3333 18.4602 13.3333 18C13.3333 17.5397 12.9602 17.1666 12.5 17.1666H10.8333V16.2745C11.8203 16.1334 12.7294 15.7442 13.4934 15.1719L16.9107 18.5892C17.2361 18.9147 17.7638 18.9147 18.0892 18.5892C18.3991 18.2793 18.4139 17.786 18.1335 17.4586L3.08583 2.41418C3.05604 2.38323 3.04222 2.36714 3.04222 2.36714ZM12.9166 9.88809V5.08329C12.9166 3.47246 11.6108 2.16663 9.99996 2.16663C8.6996 2.16663 7.598 3.0176 7.22166 4.19303L12.9166 9.88809Z"/></svg>"#;

const SVG_DEAFEN: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 3a9 9 0 0 0-9 9v5a3 3 0 0 0 3 3h2a1 1 0 0 0 1-1v-6a1 1 0 0 0-1-1H6.1A7 7 0 0 1 19 12h-2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2a3 3 0 0 0 3-3v-5a9 9 0 0 0-9-9Z"/></svg>"#;

const SVG_VIDEO: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M2 7a3 3 0 0 1 3-3h10a3 3 0 0 1 3 3v2.27l3.33-2.22A1 1 0 0 1 23 8v8a1 1 0 0 1-1.54.84L18 14.73V17a3 3 0 0 1-3 3H5a3 3 0 0 1-3-3V7Z"/></svg>"#;

const SVG_SETTINGS: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M10.56 1.1c-.46.05-.7.53-.64.98.18 1.16-.19 2.2-.98 2.53-.8.33-1.79-.15-2.49-1.1-.27-.36-.78-.52-1.14-.24-.77.59-1.45 1.27-2.04 2.04-.28.36-.12.87.24 1.14.96.7 1.43 1.7 1.1 2.49-.33.8-1.37 1.16-2.53.98-.45-.07-.93.18-.99.64a11.1 11.1 0 0 0 0 2.88c.06.46.54.7.99.64 1.16-.18 2.2.19 2.53.98.33.8-.14 1.79-1.1 2.49-.36.27-.52.78-.24 1.14.59.77 1.27 1.45 2.04 2.04.36.28.87.12 1.14-.24.7-.95 1.7-1.43 2.49-1.1.8.33 1.16 1.37.98 2.53-.07.45.18.93.64.99a11.1 11.1 0 0 0 2.88 0c.46-.06.7-.54.64-.99-.18-1.16.19-2.2.98-2.53.8-.33 1.79.14 2.49 1.1.27.36.78.52 1.14.24.77-.59 1.45-1.27 2.04-2.04.28-.36.12-.87-.24-1.14-.96-.7-1.43-1.7-1.1-2.49.33-.8 1.37-1.16 2.53-.98.45.07.93-.18.99-.64a11.1 11.1 0 0 0 0-2.88c-.06-.46-.54-.7-.99-.64-1.16.18-2.2-.19-2.53-.98-.33-.8.14-1.79 1.1-2.49.36-.27.52-.78.24-1.14a11.07 11.07 0 0 0-2.04-2.04c-.36-.28-.87-.12-1.14.24-.7.96-1.7 1.43-2.49 1.1-.8-.33-1.16-1.37-.98-2.53.07-.45-.18-.93-.64-.99a11.1 11.1 0 0 0-2.88 0ZM16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z"/></svg>"#;

const SVG_BELL: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M18 9v5a3 3 0 003 3v1H3v-1a3 3 0 003-3V9a6 6 0 1112 0zm-6 12c-1.476 0-2.752-.81-3.445-2h6.89c-.693 1.19-1.97 2-3.445 2z"/></svg>"#;

const SVG_REACT: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" d="M12 23a11 11 0 1 0 0-22 11 11 0 0 0 0 22Zm0-2a9 9 0 1 1 0-18 9 9 0 0 1 0 18ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Zm8 0a1 1 0 1 1-2 0 1 1 0 0 1 2 0Zm-7.07 4a1 1 0 0 1 1.4-1.42 3 3 0 0 0 3.34 0A1 1 0 0 1 16.07 15a5 5 0 0 1-6.14 0Z"/></svg>"#;
