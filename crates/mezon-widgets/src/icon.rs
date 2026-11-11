use gpui::{
    App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, MouseMoveEvent,
    Pixels, StyleRefinement, Svg, Transformation, Window, point, prelude::*, px, size, svg,
};

use crate::icon_motion::{ColorPlayback, MotionPlayback, MotionSpec};

#[derive(Default)]
struct IconPlayback {
    motion: MotionPlayback,
    color: ColorPlayback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    AddServe,
    AdminHomeIcon,
    AdminSettingIcon,
    Announcement,
    Anonymous,
    AppDirectoryFooterRobot,
    AppStore,
    AppStoreBadge,
    AppVersion,
    Apple,
    ArrowDown,
    ArrowLeft,
    ArrowLeftCircle,
    ArrowLeftCircleActive,
    ArrowRight,
    ArrowUp,
    ArrowUpRight,
    AspectRatio,
    Attach,
    AudioLevelCircle,
    AuditLog,
    Backspace,
    Ban,
    Bell,
    BellOff,
    Bicycle,
    Block,
    Bolt,
    Bot,
    Breakout,
    Buzz,
    CDNIcon,
    Calendar,
    CalendarIcon,
    Call,
    CallCancel,
    CallEnd,
    CallIncoming,
    CallMissed,
    CallOutgoing,
    Camera,
    CameraOff,
    CameraSwitch,
    Canvas,
    CaretDown,
    CaretLeft,
    CaretRight,
    CaretUp,
    Channel,
    ChannelAdd,
    ChannelApp,
    ChannelAppPrivate,
    ChannelBrowse,
    ChannelPrivate,
    ChannelWarning,
    Chat,
    Check,
    CheckCircle,
    CheckMarkFilter,
    CheckedIcon,
    Chevron,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronUpDown,
    Circle,
    ClanBadge,
    Clans,
    Clock,
    Close,
    CloseCircle,
    Coin,
    Community,
    Contact,
    Copy,
    Crop,
    Cursor,
    DeviceDesktop,
    DeviceMobile,
    Devices,
    Discover,
    Dot,
    DotIcon,
    Download,
    Draw,
    Edit,
    Emoji,
    EmptyUnreadStyle,
    Eraser,
    Error,
    Event,
    ExternalLink,
    Eye,
    EyeOff,
    File,
    FileAndFolder,
    FileArchive,
    FileAudio,
    FileCode,
    FileImage,
    FilePdf,
    FileSheet,
    FileText,
    Filter,
    Flag,
    Flashlight,
    FlashlightOff,
    Folder,
    FolderAdd,
    Food,
    Forum,
    Forward,
    Forward15,
    ForwardAll,
    Friends,
    Fullscreen,
    FullscreenExit,
    Gamepad,
    Gif,
    Gift,
    GlobalIcon,
    Globe,
    GooglePlay,
    GooglePlayBadge,
    Grid,
    GridDense,
    Grip,
    Guide,
    HandRaise,
    HeadPhoneICon,
    Heading1,
    Heading2,
    Heading3,
    Headphones,
    Heart,
    Help,
    Highlighter,
    History,
    Home,
    HomepageMenu,
    IConShareEventLocation,
    IconClock,
    IconReplyMessDeleted,
    IdCard,
    Image,
    ImageAdd,
    Inbox,
    Info,
    InfoIcon,
    Join,
    Keyboard,
    Lamp,
    Language,
    Laser,
    LayoutFocus,
    Leaf,
    Leave,
    LeftArrowIcon,
    Link,
    Linux,
    ListBullet,
    ListCheck,
    ListNumber,
    Live,
    LoLGame,
    Location,
    Lock,
    Locked,
    Logout,
    LongCorner,
    MacAppStoreDesktop,
    MacAppleIntel,
    MacAppleSilicon,
    MacOSCloseIcon,
    MacOSMaximizeIcon,
    MacOSMinimizeIcon,
    Mail,
    MarkRead,
    MarkUnread,
    Members,
    Mention,
    Menu,
    MenuBarIcon,
    MenuClose,
    MessageAdd,
    MessageIcon,
    Messages,
    Mic,
    MicDisable,
    MicIcon,
    MicOff,
    Microphone,
    Microsoft,
    MicrosoftDropdown,
    MicrosoftWinPortable,
    MinusCircle,
    Moon,
    More,
    MoreVertical,
    Move,
    Music,
    NoiseSuppression,
    NoiseSuppressionOff,
    NotJoinedSFU,
    OAuth2Setting,
    OpenMenu,
    Owner,
    OwnerTransfer,
    Palette,
    Paragraph,
    Paste,
    Pause,
    PhoneOff,
    Pin,
    PinOff,
    Play,
    Plus,
    PlusCircle,
    Poll,
    PrivateChatIcon,
    Ptt,
    Qr,
    QrScan,
    QuickMenusIcon,
    Quote,
    Record,
    Redo,
    Refresh,
    Reply,
    ReplyCorner,
    Rewind15,
    Role,
    RotateIcon,
    RotateLeft,
    RotateRight,
    Rules,
    School,
    ScreenShare,
    ScreenShareOff,
    Search,
    Send,
    SendMoney,
    Settings,
    ShadowBotIcon,
    Share,
    Shield,
    Shop,
    ShortCorner,
    Sidebar,
    Slash,
    Sliders,
    SortByBigSizeBtn,
    SortBySmallSizeBtn,
    Soundboard,
    Sparkles,
    Speaker,
    SpeakerLock,
    SpeakerLow,
    SpeakerOff,
    Spinner,
    Spotify,
    Square,
    SquareCheck,
    Star,
    StartCall,
    StatusDnd,
    StatusIdle,
    StatusOffline,
    StatusOnline,
    Sticker,
    Stop,
    StopCall,
    StraightLineIcon,
    SvgQualityExcellentIcon,
    SvgQualityGoodIcon,
    SvgQualityPoorIcon,
    SvgQualityUnknownIcon,
    Swap,
    Sync,
    TVIcon,
    Target,
    Text,
    Thread,
    ThreadAdd,
    ThreadPrivate,
    ToolIcon,
    Tools,
    TopRight,
    Topic,
    Transaction,
    Trash,
    Trending,
    Tv,
    Typing,
    UnLockIcon,
    Undo,
    Unlock,
    UploadImageIcon,
    User,
    UserAvatarIcon,
    UserBlock,
    UserCheck,
    UserCircle,
    UserMinus,
    UserPending,
    UserPlus,
    UserUnblock,
    UtilitiesIcon,
    Verified,
    Video,
    VideoDisable,
    VideoIcon,
    VideoOff,
    VisualStudioCode,
    VoiceIcon,
    Wallet,
    Warning,
    Webhook,
    Wifi,
    WifiOff,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
    WindowZoom,
    Windows,
    ZoomIn,
}

