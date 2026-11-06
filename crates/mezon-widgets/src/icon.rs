use gpui::{
    App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, MouseMoveEvent,
    Pixels, StyleRefinement, Svg, Transformation, Window, prelude::*, px, svg,
};

use crate::icon_motion::{MotionPlayback, MotionSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    AddCircle,
    AddIcon,
    AddPerson,
    AddServe,
    AdminHomeIcon,
    AdminSettingIcon,
    AnnotateArrow,
    AnnotateEllipse,
    AnnotateGrip,
    AnnotateHighlighter,
    AnnotateLaser,
    AnnotatePointer,
    AnnotateRectangle,
    AnnotateSelect,
    Announcement,
    AnonymousAvatar,
    AppChannelIcon,
    AppDirectoryFooterRobot,
    AppHelpIcon,
    Apple,
    AppStore,
    AppStoreBadge,
    ArrowDown,
    ArrowDownFill,
    ArrowLeft,
    ArrowLeftCircle,
    ArrowLeftCircleActive,
    ArrowRight,
    AspectRatioIcon,
    AudioLevelCircle,
    AudioPause,
    AudioPlay,
    AuditLogIcon,
    AvatarUser,
    BackToCategoriesGif,
    Bell,
    BlockquoteIcon,
    BulletListIcon,
    CalendarIcon,
    CallSetting,
    CancelCall,
    EndCall,
    CanvasIcon,
    CDNIcon,
    ChannelBrowser,
    ChannelAppFallback,
    Chat,
    Check,
    CheckedIcon,
    CheckIcon,
    CheckListIcon,
    CheckMarkFilter,
    CaretDown,
    CaretRight,
    Chevron,
    ChevronDown,
    ChevronDownIcon,
    ChevronDownThin,
    ChevronRight,
    CirclePlusFill,
    ClanGuideHouse,
    ClanGuideSeal,
    ClockIcon,
    Close,
    CloseButton,
    CloseIcon,
    Community,
    CompassIcon,
    ConvertAccount,
    CopyIcon,
    CreateCategoryIcon,
    DarkModeIcon,
    DatePickerIndicator,
    DeviceDesktopIcon,
    DeviceMobileIcon,
    DollarIcon,
    DotIcon,
    Download,
    EmptyMention,
    EmptyUnread,
    EmptyUnreadStyle,
    Eraser,
    EscIcon,
    ExitFullScreen,
    EyeClose,
    EyeOpen,
    FileAndFolder,
    FileIcon,
    FileThumbAudio,
    FileThumbCode,
    FileThumbDoc,
    FileThumbEmpty,
    FileThumbJson,
    FileThumbPdf,
    FileThumbRar,
    FileThumbXls,
    FiltersIcon,
    Flower,
    Forum,
    FullScreen,
    GamingConsoleIcon,
    Gif,
    GlobalIcon,
    GooglePlay,
    GooglePlayBadge,
    GuideIcon,
    H1Icon,
    H2Icon,
    H3Icon,
    Hashtag,
    HashtagLock,
    HashtagLocked,
    HashtagWarning,
    HatIcon,
    HeadPhoneICon,
    History,
    HistoryTransaction,
    HomepageDownload,
    HomepageMenu,
    IConAcceptFriend,
    IconAddFriendDM,
    IconClock,
    IconClockChannel,
    IconEditThreeDot,
    IconEvents,
    IconFriend,
    IconFriends,
    IConIgnoreFriend,
    IconLock,
    IconMeetDM,
    IconOr,
    IconPhoneDM,
    IconRemove,
    IconReplyMessDeleted,
    IconReplyMessDeletedWeb,
    IconShareContact,
    IConShareEventLocation,
    IconShareEventVoice,
    IconTick,
    IconUserProfileDM,
    EmojiCatStar,
    EmojiCatLeaf,
    EmojiCatBowl,
    EmojiCatGame,
    EmojiCatBicycle,
    EmojiCatHeart,
    EmojiCatRibbon,
    EmojiCatObject,
    ImageThumbnail,
    ImageUploadIcon,
    Inbox,
    IncomingCall,
    InfoIcon,
    InPttCall,
    Joystick,
    LeaveClanIcon,
    LeftArrowIcon,
    Linux,
    LoadingSpinner,
    Location,
    Locked,
    LockedPrivate,
    LockIcon,
    LoLGame,
    LongArrowRight,
    LongCorner,
    LoudVolume,
    LowVolume,
    MacAppleIntel,
    MacAppleSilicon,
    MacAppStoreDesktop,
    MacOSCloseIcon,
    MacOSMaximizeIcon,
    MacOSMinimizeIcon,
    MarketIcons,
    MemberIcon,
    MemberList,
    MenuBarIcon,
    MenuClose,
    MessageIcon,
    MessageSquareIcon,
    Mic,
    MicDisable,
    MicEnable,
    MicIcon,
    Microphone,
    Microsoft,
    MicrosoftDropdown,
    MicrosoftWinPortable,
    MinusCircleIcon,
    MissedCall,
    MuteBell,
    MutedVolume,
    NetworkStatus,
    NoiseSupressionDisabledIcon,
    NoiseSupressionIcon,
    NotJoinedSFU,
    OAuth2Setting,
    OfflineStatus,
    OnlineStatus,
    OpenMenu,
    OrderedListIcon,
    OutGoingCall,
    OwnerIcon,
    PaintTray,
    ParagraphIcon,
    PasteIcon,
    PauseButton,
    PauseIcon,
    PendingFriend,
    PenEdit,
    People,
    PhoneOff,
    Pin,
    PinList,
    PinRight,
    PlayButton,
    Plus,
    PlusIcon,
    Private,
    PrivateAppChannelIcon,
    PrivateChatIcon,
    QrCode,
    ReloadIcon,
    ReplyCorner,
    ResendMessageRightClick,
    RightFilledTriangle,
    RightIcon,
    RoleIcon,
    RotateIcon,
    RotateLeftIcon,
    RotateRightIcon,
    RuleIcon,
    School,
    Search,
    SearchIcon,
    SelectFileIcon,
    SendMoney,
    Sent,
    SettingProfile,
    Settings,
    ShadowBotIcon,
    ShortCorner,
    SideMenuIcon,
    Smile,
    SmilingFace,
    SortByBigSizeBtn,
    SortBySmallSizeBtn,
    Sparkles,
    Speaker,
    SpeakerLocked,
    Spotify,
    StartCall,
    Sticker,
    StopCall,
    StraightLineIcon,
    Stream,
    SvgQualityExcellentIcon,
    SvgQualityGoodIcon,
    SvgQualityPoorIcon,
    SvgQualityUnknownIcon,
    SyncIcon,
    TargetIcon,
    ThreadEmpty,
    ThreadIcon,
    ThreadIconLocker,
    ThreadLock,
    TextTool,
    ThreeDot,
    Tick,
    ToolIcon,
    TopicIcon,
    TypingIndicator,
    TopRight,
    Transaction,
    TransferOwner,
    TrashIcon,
    TrendingGifs,
    TriangleAlert,
    TVIcon,
    UnLockIcon,
    UpcomingEventIcon,
    UploadImage,
    UploadImageIcon,
    UploadSoundIcon,
    UserAvatarIcon,
    UserIcon,
    UtilitiesIcon,
    VideoDisable,
    VideoIcon,
    VisualStudioCode,
    VoiceArowDownIcon,
    VoiceArowUpIcon,
    VoiceBreakoutIcon,
    VoiceCameraDisabledIcon,
    VoiceCameraIcon,
    VoiceEmojiControlIcon,
    VoiceFocusIcon,
    VoiceGridIcon,
    VoiceIcon,
    VoiceMicDisabledIcon,
    VoiceAgentIcon,
    VoiceMicIcon,
    VoicePopOutIcon,
    VoiceRaiseHandIcon,
    VoiceRecordIcon,
    VoiceRecordStopIcon,
    VoiceScreenShareIcon,
    VoiceScreenShareStopIcon,
    VoiceSoundControlIcon,
    WebhooksIcon,
    WelcomeIcon,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
    Windows,
    WindowZoom,
    ZoomIcon,
    Reply,
    ReplyRightClick,
    PinMessageRightClick,
    CopyTextRightClick,
    DeleteMessageRightClick,
    ReportMessageRightClick,
    DollarIconRightClick,
    ForwardAllRightClick,
    ForwardRightClick,
    EndPollNowIcon,
    MarkUnreadIcon,
    AddToInboxIcon,
    QuickActionEdit,
    QuickActionEmpty,
    QuickActionInfo,
    QuickActionTrash,
    QuickMenusIcon,
}

