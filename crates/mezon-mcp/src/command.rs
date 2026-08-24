use serde_json::Value;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy)]
pub enum CaptureTarget {
    Window,
    Chat,
}

#[derive(Debug)]
pub enum McpCommand {
    GetContext {
        reply: oneshot::Sender<Value>,
    },
    Navigate {
        path: String,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Logout {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Refresh {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Quit {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    ShowWindow {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Capture {
        target: CaptureTarget,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    GoBack {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    GoForward {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    GetSettings {
        reply: oneshot::Sender<Value>,
    },
    SetSetting {
        key: String,
        value: Value,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    SetCliEnabled {
        enabled: bool,
        reply: oneshot::Sender<anyhow::Result<bool>>,
    },
    JoinVoice {
        channel_id: i64,
        clan_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    LeaveVoice {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    GetRecordingState {
        reply: oneshot::Sender<Value>,
    },
    StartRecording {
        path: Option<String>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    StopRecording {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    GetScrollState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    SetPanel {
        kind: Option<String>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    OpenImageViewer {
        message_id: i64,
        attachment_index: usize,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    OpenPdfViewer {
        message_id: i64,
        attachment_index: usize,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ScrollWheel {
        delta_y: f32,
        ticks: u32,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ScrollMessages {
        to_top: bool,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ComposerType {
        text: String,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ComposerState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ComposerPick {
        index: usize,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ComposerSubmit {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    OpenTopic {
        message_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CloseTopic {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    TopicState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    TopicType {
        text: String,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    TopicDropPaths {
        paths: Vec<String>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    TopicSubmit {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    TopicScrollWheel {
        delta_y: f32,
        ticks: u32,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    EditBegin {
        message_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    EditType {
        text: String,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    EditPick {
        index: usize,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    EditState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    EditSave {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ComposerPanelSend {
        kind: String,
        url: String,
        filename: String,
        width: i32,
        height: i32,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ComposerDropPaths {
        paths: Vec<String>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    SendBuzz {
        text: String,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    SendAttachment {
        paths: Vec<String>,
        content: String,
        anonymous: bool,
        reply_to: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ListEmojis {
        clan_id: Option<String>,
        query: Option<String>,
        limit: usize,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    LoadMoreMessages {
        older: bool,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ListLoadedMessages {
        limit: usize,
        topic: bool,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ReplyBegin {
        message_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    JumpToMessage {
        message_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    JumpToPresent {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    SetUserStatus {
        status: String,
        minutes: i32,
        until_turn_on: bool,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    GetUserStatus {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    GetMemberList {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CloseModal {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ListBannedUsers {
        clan_id: i64,
        channel_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    MemberMenuState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    MemberMenuOpen {
        user_id: i64,
        x: f32,
        y: f32,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    MemberMenuClose {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    MemberMenuPick {
        index: usize,
        value: Option<i32>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ClanMenuState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ClanMenuOpen {
        clan_id: i64,
        x: f32,
        y: f32,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ClanMenuClose {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ClanMenuPick {
        index: usize,
        value: Option<i32>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ListCategories {
        clan_id: i64,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CreateCategory {
        clan_id: i64,
        name: String,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ChannelMenuState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ChannelMenuOpen {
        clan_id: i64,
        channel_id: i64,
        x: f32,
        y: f32,
        in_favorites: bool,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ChannelMenuClose {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    ChannelMenuPick {
        index: usize,
        value: Option<i32>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CategoryMenuState {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CategoryMenuOpen {
        clan_id: i64,
        category_id: String,
        x: f32,
        y: f32,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CategoryMenuClose {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CategoryMenuPick {
        index: usize,
        value: Option<i32>,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    CreateClan {
        name: String,
        logo: String,
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
    OpenCreateClanModal {
        reply: oneshot::Sender<anyhow::Result<Value>>,
    },
}