impl IconName {
    pub fn path(self) -> &'static str {
        match self {
            Self::AddServe => "icons/add-serve.svg",
            Self::AdminHomeIcon => "icons/admin-home-icon.svg",
            Self::AdminSettingIcon => "icons/admin-setting-icon.svg",
            Self::Announcement => "icons/announcement.svg",
            Self::Anonymous => "icons/anonymous.svg",
            Self::AppDirectoryFooterRobot => "icons/app-directory-footer-robot.svg",
            Self::AppStore => "icons/app-store.svg",
            Self::AppStoreBadge => "icons/app-store-badge.svg",
            Self::AppVersion => "icons/app-version.svg",
            Self::Apple => "icons/apple.svg",
            Self::ArrowDown => "icons/arrow-down.svg",
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowLeftCircle => "icons/arrow-left-circle.svg",
            Self::ArrowLeftCircleActive => "icons/arrow-left-circle-active.svg",
            Self::ArrowRight => "icons/arrow-right.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::ArrowUpRight => "icons/arrow-up-right.svg",
            Self::AspectRatio => "icons/aspect-ratio.svg",
            Self::Attach => "icons/attach.svg",
            Self::AudioLevelCircle => "icons/audio-level-circle.svg",
            Self::AuditLog => "icons/audit-log.svg",
            Self::Backspace => "icons/backspace.svg",
            Self::Ban => "icons/ban.svg",
            Self::Bell => "icons/bell.svg",
            Self::BellOff => "icons/bell-off.svg",
            Self::Bicycle => "icons/bicycle.svg",
            Self::Block => "icons/block.svg",
            Self::Bolt => "icons/bolt.svg",
            Self::Bot => "icons/bot.svg",
            Self::Breakout => "icons/breakout.svg",
            Self::Buzz => "icons/buzz.svg",
            Self::CDNIcon => "icons/cdn-icon.svg",
            Self::Calendar => "icons/calendar.svg",
            Self::CalendarIcon => "icons/calendar-icon.svg",
            Self::Call => "icons/call.svg",
            Self::CallCancel => "icons/call-cancel.svg",
            Self::CallEnd => "icons/call-end.svg",
            Self::CallIncoming => "icons/call-incoming.svg",
            Self::CallMissed => "icons/call-missed.svg",
            Self::CallOutgoing => "icons/call-outgoing.svg",
            Self::Camera => "icons/camera.svg",
            Self::CameraOff => "icons/camera-off.svg",
            Self::CameraSwitch => "icons/camera-switch.svg",
            Self::Canvas => "icons/canvas.svg",
            Self::CaretDown => "icons/caret-down.svg",
            Self::CaretLeft => "icons/caret-left.svg",
            Self::CaretRight => "icons/caret-right.svg",
            Self::CaretUp => "icons/caret-up.svg",
            Self::Channel => "icons/channel.svg",
            Self::ChannelAdd => "icons/channel-add.svg",
            Self::ChannelApp => "icons/channel-app.svg",
            Self::ChannelAppPrivate => "icons/channel-app-private.svg",
            Self::ChannelBrowse => "icons/channel-browse.svg",
            Self::ChannelPrivate => "icons/channel-private.svg",
            Self::ChannelWarning => "icons/channel-warning.svg",
            Self::Chat => "icons/chat.svg",
            Self::Check => "icons/check.svg",
            Self::CheckCircle => "icons/check-circle.svg",
            Self::CheckMarkFilter => "icons/check-mark-filter.svg",
            Self::CheckedIcon => "icons/checked-icon.svg",
            Self::Chevron => "icons/chevron.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronLeft => "icons/chevron-left.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ChevronUp => "icons/chevron-up.svg",
            Self::ChevronUpDown => "icons/chevron-up-down.svg",
            Self::Circle => "icons/circle.svg",
            Self::ClanBadge => "icons/clan-badge.svg",
            Self::Clans => "icons/clans.svg",
            Self::Clock => "icons/clock.svg",
            Self::Close => "icons/close.svg",
            Self::CloseCircle => "icons/close-circle.svg",
            Self::Coin => "icons/coin.svg",
            Self::Community => "icons/community.svg",
            Self::Contact => "icons/contact.svg",
            Self::Copy => "icons/copy.svg",
            Self::Crop => "icons/crop.svg",
            Self::Cursor => "icons/cursor.svg",
            Self::DeviceDesktop => "icons/device-desktop.svg",
            Self::DeviceMobile => "icons/device-mobile.svg",
            Self::Devices => "icons/devices.svg",
            Self::Discover => "icons/discover.svg",
            Self::Dot => "icons/dot.svg",
            Self::DotIcon => "icons/dot-icon.svg",
            Self::Download => "icons/download.svg",
            Self::Draw => "icons/draw.svg",
            Self::Edit => "icons/edit.svg",
            Self::Emoji => "icons/emoji.svg",
            Self::EmptyUnreadStyle => "icons/empty-unread-style.svg",
            Self::Eraser => "icons/eraser.svg",
            Self::Error => "icons/error.svg",
            Self::Event => "icons/event.svg",
            Self::ExternalLink => "icons/external-link.svg",
            Self::Eye => "icons/eye.svg",
            Self::EyeOff => "icons/eye-off.svg",
            Self::File => "icons/file.svg",
            Self::FileAndFolder => "icons/file-and-folder.svg",
            Self::FileArchive => "icons/file-archive.svg",
            Self::FileAudio => "icons/file-audio.svg",
            Self::FileCode => "icons/file-code.svg",
            Self::FileImage => "icons/file-image.svg",
            Self::FilePdf => "icons/file-pdf.svg",
            Self::FileSheet => "icons/file-sheet.svg",
            Self::FileText => "icons/file-text.svg",
            Self::Filter => "icons/filter.svg",
            Self::Flag => "icons/flag.svg",
            Self::Flashlight => "icons/flashlight.svg",
            Self::FlashlightOff => "icons/flashlight-off.svg",
            Self::Folder => "icons/folder.svg",
            Self::FolderAdd => "icons/folder-add.svg",
            Self::Food => "icons/food.svg",
            Self::Forum => "icons/forum.svg",
            Self::Forward => "icons/forward.svg",
            Self::Forward15 => "icons/forward-15.svg",
            Self::ForwardAll => "icons/forward-all.svg",
            Self::Friends => "icons/friends.svg",
            Self::Fullscreen => "icons/fullscreen.svg",
            Self::FullscreenExit => "icons/fullscreen-exit.svg",
            Self::Gamepad => "icons/gamepad.svg",
            Self::Gif => "icons/gif.svg",
            Self::Gift => "icons/gift.svg",
            Self::GlobalIcon => "icons/global-icon.svg",
            Self::Globe => "icons/globe.svg",
            Self::GooglePlay => "icons/google-play.svg",
            Self::GooglePlayBadge => "icons/google-play-badge.svg",
            Self::Grid => "icons/grid.svg",
            Self::GridDense => "icons/grid-dense.svg",
            Self::Grip => "icons/grip.svg",
            Self::Guide => "icons/guide.svg",
            Self::HandRaise => "icons/hand-raise.svg",
            Self::HeadPhoneICon => "icons/head-phone-i-con.svg",
            Self::Heading1 => "icons/heading-1.svg",
            Self::Heading2 => "icons/heading-2.svg",
            Self::Heading3 => "icons/heading-3.svg",
            Self::Headphones => "icons/headphones.svg",
            Self::Heart => "icons/heart.svg",
            Self::Help => "icons/help.svg",
            Self::Highlighter => "icons/highlighter.svg",
            Self::History => "icons/history.svg",
            Self::Home => "icons/home.svg",
            Self::HomepageMenu => "icons/homepage-menu.svg",
            Self::IConShareEventLocation => "icons/i-con-share-event-location.svg",
            Self::IconClock => "icons/icon-clock.svg",
            Self::IconReplyMessDeleted => "icons/icon-reply-mess-deleted.svg",
            Self::IdCard => "icons/id-card.svg",
            Self::Image => "icons/image.svg",
            Self::ImageAdd => "icons/image-add.svg",
            Self::Inbox => "icons/inbox.svg",
            Self::Info => "icons/info.svg",
            Self::InfoIcon => "icons/info-icon.svg",
            Self::Join => "icons/join.svg",
            Self::Keyboard => "icons/keyboard.svg",
            Self::Lamp => "icons/lamp.svg",
            Self::Language => "icons/language.svg",
            Self::Laser => "icons/laser.svg",
            Self::LayoutFocus => "icons/layout-focus.svg",
            Self::Leaf => "icons/leaf.svg",
            Self::Leave => "icons/leave.svg",
            Self::LeftArrowIcon => "icons/left-arrow-icon.svg",
            Self::Link => "icons/link.svg",
            Self::Linux => "icons/linux.svg",
            Self::ListBullet => "icons/list-bullet.svg",
            Self::ListCheck => "icons/list-check.svg",
            Self::ListNumber => "icons/list-number.svg",
            Self::Live => "icons/live.svg",
            Self::LoLGame => "icons/lo-l-game.svg",
            Self::Location => "icons/location.svg",
            Self::Lock => "icons/lock.svg",
            Self::Locked => "icons/locked.svg",
            Self::Logout => "icons/logout.svg",
            Self::LongCorner => "icons/long-corner.svg",
            Self::MacAppStoreDesktop => "icons/mac-app-store-desktop.svg",
            Self::MacAppleIntel => "icons/mac-apple-intel.svg",
            Self::MacAppleSilicon => "icons/mac-apple-silicon.svg",
            Self::MacOSCloseIcon => "icons/mac-os-close-icon.svg",
            Self::MacOSMaximizeIcon => "icons/mac-os-maximize-icon.svg",
            Self::MacOSMinimizeIcon => "icons/mac-os-minimize-icon.svg",
            Self::Mail => "icons/mail.svg",
            Self::MarkRead => "icons/mark-read.svg",
            Self::MarkUnread => "icons/mark-unread.svg",
            Self::Members => "icons/members.svg",
            Self::Mention => "icons/mention.svg",
            Self::Menu => "icons/menu.svg",
            Self::MenuBarIcon => "icons/menu-bar-icon.svg",
            Self::MenuClose => "icons/menu-close.svg",
            Self::MessageAdd => "icons/message-add.svg",
            Self::MessageIcon => "icons/message-icon.svg",
            Self::Messages => "icons/messages.svg",
            Self::Mic => "icons/mic.svg",
            Self::MicDisable => "icons/mic-disable.svg",
            Self::MicIcon => "icons/mic-icon.svg",
            Self::MicOff => "icons/mic-off.svg",
            Self::Microphone => "icons/microphone.svg",
            Self::Microsoft => "icons/microsoft.svg",
            Self::MicrosoftDropdown => "icons/microsoft-dropdown.svg",
            Self::MicrosoftWinPortable => "icons/microsoft-win-portable.svg",
            Self::MinusCircle => "icons/minus-circle.svg",
            Self::Moon => "icons/moon.svg",
            Self::More => "icons/more.svg",
            Self::MoreVertical => "icons/more-vertical.svg",
            Self::Move => "icons/move.svg",
            Self::Music => "icons/music.svg",
            Self::NoiseSuppression => "icons/noise-suppression.svg",
            Self::NoiseSuppressionOff => "icons/noise-suppression-off.svg",
            Self::NotJoinedSFU => "icons/not-joined-sfu.svg",
            Self::OAuth2Setting => "icons/o-auth2-setting.svg",
            Self::OpenMenu => "icons/open-menu.svg",
            Self::Owner => "icons/owner.svg",
            Self::OwnerTransfer => "icons/owner-transfer.svg",
            Self::Palette => "icons/palette.svg",
            Self::Paragraph => "icons/paragraph.svg",
            Self::Paste => "icons/paste.svg",
            Self::Pause => "icons/pause.svg",
            Self::PhoneOff => "icons/phone-off.svg",
            Self::Pin => "icons/pin.svg",
            Self::PinOff => "icons/pin-off.svg",
            Self::Play => "icons/play.svg",
            Self::Plus => "icons/plus.svg",
            Self::PlusCircle => "icons/plus-circle.svg",
            Self::Poll => "icons/poll.svg",
            Self::PrivateChatIcon => "icons/private-chat-icon.svg",
            Self::Ptt => "icons/ptt.svg",
            Self::Qr => "icons/qr.svg",
            Self::QrScan => "icons/qr-scan.svg",
            Self::QuickMenusIcon => "icons/quick-menus.svg",
            Self::Quote => "icons/quote.svg",
            Self::Record => "icons/record.svg",
            Self::Redo => "icons/redo.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Reply => "icons/reply.svg",
            Self::ReplyCorner => "icons/reply-corner.svg",
            Self::Rewind15 => "icons/rewind-15.svg",
            Self::Role => "icons/role.svg",
            Self::RotateIcon => "icons/rotate-icon.svg",
            Self::RotateLeft => "icons/rotate-left.svg",
            Self::RotateRight => "icons/rotate-right.svg",
            Self::Rules => "icons/rules.svg",
            Self::School => "icons/school.svg",
            Self::ScreenShare => "icons/screen-share.svg",
            Self::ScreenShareOff => "icons/screen-share-off.svg",
            Self::Search => "icons/search.svg",
            Self::Send => "icons/send.svg",
            Self::SendMoney => "icons/send-money.svg",
            Self::Settings => "icons/settings.svg",
            Self::ShadowBotIcon => "icons/shadow-bot-icon.svg",
            Self::Share => "icons/share.svg",
            Self::Shield => "icons/shield.svg",
            Self::Shop => "icons/shop.svg",
            Self::ShortCorner => "icons/short-corner.svg",
            Self::Sidebar => "icons/sidebar.svg",
            Self::Slash => "icons/slash.svg",
            Self::Sliders => "icons/sliders.svg",
            Self::SortByBigSizeBtn => "icons/sort-by-big-size-btn.svg",
            Self::SortBySmallSizeBtn => "icons/sort-by-small-size-btn.svg",
            Self::Soundboard => "icons/soundboard.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::Speaker => "icons/speaker.svg",
            Self::SpeakerLock => "icons/speaker-lock.svg",
            Self::SpeakerLow => "icons/speaker-low.svg",
            Self::SpeakerOff => "icons/speaker-off.svg",
            Self::Spinner => "icons/spinner.svg",
            Self::Spotify => "icons/spotify.svg",
            Self::Square => "icons/square.svg",
            Self::SquareCheck => "icons/square-check.svg",
            Self::Star => "icons/star.svg",
            Self::StartCall => "icons/start-call.svg",
            Self::StatusDnd => "icons/status-dnd.svg",
            Self::StatusIdle => "icons/status-idle.svg",
            Self::StatusOffline => "icons/status-offline.svg",
            Self::StatusOnline => "icons/status-online.svg",
            Self::Sticker => "icons/sticker.svg",
            Self::Stop => "icons/stop.svg",
            Self::StopCall => "icons/stop-call.svg",
            Self::StraightLineIcon => "icons/straight-line-icon.svg",
            Self::SvgQualityExcellentIcon => "icons/svg-quality-excellent-icon.svg",
            Self::SvgQualityGoodIcon => "icons/svg-quality-good-icon.svg",
            Self::SvgQualityPoorIcon => "icons/svg-quality-poor-icon.svg",
            Self::SvgQualityUnknownIcon => "icons/svg-quality-unknown-icon.svg",
            Self::Swap => "icons/swap.svg",
            Self::Sync => "icons/sync.svg",
            Self::TVIcon => "icons/tv-icon.svg",
            Self::Target => "icons/target.svg",
            Self::Text => "icons/text.svg",
            Self::Thread => "icons/thread.svg",
            Self::ThreadAdd => "icons/thread-add.svg",
            Self::ThreadPrivate => "icons/thread-private.svg",
            Self::ToolIcon => "icons/tool-icon.svg",
            Self::Tools => "icons/tools.svg",
            Self::TopRight => "icons/top-right.svg",
            Self::Topic => "icons/topic.svg",
            Self::Transaction => "icons/transaction.svg",
            Self::Trash => "icons/trash.svg",
            Self::Trending => "icons/trending.svg",
            Self::Tv => "icons/tv.svg",
            Self::Typing => "icons/typing.svg",
            Self::UnLockIcon => "icons/un-lock-icon.svg",
            Self::Undo => "icons/undo.svg",
            Self::Unlock => "icons/unlock.svg",
            Self::UploadImageIcon => "icons/upload-image-icon.svg",
            Self::User => "icons/user.svg",
            Self::UserAvatarIcon => "icons/user-avatar-icon.svg",
            Self::UserBlock => "icons/user-block.svg",
            Self::UserCheck => "icons/user-check.svg",
            Self::UserCircle => "icons/user-circle.svg",
            Self::UserMinus => "icons/user-minus.svg",
            Self::UserPending => "icons/user-pending.svg",
            Self::UserPlus => "icons/user-plus.svg",
            Self::UserUnblock => "icons/user-unblock.svg",
            Self::UtilitiesIcon => "icons/utilities-icon.svg",
            Self::Verified => "icons/verified.svg",
            Self::Video => "icons/video.svg",
            Self::VideoDisable => "icons/video-disable.svg",
            Self::VideoIcon => "icons/video-icon.svg",
            Self::VideoOff => "icons/video-off.svg",
            Self::VisualStudioCode => "icons/visual-studio-code.svg",
            Self::VoiceIcon => "icons/voice-icon.svg",
            Self::Wallet => "icons/wallet.svg",
            Self::Warning => "icons/warning.svg",
            Self::Webhook => "icons/webhook.svg",
            Self::Wifi => "icons/wifi.svg",
            Self::WifiOff => "icons/wifi-off.svg",
            Self::WindowMaximize => "icons/window-maximize.svg",
            Self::WindowMinimize => "icons/window-minimize.svg",
            Self::WindowRestore => "icons/window-restore.svg",
            Self::WindowZoom => "icons/window-zoom.svg",
            Self::Windows => "icons/windows.svg",
            Self::ZoomIn => "icons/zoom-in.svg",
        }
    }
}