impl IconName {
    pub fn path(self) -> &'static str {
        match self {
            Self::AddCircle => "icons/add-circle.svg",
            Self::AddIcon => "icons/add-icon.svg",
            Self::AddPerson => "icons/add-person.svg",
            Self::AddServe => "icons/add-serve.svg",
            Self::AdminHomeIcon => "icons/admin-home-icon.svg",
            Self::AdminSettingIcon => "icons/admin-setting-icon.svg",
            Self::AnnotateArrow => "icons/annotate-arrow.svg",
            Self::AnnotateEllipse => "icons/annotate-ellipse.svg",
            Self::AnnotateGrip => "icons/annotate-grip.svg",
            Self::AnnotateHighlighter => "icons/annotate-highlighter.svg",
            Self::AnnotateLaser => "icons/annotate-laser.svg",
            Self::AnnotatePointer => "icons/annotate-pointer.svg",
            Self::AnnotateRectangle => "icons/annotate-rectangle.svg",
            Self::AnnotateSelect => "icons/annotate-select.svg",
            Self::Announcement => "icons/announcement.svg",
            Self::AnonymousAvatar => "icons/anonymous-avatar.svg",
            Self::AppChannelIcon => "icons/app-channel-icon.svg",
            Self::AppDirectoryFooterRobot => "icons/app-directory-footer-robot.svg",
            Self::AppHelpIcon => "icons/app-help-icon.svg",
            Self::Apple => "icons/apple.svg",
            Self::AppStore => "icons/app-store.svg",
            Self::AppStoreBadge => "icons/app-store-badge.svg",
            Self::ArrowDown => "icons/arrow-down.svg",
            Self::ArrowDownFill => "icons/arrow-down-fill.svg",
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowLeftCircle => "icons/arrow-left-circle.svg",
            Self::ArrowLeftCircleActive => "icons/arrow-left-circle-active.svg",
            Self::ArrowRight => "icons/arrow-right.svg",
            Self::AspectRatioIcon => "icons/aspect-ratio-icon.svg",
            Self::AudioLevelCircle => "icons/audio-level-circle.svg",
            Self::AudioPause => "icons/audio-pause.svg",
            Self::AudioPlay => "icons/audio-play.svg",
            Self::AuditLogIcon => "icons/audit-log-icon.svg",
            Self::AvatarUser => "icons/avatar-user.svg",
            Self::BackToCategoriesGif => "icons/back-to-categories-gif.svg",
            Self::Bell => "icons/bell.svg",
            Self::BlockquoteIcon => "icons/blockquote-icon.svg",
            Self::BulletListIcon => "icons/bullet-list-icon.svg",
            Self::CalendarIcon => "icons/calendar-icon.svg",
            Self::CallSetting => "icons/call-setting.svg",
            Self::CancelCall => "icons/cancel-call.svg",
            Self::EndCall => "icons/end-call.svg",
            Self::CanvasIcon => "icons/canvas-icon.svg",
            Self::CDNIcon => "icons/cdn-icon.svg",
            Self::ChannelBrowser => "icons/channel-browser.svg",
            Self::ChannelAppFallback => "icons/channel-app-fallback.svg",
            Self::Chat => "icons/chat.svg",
            Self::Check => "icons/check.svg",
            Self::CheckedIcon => "icons/checked-icon.svg",
            Self::CheckIcon => "icons/check-icon.svg",
            Self::CheckListIcon => "icons/check-list-icon.svg",
            Self::CheckMarkFilter => "icons/check-mark-filter.svg",
            Self::CaretDown => "icons/caret-down.svg",
            Self::CaretRight => "icons/caret-right.svg",
            Self::Chevron => "icons/chevron.svg",
            Self::ChevronDown => "icons/chevron_down.svg",
            Self::ChevronDownIcon => "icons/chevron-down-icon.svg",
            Self::ChevronDownThin => "icons/chevron-down-thin.svg",
            Self::ChevronRight => "icons/chevron_right.svg",
            Self::CirclePlusFill => "icons/circle-plus-fill.svg",
            Self::ClanGuideHouse => "icons/clan-guide-house.svg",
            Self::ClanGuideSeal => "icons/clan-guide-seal.svg",
            Self::ClockIcon => "icons/clock-icon.svg",
            Self::Close => "icons/close.svg",
            Self::CloseButton => "icons/close-button.svg",
            Self::CloseIcon => "icons/close-icon.svg",
            Self::Community => "icons/community.svg",
            Self::CompassIcon => "icons/compass-icon.svg",
            Self::ConvertAccount => "icons/convert-account.svg",
            Self::CopyIcon => "icons/copy-icon.svg",
            Self::CreateCategoryIcon => "icons/create-category-icon.svg",
            Self::DarkModeIcon => "icons/dark-mode-icon.svg",
            Self::DatePickerIndicator => "icons/date-picker-indicator.svg",
            Self::DeviceDesktopIcon => "icons/device-desktop-icon.svg",
            Self::DeviceMobileIcon => "icons/device-mobile-icon.svg",
            Self::DollarIcon => "icons/dollar-icon.svg",
            Self::DotIcon => "icons/dot-icon.svg",
            Self::Download => "icons/download.svg",
            Self::EmptyMention => "icons/empty-mention.svg",
            Self::EmptyUnread => "icons/empty-unread.svg",
            Self::EmptyUnreadStyle => "icons/empty-unread-style.svg",
            Self::Eraser => "icons/eraser.svg",
            Self::EscIcon => "icons/esc-icon.svg",
            Self::ExitFullScreen => "icons/exit-full-screen.svg",
            Self::EyeClose => "icons/eye-close.svg",
            Self::EyeOpen => "icons/eye-open.svg",
            Self::FileAndFolder => "icons/file-and-folder.svg",
            Self::FileIcon => "icons/file-icon.svg",
            Self::FileThumbAudio => "icons/file-thumb-audio.svg",
            Self::FileThumbCode => "icons/file-thumb-code.svg",
            Self::FileThumbDoc => "icons/file-thumb-doc.svg",
            Self::FileThumbEmpty => "icons/file-thumb-empty.svg",
            Self::FileThumbJson => "icons/file-thumb-json.svg",
            Self::FileThumbPdf => "icons/file-thumb-pdf.svg",
            Self::FileThumbRar => "icons/file-thumb-rar.svg",
            Self::FileThumbXls => "icons/file-thumb-xls.svg",
            Self::FiltersIcon => "icons/filters-icon.svg",
            Self::Flower => "icons/flower.svg",
            Self::Forum => "icons/forum.svg",
            Self::FullScreen => "icons/full-screen.svg",
            Self::GamingConsoleIcon => "icons/gaming-console-icon.svg",
            Self::Gif => "icons/gif.svg",
            Self::GlobalIcon => "icons/global-icon.svg",
            Self::GooglePlay => "icons/google-play.svg",
            Self::GooglePlayBadge => "icons/google-play-badge.svg",
            Self::GuideIcon => "icons/guide-icon.svg",
            Self::H1Icon => "icons/h1-icon.svg",
            Self::H2Icon => "icons/h2-icon.svg",
            Self::H3Icon => "icons/h3-icon.svg",
            Self::Hashtag => "icons/hashtag.svg",
            Self::HashtagLock => "icons/hashtag-lock.svg",
            Self::HashtagLocked => "icons/hashtag-locked.svg",
            Self::HashtagWarning => "icons/hashtag-warning.svg",
            Self::HatIcon => "icons/hat-icon.svg",
            Self::HeadPhoneICon => "icons/head-phone-i-con.svg",
            Self::History => "icons/history.svg",
            Self::HistoryTransaction => "icons/history-transaction.svg",
            Self::HomepageDownload => "icons/homepage-download.svg",
            Self::HomepageMenu => "icons/homepage-menu.svg",
            Self::IConAcceptFriend => "icons/i-con-accept-friend.svg",
            Self::IconAddFriendDM => "icons/icon-add-friend-dm.svg",
            Self::IconClock => "icons/icon-clock.svg",
            Self::IconClockChannel => "icons/icon-clock-channel.svg",
            Self::IconEditThreeDot => "icons/icon-edit-three-dot.svg",
            Self::IconEvents => "icons/icon-events.svg",
            Self::IconFriend => "icons/icon-friend.svg",
            Self::IconFriends => "icons/icon-friends.svg",
            Self::IConIgnoreFriend => "icons/i-con-ignore-friend.svg",
            Self::IconLock => "icons/icon-lock.svg",
            Self::IconMeetDM => "icons/icon-meet-dm.svg",
            Self::IconOr => "icons/icon-or.svg",
            Self::IconPhoneDM => "icons/icon-phone-dm.svg",
            Self::IconRemove => "icons/icon-remove.svg",
            Self::IconReplyMessDeleted => "icons/icon-reply-mess-deleted.svg",
            Self::IconReplyMessDeletedWeb => "icons/icon-reply-mess-deleted-web.svg",
            Self::IconShareContact => "icons/icon-share-contact.svg",
            Self::IConShareEventLocation => "icons/i-con-share-event-location.svg",
            Self::IconShareEventVoice => "icons/icon-share-event-voice.svg",
            Self::IconTick => "icons/icon-tick.svg",
            Self::IconUserProfileDM => "icons/icon-user-profile-dm.svg",
            Self::EmojiCatStar => "icons/emoji-cat-star.svg",
            Self::EmojiCatLeaf => "icons/emoji-cat-leaf.svg",
            Self::EmojiCatBowl => "icons/emoji-cat-bowl.svg",
            Self::EmojiCatGame => "icons/emoji-cat-game.svg",
            Self::EmojiCatBicycle => "icons/emoji-cat-bicycle.svg",
            Self::EmojiCatHeart => "icons/emoji-cat-heart.svg",
            Self::EmojiCatRibbon => "icons/emoji-cat-ribbon.svg",
            Self::EmojiCatObject => "icons/emoji-cat-object.svg",
            Self::ImageThumbnail => "icons/image-thumbnail.svg",
            Self::ImageUploadIcon => "icons/image-upload-icon.svg",
            Self::Inbox => "icons/inbox.svg",
            Self::IncomingCall => "icons/incoming-call.svg",
            Self::InfoIcon => "icons/info-icon.svg",
            Self::InPttCall => "icons/in-ptt-call.svg",
            Self::Joystick => "icons/joystick.svg",
            Self::LeaveClanIcon => "icons/leave-clan-icon.svg",
            Self::LeftArrowIcon => "icons/left-arrow-icon.svg",
            Self::Linux => "icons/linux.svg",
            Self::LoadingSpinner => "icons/loading-spinner.svg",
            Self::Location => "icons/location.svg",
            Self::Locked => "icons/locked.svg",
            Self::LockedPrivate => "icons/locked-private.svg",
            Self::LockIcon => "icons/lock-icon.svg",
            Self::LoLGame => "icons/lo-l-game.svg",
            Self::LongArrowRight => "icons/long-arrow-right.svg",
            Self::LongCorner => "icons/long-corner.svg",
            Self::LoudVolume => "icons/loud-volume.svg",
            Self::LowVolume => "icons/low-volume.svg",
            Self::MacAppleIntel => "icons/mac-apple-intel.svg",
            Self::MacAppleSilicon => "icons/mac-apple-silicon.svg",
            Self::MacAppStoreDesktop => "icons/mac-app-store-desktop.svg",
            Self::MacOSCloseIcon => "icons/mac-os-close-icon.svg",
            Self::MacOSMaximizeIcon => "icons/mac-os-maximize-icon.svg",
            Self::MacOSMinimizeIcon => "icons/mac-os-minimize-icon.svg",
            Self::MarketIcons => "icons/market-icons.svg",
            Self::MemberIcon => "icons/member-icon.svg",
            Self::MemberList => "icons/member-list.svg",
            Self::MenuBarIcon => "icons/menu-bar-icon.svg",
            Self::MenuClose => "icons/menu-close.svg",
            Self::MessageIcon => "icons/message-icon.svg",
            Self::MessageSquareIcon => "icons/message-square-icon.svg",
            Self::Mic => "icons/mic.svg",
            Self::MicDisable => "icons/mic-disable.svg",
            Self::MicEnable => "icons/mic-enable.svg",
            Self::MicIcon => "icons/mic-icon.svg",
            Self::Microphone => "icons/microphone.svg",
            Self::Microsoft => "icons/microsoft.svg",
            Self::MicrosoftDropdown => "icons/microsoft-dropdown.svg",
            Self::MicrosoftWinPortable => "icons/microsoft-win-portable.svg",
            Self::MinusCircleIcon => "icons/minus-circle-icon.svg",
            Self::MissedCall => "icons/missed-call.svg",
            Self::MuteBell => "icons/mute-bell.svg",
            Self::MutedVolume => "icons/muted-volume.svg",
            Self::NetworkStatus => "icons/network-status.svg",
            Self::NoiseSupressionDisabledIcon => "icons/noise-supression-disabled-icon.svg",
            Self::NoiseSupressionIcon => "icons/noise-supression-icon.svg",
            Self::NotJoinedSFU => "icons/not-joined-sfu.svg",
            Self::OAuth2Setting => "icons/o-auth2-setting.svg",
            Self::OfflineStatus => "icons/offline-status.svg",
            Self::OnlineStatus => "icons/online-status.svg",
            Self::OpenMenu => "icons/open-menu.svg",
            Self::OrderedListIcon => "icons/ordered-list-icon.svg",
            Self::OutGoingCall => "icons/out-going-call.svg",
            Self::OwnerIcon => "icons/owner-icon.svg",
            Self::PaintTray => "icons/paint-tray.svg",
            Self::ParagraphIcon => "icons/paragraph-icon.svg",
            Self::PasteIcon => "icons/paste-icon.svg",
            Self::PauseButton => "icons/pause-button.svg",
            Self::PauseIcon => "icons/pause-icon.svg",
            Self::PendingFriend => "icons/pending-friend.svg",
            Self::PenEdit => "icons/pen-edit.svg",
            Self::People => "icons/people.svg",
            Self::PhoneOff => "icons/phone-off.svg",
            Self::Pin => "icons/pin.svg",
            Self::PinList => "icons/pin-list.svg",
            Self::PinRight => "icons/pin-right.svg",
            Self::PlayButton => "icons/play-button.svg",
            Self::Plus => "icons/plus.svg",
            Self::PlusIcon => "icons/plus-icon.svg",
            Self::Private => "icons/private.svg",
            Self::PrivateAppChannelIcon => "icons/private-app-channel-icon.svg",
            Self::PrivateChatIcon => "icons/private-chat-icon.svg",
            Self::QrCode => "icons/qr-code.svg",
            Self::ReloadIcon => "icons/reload-icon.svg",
            Self::ReplyCorner => "icons/reply-corner.svg",
            Self::ResendMessageRightClick => "icons/resend-message-right-click.svg",
            Self::RightFilledTriangle => "icons/right-filled-triangle.svg",
            Self::RightIcon => "icons/right-icon.svg",
            Self::RoleIcon => "icons/role-icon.svg",
            Self::RotateIcon => "icons/rotate-icon.svg",
            Self::RotateLeftIcon => "icons/rotate-left-icon.svg",
            Self::RotateRightIcon => "icons/rotate-right-icon.svg",
            Self::RuleIcon => "icons/rule-icon.svg",
            Self::School => "icons/school.svg",
            Self::Search => "icons/search.svg",
            Self::SearchIcon => "icons/search-icon.svg",
            Self::SelectFileIcon => "icons/select-file-icon.svg",
            Self::SendMoney => "icons/send-money.svg",
            Self::Sent => "icons/sent.svg",
            Self::SettingProfile => "icons/setting-profile.svg",
            Self::Settings => "icons/settings.svg",
            Self::ShadowBotIcon => "icons/shadow-bot-icon.svg",
            Self::ShortCorner => "icons/short-corner.svg",
            Self::SideMenuIcon => "icons/side-menu-icon.svg",
            Self::Smile => "icons/smile.svg",
            Self::SmilingFace => "icons/smiling-face.svg",
            Self::SortByBigSizeBtn => "icons/sort-by-big-size-btn.svg",
            Self::SortBySmallSizeBtn => "icons/sort-by-small-size-btn.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::Speaker => "icons/speaker.svg",
            Self::SpeakerLocked => "icons/speaker-locked.svg",
            Self::Spotify => "icons/spotify.svg",
            Self::StartCall => "icons/start-call.svg",
            Self::Sticker => "icons/sticker.svg",
            Self::StopCall => "icons/stop-call.svg",
            Self::StraightLineIcon => "icons/straight-line-icon.svg",
            Self::Stream => "icons/stream.svg",
            Self::SvgQualityExcellentIcon => "icons/svg-quality-excellent-icon.svg",
            Self::SvgQualityGoodIcon => "icons/svg-quality-good-icon.svg",
            Self::SvgQualityPoorIcon => "icons/svg-quality-poor-icon.svg",
            Self::SvgQualityUnknownIcon => "icons/svg-quality-unknown-icon.svg",
            Self::SyncIcon => "icons/sync-icon.svg",
            Self::TargetIcon => "icons/target-icon.svg",
            Self::ThreadEmpty => "icons/thread-empty.svg",
            Self::ThreadIcon => "icons/thread-icon.svg",
            Self::ThreadIconLocker => "icons/thread-icon-locker.svg",
            Self::ThreadLock => "icons/thread-lock.svg",
            Self::TextTool => "icons/text-tool.svg",
            Self::ThreeDot => "icons/three-dot.svg",
            Self::Tick => "icons/tick.svg",
            Self::ToolIcon => "icons/tool-icon.svg",
            Self::TopicIcon => "icons/topic-icon.svg",
            Self::TypingIndicator => "icons/typing-indicator.svg",
            Self::TopRight => "icons/top-right.svg",
            Self::Transaction => "icons/transaction.svg",
            Self::TransferOwner => "icons/transfer-owner.svg",
            Self::TrashIcon => "icons/trash-icon.svg",
            Self::TrendingGifs => "icons/trending-gifs.svg",
            Self::TriangleAlert => "icons/triangle-alert.svg",
            Self::TVIcon => "icons/tv-icon.svg",
            Self::UnLockIcon => "icons/un-lock-icon.svg",
            Self::UpcomingEventIcon => "icons/upcoming-event-icon.svg",
            Self::UploadImage => "icons/upload-image.svg",
            Self::UploadImageIcon => "icons/upload-image-icon.svg",
            Self::UploadSoundIcon => "icons/upload-sound-icon.svg",
            Self::UserAvatarIcon => "icons/user-avatar-icon.svg",
            Self::UserIcon => "icons/user-icon.svg",
            Self::UtilitiesIcon => "icons/utilities-icon.svg",
            Self::VideoDisable => "icons/video-disable.svg",
            Self::VideoIcon => "icons/video-icon.svg",
            Self::VisualStudioCode => "icons/visual-studio-code.svg",
            Self::VoiceArowDownIcon => "icons/voice-arow-down-icon.svg",
            Self::VoiceArowUpIcon => "icons/voice-arow-up-icon.svg",
            Self::VoiceBreakoutIcon => "icons/voice-breakout-icon.svg",
            Self::VoiceCameraDisabledIcon => "icons/voice-camera-disabled-icon.svg",
            Self::VoiceCameraIcon => "icons/voice-camera-icon.svg",
            Self::VoiceEmojiControlIcon => "icons/voice-emoji-control-icon.svg",
            Self::VoiceFocusIcon => "icons/voice-focus-icon.svg",
            Self::VoiceAgentIcon => "icons/voice-agent-icon.svg",
            Self::VoiceGridIcon => "icons/voice-grid-icon.svg",
            Self::VoiceIcon => "icons/voice-icon.svg",
            Self::VoiceMicDisabledIcon => "icons/voice-mic-disabled-icon.svg",
            Self::VoiceMicIcon => "icons/voice-mic-icon.svg",
            Self::VoicePopOutIcon => "icons/voice-pop-out-icon.svg",
            Self::VoiceRaiseHandIcon => "icons/voice-raise-hand-icon.svg",
            Self::VoiceRecordIcon => "icons/voice-record-icon.svg",
            Self::VoiceRecordStopIcon => "icons/voice-record-stop-icon.svg",
            Self::VoiceScreenShareIcon => "icons/voice-screen-share-icon.svg",
            Self::VoiceScreenShareStopIcon => "icons/voice-screen-share-stop-icon.svg",
            Self::VoiceSoundControlIcon => "icons/voice-sound-control-icon.svg",
            Self::WebhooksIcon => "icons/webhooks-icon.svg",
            Self::WelcomeIcon => "icons/welcome-icon.svg",
            Self::WindowClose => "icons/window-close.svg",
            Self::WindowMaximize => "icons/window-maximize.svg",
            Self::WindowMinimize => "icons/window-minimize.svg",
            Self::WindowRestore => "icons/window-restore.svg",
            Self::Windows => "icons/windows.svg",
            Self::WindowZoom => "icons/window-zoom.svg",
            Self::ZoomIcon => "icons/zoom-icon.svg",
            Self::Reply => "icons/reply.svg",
            Self::ReplyRightClick => "icons/reply-right-click.svg",
            Self::PinMessageRightClick => "icons/pin-message-right-click.svg",
            Self::CopyTextRightClick => "icons/copy-text-right-click.svg",
            Self::DeleteMessageRightClick => "icons/delete-message-right-click.svg",
            Self::ReportMessageRightClick => "icons/report-message-right-click.svg",
            Self::DollarIconRightClick => "icons/dollar-icon-right-click.svg",
            Self::ForwardAllRightClick => "icons/forward-all-right-click.svg",
            Self::ForwardRightClick => "icons/forward-right-click.svg",
            Self::EndPollNowIcon => "icons/end-poll-now.svg",
            Self::MarkUnreadIcon => "icons/mark-unread.svg",
            Self::AddToInboxIcon => "icons/add-to-inbox.svg",
            Self::QuickActionEdit => "icons/quick-action-edit.svg",
            Self::QuickActionEmpty => "icons/quick-action-empty.svg",
            Self::QuickActionInfo => "icons/quick-action-info.svg",
            Self::QuickActionTrash => "icons/quick-action-trash.svg",
            Self::QuickMenusIcon => "icons/quick-menus.svg",
        }
    }
}