pub struct Icon {
    pub(crate) svg: Svg,
    path: &'static str,
    motion: Option<MotionSpec>,
    id: ElementId,
    motion_enabled: bool,
    explicit_transformation: bool,
    base_transformation: Transformation,
    pub(crate) selected: bool,
    pub(crate) preserve_color: bool,
}

impl Icon {
    pub fn motion_scope(element: impl IntoElement, enabled: bool) -> crate::IconMotionScope {
        crate::IconMotionScope::new(element, enabled)
    }

    #[track_caller]
    pub fn new(name: IconName) -> Self {
        let motion = MotionSpec::for_path(name.path())
            .filter(|spec| spec.has_hover_motion())
            .copied();
        Self {
            svg: svg().path(name.path()).size(px(16.)).flex_none(),
            path: name.path(),
            motion,
            id: ElementId::CodeLocation(*std::panic::Location::caller()),
            motion_enabled: true,
            explicit_transformation: false,
            base_transformation: Transformation::default(),
            selected: false,
            preserve_color: false,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn color(mut self, color: impl Into<gpui::Hsla>) -> Self {
        self.svg.style().text.color = Some(color.into());
        self.preserve_color = true;
        self
    }

    pub fn motion_enabled(mut self, enabled: bool) -> Self {
        self.motion_enabled = enabled;
        self
    }

    pub fn with_transformation(mut self, transformation: Transformation) -> Self {
        self.svg = self.svg.with_transformation(transformation);
        self.explicit_transformation = true;
        self.base_transformation = transformation;
        self
    }

    #[cfg(test)]
    fn resolved_base_color(&mut self, window: &Window, cx: &App) -> gpui::Hsla {
        let explicit = self.svg.style().text.color;
        self.resolve_color(explicit, window, cx)
    }

    fn resolve_color(&self, explicit: Option<gpui::Hsla>, window: &Window, cx: &App) -> gpui::Hsla {
        if self.preserve_color
            && let Some(color) = explicit
        {
            return color;
        }
        if MotionSpec::for_path(self.path).is_some()
            && let Some(theme) = mezon_theme::try_theme(cx)
        {
            if self.selected {
                return theme.tokens.icon_selected().into();
            }
            if let Some(color) = explicit {
                if color == gpui::Hsla::from(theme.tokens.bg_icon_theme_active) {
                    return theme.tokens.icon_selected().into();
                }
                if [
                    theme.tokens.bg_icon_theme,
                    theme.text_secondary,
                    theme.text_muted,
                    theme.tokens.text_theme_primary,
                    theme.tokens.text_secondary,
                ]
                .into_iter()
                .any(|neutral| color == gpui::Hsla::from(neutral))
                {
                    return theme.tokens.icon_normal().into();
                }
            }
            return explicit.unwrap_or_else(|| theme.tokens.icon_normal().into());
        }
        explicit.unwrap_or_else(|| window.text_style().color)
    }

    fn glyph_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let aspect_ratio = match self.path {
            "icons/caret-up.svg" | "icons/caret-down.svg" => 2.,
            "icons/caret-left.svg" | "icons/caret-right.svg" => 0.5,
            _ => return bounds,
        };
        let width = bounds.size.width.min(bounds.size.height * aspect_ratio);
        let height = width / aspect_ratio;
        Bounds::new(
            bounds.center() - point(width / 2., height / 2.),
            size(width, height),
        )
    }

    fn supports_hover_motion(&self) -> bool {
        self.motion_enabled
            && self
                .motion
                .is_some_and(|spec| !self.explicit_transformation || spec.translates_only())
    }
}

impl IntoElement for Icon {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut StyleRefinement {
        self.svg.style()
    }
}

impl InteractiveElement for Icon {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.svg.interactivity()
    }
}