pub struct Icon {
    svg: Svg,
    name: IconName,
    motion: Option<&'static MotionSpec>,
    id: ElementId,
    motion_enabled: bool,
    explicit_transformation: bool,
}

impl Icon {
    pub fn motion_scope(element: impl IntoElement, enabled: bool) -> crate::IconMotionScope {
        crate::IconMotionScope::new(element, enabled)
    }

    #[track_caller]
    pub fn new(name: IconName) -> Self {
        let motion = MotionSpec::for_path(name.path()).filter(|spec| spec.has_hover_motion());
        Self {
            svg: svg().path(name.path()).size(px(16.)).flex_none(),
            name,
            motion,
            id: ElementId::CodeLocation(*std::panic::Location::caller()),
            motion_enabled: true,
            explicit_transformation: false,
        }
    }

    pub fn motion_enabled(mut self, enabled: bool) -> Self {
        self.motion_enabled = enabled;
        self
    }

    pub fn with_transformation(mut self, transformation: Transformation) -> Self {
        self.svg = self.svg.with_transformation(transformation);
        self.explicit_transformation = true;
        self
    }

    fn resolved_base_color(&mut self, window: &Window) -> gpui::Hsla {
        self.svg
            .style()
            .text
            .color
            .unwrap_or_else(|| window.text_style().color)
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
        Element::id(&self.svg).or_else(|| self.supports_hover_motion().then(|| self.id.clone()))
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
            self.supports_hover_motion()
                .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal))
        })
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut paint_bounds = bounds;
        if let Some(global_id) = global_id {
            let enabled = self.supports_hover_motion()
                && window.is_window_active()
                && crate::icon_motion::IconMotionPreferences::enabled(cx);
            let hovered = enabled
                && crate::icon_motion::scoped_hover(cx)
                    .unwrap_or_else(|| hitbox.as_ref().is_some_and(|h| h.is_hovered(window)));
            let now = std::time::Instant::now();
            let frame =
                window.with_element_state(global_id, |playback: Option<MotionPlayback>, _| {
                    let mut playback = playback.unwrap_or_default();
                    let frame = playback.frame(self.name.path(), self.motion, hovered, now);
                    (frame, playback)
                });
            if let Some(frame) = frame {
                if self.explicit_transformation {
                    paint_bounds = frame.translated_bounds(bounds);
                } else {
                    let replacement = svg();
                    let inner = std::mem::replace(&mut self.svg, replacement);
                    self.svg = inner.with_transformation(frame.transformation(bounds));
                }
                if frame.running {
                    window.request_animation_frame();
                }
            }
        }
        let explicit_color = self.svg.style().text.color;
        self.svg.style().text.color = Some(self.resolved_base_color(window));
        self.svg.paint(
            global_id,
            inspector_id,
            paint_bounds,
            state,
            hitbox,
            window,
            cx,
        );
        self.svg.style().text.color = explicit_color;
        if self.supports_hover_motion()
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
                    Icon::new(IconName::SelectFileIcon)
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
            fn render(mut self, window: &mut Window, _: &mut App) -> impl IntoElement {
                self.observed
                    .set(Some(self.icon.resolved_base_color(window)));
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
                        icon: Icon::new(IconName::IconFriends),
                        observed: self.inherited.clone(),
                    })
                    .child(TintProbe {
                        icon: Icon::new(IconName::IconFriends).text_color(self.explicit_color),
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