impl Element for Icon {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(Element::id(&self.svg).unwrap_or_else(|| self.id.clone()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.svg.source_location()
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.svg.request_layout(global_id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = self
            .svg
            .prepaint(global_id, inspector_id, bounds, state, window, cx);
        hitbox.or_else(|| {
            (self.supports_hover_motion() && crate::icon_motion::IconMotionPreferences::enabled(cx))
                .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal))
        })
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut inner = std::mem::replace(&mut self.svg, svg());
        inner.interactivity().paint(
            global_id,
            inspector_id,
            bounds,
            hitbox.as_ref(),
            window,
            cx,
            |style, window, cx| {
                let mut paint_bounds = self.glyph_bounds(bounds);
                let target_color = self.resolve_color(style.text.color, window, cx);
                let mut paint_color = target_color;
                let mut transformation = self.base_transformation;
                if self.motion_enabled
                    && crate::icon_motion::IconMotionPreferences::enabled(cx)
                    && let Some(global_id) = global_id
                {
                    let enabled = self.supports_hover_motion()
                        && window.is_window_active()
                        && crate::icon_motion::IconMotionPreferences::enabled(cx);
                    let hovered = enabled
                        && crate::icon_motion::scoped_hover(cx).unwrap_or_else(|| {
                            hitbox.as_ref().is_some_and(|h| h.is_hovered(window))
                        });
                    let now = std::time::Instant::now();
                    let (frame, color, color_running) = window.with_element_state(
                        global_id,
                        |playback: Option<IconPlayback>, _| {
                            let mut playback = playback.unwrap_or_default();
                            let frame = playback.motion.frame(
                                self.path,
                                self.motion.as_ref(),
                                hovered,
                                now,
                            );
                            let (color, running) = playback.color.frame(target_color, now);
                            ((frame, color, running), playback)
                        },
                    );
                    paint_color = color;
                    if color_running && window.is_window_active() {
                        window.request_animation_frame();
                    }
                    if let Some(frame) = frame {
                        if self.explicit_transformation {
                            paint_bounds = frame.translated_bounds(paint_bounds);
                        } else {
                            transformation = frame.transformation(paint_bounds);
                        }
                        if frame.running {
                            window.request_animation_frame();
                        }
                    }
                }
                svg()
                    .path(self.path)
                    .with_transformation(transformation)
                    .text_color(paint_color)
                    .paint(None, None, paint_bounds, &mut (), &mut None, window, cx);
            },
        );
        self.svg = inner;
        if self.supports_hover_motion()
            && crate::icon_motion::IconMotionPreferences::enabled(cx)
            && crate::icon_motion::scoped_hover(cx).is_none()
            && let Some(hitbox) = hitbox.as_ref()
        {
            let hitbox = hitbox.clone();
            let view = window.current_view();
            let mut was_hovered = hitbox.is_hovered(window);
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let hovered = hitbox.is_hovered(window);
                if phase == DispatchPhase::Capture && hovered != was_hovered {
                    was_hovered = hovered;
                    cx.notify(view);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, MouseButton, Render, TestApp, div, point};

    #[derive(Clone)]
    struct PaintSample {
        id: GlobalElementId,
        hitbox: bool,
        started: Option<std::time::Instant>,
        color: gpui::Hsla,
    }

    struct PaintProbe {
        icon: Icon,
        target: gpui::Hsla,
        samples: std::rc::Rc<std::cell::RefCell<Vec<PaintSample>>>,
    }

    impl IntoElement for PaintProbe {
        type Element = Self;
        fn into_element(self) -> Self {
            self
        }
    }

    impl Element for PaintProbe {
        type RequestLayoutState = ();
        type PrepaintState = Option<Hitbox>;
        fn id(&self) -> Option<ElementId> {
            Element::id(&self.icon)
        }
        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
            self.icon.source_location()
        }
        fn request_layout(
            &mut self,
            id: Option<&GlobalElementId>,
            inspector: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, ()) {
            self.icon.request_layout(id, inspector, window, cx)
        }
        fn prepaint(
            &mut self,
            id: Option<&GlobalElementId>,
            inspector: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            state: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) -> Option<Hitbox> {
            self.icon.prepaint(id, inspector, bounds, state, window, cx)
        }
        fn paint(
            &mut self,
            id: Option<&GlobalElementId>,
            inspector: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            state: &mut (),
            hitbox: &mut Option<Hitbox>,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.icon
                .paint(id, inspector, bounds, state, hitbox, window, cx);
            let id = id.unwrap();
            let (started, color) =
                window.with_element_state(id, |state: Option<IconPlayback>, _| {
                    let mut state = state.unwrap_or_default();
                    let color = if state.color.has_target() {
                        state.color.frame(self.target, std::time::Instant::now()).0
                    } else {
                        self.target
                    };
                    ((state.motion.started_at(), color), state)
                });
            self.samples.borrow_mut().push(PaintSample {
                id: id.clone(),
                hitbox: hitbox.is_some(),
                started,
                color,
            });
        }
    }

    struct PlaybackFixture {
        users: Vec<usize>,
        color: gpui::Hsla,
        clicks: usize,
        samples: std::rc::Rc<std::cell::RefCell<Vec<PaintSample>>>,
    }

    impl Render for PlaybackFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("playback-fixture")
                .flex()
                .flex_col()
                .children(self.users.iter().map(|user| {
                    div()
                        .id(("user", *user))
                        .w(px(80.))
                        .h(px(40.))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.clicks += 1;
                            cx.notify();
                        }))
                        .child(PaintProbe {
                            icon: Icon::new(IconName::HandRaise)
                                .size(px(24.))
                                .color(self.color),
                            target: self.color,
                            samples: self.samples.clone(),
                        })
                }))
        }
    }

    #[test]
    fn keyed_repeated_icons_keep_independent_playback_when_rows_change() {
        use std::{cell::RefCell, rc::Rc};
        let mut app = TestApp::new();
        let samples = Rc::new(RefCell::new(Vec::new()));
        let mut window = app.open_window(|window, _| {
            window.activate_window();
            PlaybackFixture {
                users: vec![1, 2],
                color: gpui::white(),
                clicks: 0,
                samples: samples.clone(),
            }
        });
        window.update(|_, window, _| window.activate_window());
        window.draw();
        window.simulate_mouse_move(point(px(10.), px(10.)));
        samples.borrow_mut().clear();
        window.draw();
        let first = samples.borrow().clone();
        assert_ne!(first[0].id, first[1].id);
        assert!(first[0].started.is_some());
        assert!(first[1].started.is_none());
        samples.borrow_mut().clear();
        window.draw();
        assert_eq!(samples.borrow()[0].started, first[0].started);
        window.update(|view, _, cx| {
            view.users = vec![2, 1];
            cx.notify();
        });
        window.draw();
        samples.borrow_mut().clear();
        window.draw();
        assert_eq!(samples.borrow()[0].id, first[1].id);
        assert_eq!(samples.borrow()[1].id, first[0].id);
        assert!(samples.borrow()[0].started.is_some());
        assert!(samples.borrow()[1].started.is_none());
        window.update(|view, _, cx| {
            view.users = vec![2];
            cx.notify();
        });
        window.draw();
        window.simulate_click(point(px(10.), px(10.)), MouseButton::Left);
        assert_eq!(window.read(|view, _| view.clicks), 1);
    }

    #[test]
    fn animations_off_skip_motion_hitboxes_and_apply_colors_immediately() {
        use gpui::AppContext;
        use std::{cell::RefCell, rc::Rc};
        let mut app = TestApp::new();
        let settings = app.update(|cx| {
            let settings = cx.new(|_| mezon_store::Settings {
                icon_motion: false,
                ..Default::default()
            });
            mezon_store::Settings::init_global(&settings, cx);
            settings
        });
        let samples = Rc::new(RefCell::new(Vec::new()));
        let mut window = app.open_window(|window, _| {
            window.activate_window();
            PlaybackFixture {
                users: vec![1],
                color: gpui::white(),
                clicks: 0,
                samples: samples.clone(),
            }
        });
        window.update(|_, window, _| window.activate_window());
        window.draw();
        assert!(!samples.borrow().last().unwrap().hitbox);
        window.update(|view, _, cx| {
            view.color = gpui::black();
            cx.notify();
        });
        window.draw();
        assert_eq!(samples.borrow().last().unwrap().color, gpui::black());
        window.simulate_mouse_move(point(px(10.), px(10.)));
        window.simulate_click(point(px(10.), px(10.)), MouseButton::Left);
        assert_eq!(window.read(|view, _| view.clicks), 1);
        window.update(|_, _, cx| {
            settings.update(cx, |settings, cx| {
                settings.icon_motion = true;
                cx.notify();
            });
            cx.refresh_windows();
        });
        window.draw();
        assert!(samples.borrow().last().unwrap().hitbox);
        assert!(samples.borrow().last().unwrap().started.is_some());
        window.update(|view, _, cx| {
            settings.update(cx, |settings, cx| {
                settings.icon_motion = false;
                cx.notify();
            });
            view.color = gpui::white();
            cx.refresh_windows();
            cx.notify();
        });
        window.draw();
        assert!(!samples.borrow().last().unwrap().hitbox);
    }

    #[test]
    fn caret_glyphs_fit_square_and_rectangular_slots_without_changing_layout() {
        for (name, ratio) in [
            (IconName::CaretUp, 2.),
            (IconName::CaretDown, 2.),
            (IconName::CaretLeft, 0.5),
            (IconName::CaretRight, 0.5),
        ] {
            let mut icon = Icon::new(name).size(px(16.));
            assert_eq!(icon.svg.style().size.width, Some(px(16.).into()));
            assert_eq!(icon.svg.style().size.height, Some(px(16.).into()));
            for slot in [
                size(px(16.), px(16.)),
                size(px(12.), px(6.)),
                size(px(6.), px(12.)),
            ] {
                let bounds = Bounds::new(point(px(10.), px(20.)), slot);
                let glyph = icon.glyph_bounds(bounds);
                assert_eq!(glyph.center(), bounds.center());
                assert_eq!(glyph.size.width, glyph.size.height * ratio);
                assert!(glyph.size.width <= slot.width);
                assert!(glyph.size.height <= slot.height);
            }
        }
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(16.), px(16.)));
        assert_eq!(Icon::new(IconName::Bell).glyph_bounds(bounds), bounds);
    }

    #[test]
    fn one_asset_supports_different_display_sizes() {
        for size in [12., 18., 20., 32., 48., 64.] {
            let mut icon = Icon::new(IconName::Bell).size(px(size));
            assert_eq!(icon.path, "icons/bell.svg");
            assert_eq!(icon.svg.style().size.width, Some(px(size).into()));
        }
    }

    #[test]
    fn legacy_static_icons_keep_identity_through_layout() {
        struct StaticFixture {
            name: IconName,
        }

        impl Render for StaticFixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().child(Icon::new(self.name).motion_enabled(false).size(px(20.)))
            }
        }

        let mut app = TestApp::new();
        for name in [IconName::StatusOnline, IconName::ImageAdd] {
            let mut window = app.open_window(move |_, _| StaticFixture { name });
            window.draw();
            window.draw();
        }
    }

    #[test]
    fn icons_follow_runtime_theme_and_preserve_surface_foregrounds() {
        let mut app = TestApp::new();
        let mut window = app.open_window(|_, _| Fixture {
            clicks: 0,
            transformed: false,
            hover_style: false,
            renders: 0,
        });
        for name in ["dark", "light", "purple_haze", "sunrise"] {
            window.update(|_, window, cx| {
                let theme = mezon_theme::resolve_theme(name);
                let normal = theme.tokens.icon_normal();
                let selected = theme.tokens.icon_selected();
                mezon_theme::set_theme(theme, cx);
                assert_eq!(
                    Icon::new(IconName::Bell).resolved_base_color(window, cx),
                    normal.into()
                );
                assert_eq!(
                    Icon::new(IconName::Bell)
                        .selected(true)
                        .resolved_base_color(window, cx),
                    selected.into()
                );
                let foreground = gpui::rgb(0xff0000);
                assert_eq!(
                    Icon::new(IconName::Bell)
                        .color(foreground)
                        .resolved_base_color(window, cx),
                    foreground.into()
                );
            });
        }
    }

    struct Fixture {
        clicks: usize,
        transformed: bool,
        hover_style: bool,
        renders: usize,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            div()
                .id("icon-action")
                .size(px(40.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.clicks += 1;
                    cx.notify();
                }))
                .child(
                    Icon::new(IconName::ImageAdd)
                        .size(px(20.))
                        .text_color(gpui::white())
                        .when(self.hover_style, |icon| {
                            icon.hover(|style| style.text_color(gpui::black()))
                        })
                        .when(self.transformed, |icon| {
                            icon.with_transformation(Transformation::rotate(gpui::radians(1.)))
                        }),
                )
        }
    }

    #[test]
    fn animated_icon_keeps_parent_clicks_and_explicit_transform_consumer() {
        let mut app = TestApp::new();
        let mut window = app.open_window(|_, _| Fixture {
            clicks: 0,
            transformed: false,
            hover_style: true,
            renders: 0,
        });
        window.draw();
        window.simulate_mouse_move(point(px(10.), px(10.)));
        window.draw();
        window.simulate_click(point(px(10.), px(10.)), MouseButton::Left);
        assert_eq!(window.read(|view, _| view.clicks), 1);
        window.update(|view, _, cx| {
            view.transformed = true;
            cx.notify();
        });
        window.draw();
        window.simulate_click(point(px(10.), px(10.)), MouseButton::Left);
        assert_eq!(window.read(|view, _| view.clicks), 2);
        app.update(|cx| {
            cx.set_global(crate::IconMotionPreferences {
                reduced_motion: true,
            })
        });
        window.draw();
        window.simulate_click(point(px(10.), px(10.)), MouseButton::Left);
        assert_eq!(window.read(|view, _| view.clicks), 3);
    }

    #[test]
    fn unstyled_icon_repaints_on_pointer_entry_and_exit() {
        let mut app = TestApp::new();
        let mut window = app.open_window(|_, _| Fixture {
            clicks: 0,
            transformed: false,
            hover_style: false,
            renders: 0,
        });
        window.draw();
        window.simulate_mouse_move(point(px(60.), px(60.)));
        window.draw();
        let outside = window.read(|view, _| view.renders);
        window.simulate_mouse_move(point(px(10.), px(10.)));
        window.draw();
        let inside = window.read(|view, _| view.renders);
        assert!(inside > outside);
        window.simulate_mouse_move(point(px(60.), px(60.)));
        window.draw();
        assert!(window.read(|view, _| view.renders) > inside);
        assert_eq!(window.read(|view, _| view.clicks), 0);
    }

    #[test]
    fn icon_tint_tracks_parent_theme_changes_and_keeps_explicit_colors() {
        use std::{cell::Cell, rc::Rc};

        #[derive(IntoElement)]
        struct TintProbe {
            icon: Icon,
            observed: Rc<Cell<Option<gpui::Hsla>>>,
        }

        impl RenderOnce for TintProbe {
            fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
                self.observed
                    .set(Some(self.icon.resolved_base_color(window, cx)));
                self.icon
            }
        }

        struct TintFixture {
            color: gpui::Hsla,
            explicit_color: gpui::Hsla,
            inherited: Rc<Cell<Option<gpui::Hsla>>>,
            explicit: Rc<Cell<Option<gpui::Hsla>>>,
        }

        impl Render for TintFixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .text_color(self.color)
                    .child(TintProbe {
                        icon: Icon::new(IconName::Friends),
                        observed: self.inherited.clone(),
                    })
                    .child(TintProbe {
                        icon: Icon::new(IconName::Friends).text_color(self.explicit_color),
                        observed: self.explicit.clone(),
                    })
            }
        }

        let mut app = TestApp::new();
        let inherited = Rc::new(Cell::new(None));
        let explicit = Rc::new(Cell::new(None));
        let explicit_color: gpui::Hsla = gpui::rgb(0x5865f2).into();
        let mut window = app.open_window(|_, _| TintFixture {
            color: gpui::white(),
            explicit_color,
            inherited: inherited.clone(),
            explicit: explicit.clone(),
        });
        for name in ["dark", "light", "purple_haze"] {
            let theme = mezon_theme::resolve_theme(name);
            for color in [theme.tokens.text_secondary, theme.tokens.text_theme_primary] {
                let color = color.into();
                window.update(|view, _, cx| {
                    view.color = color;
                    cx.notify();
                });
                window.draw();
                assert_eq!(inherited.get(), Some(color));
                assert_eq!(explicit.get(), Some(explicit_color));
            }
        }
    }
}
