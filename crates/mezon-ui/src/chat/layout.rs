use std::collections::HashMap;

use crate::chat::channel_app_bar::ChannelAppBarTarget;
use gpui::{
    AnyView, App, Context, DismissEvent, Entity, FocusHandle, Focusable, Pixels, ScrollHandle,
    Size, StyleRefinement, Subscription, Task, Window, deferred, div, linear_color_stop,
    linear_gradient, prelude::*, px, relative,
};
use mezon_store::{
    AuthState, AutoUpdateStatus, AutoUpdateStore, CHANNEL_ACTIVE_ARCHIVED, CHANNEL_ACTIVE_JOINED,
    Channel, ChannelEvent, ChannelId, ChannelList, ChannelType, ClanId, ClanList, ClanMembersStore,
    DirectChannel, DirectKind, DirectMessageStore, GroupMembersStore, InboxStore,
    MessageSearchEvent, MessageSearchStore, MessagesStore, PinnedEvent, PinnedMessagesStore,
    Settings, StreamStore, THREAD_STATUS_ARCHIVED, ThreadsEvent, ThreadsStore, TopicsEvent,
    TopicsStore, UiState, VoiceConnection, VoiceMember, VoiceModerationError, VoiceStore,
    expand_mention_name_tokens,
};
use ui::PopoverMenuHandle;

use crate::app::shell::Shell;
use crate::chat::area::ChatArea;
use crate::chat::inbox::{InboxPopoverPanel, clan_has_inbox_badge};
use crate::chat::message::{ReactionPicker, ReactionPickerEvent};
use crate::chat::message_search::{
    MessageSearchPanel, apply_search_dropdown_item, register_chat_layout,
};
use crate::chat::pinned_popover::PinnedPopoverPanel;
use crate::chat::threads_popover::ThreadsPopoverPanel;
use crate::chat::voice_sound_picker::{VoiceSoundPicker, VoiceSoundPickerEvent};
use crate::chat::{CanvasPopoverPanel, CanvasView};
use crate::components::compositions::channel_row::channel_icon;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::components::primitives::{
    Icon, IconName, InputEvent, InputState, Slider, SliderEvent, SliderState,
};
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};
use crate::{ChannelSidebar, ClanSidebar, DirectSidebar};

pub struct ChatLayout {
    pub(crate) channel_list: Entity<ChannelList>,
    pub chat_area: ChatArea,
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
    direct_sidebar: Entity<DirectSidebar>,
    friends_page: Entity<crate::chat::FriendsPage>,
    clan_members_page: Entity<crate::chat::clan_members_page::ClanMembersPage>,
    clan_channels_page: Entity<crate::chat::clan_channels_page::ClanChannelsPage>,
    direct_store: Entity<DirectMessageStore>,
    user_info_bar: Entity<UserInfoBar>,
    clan_list: Entity<ClanList>,
    auth_state: Entity<AuthState>,
    settings: Entity<Settings>,
    voice_store: Entity<VoiceStore>,
    stream_store: Entity<StreamStore>,
    voice_strip_scroll: ScrollHandle,
    voice_strip_width: Pixels,
    voice_grid_page: usize,
    voice_grid_wheel_accum: f32,
    voice_grid_size: Size<Pixels>,
    voice_show_members: bool,
    voice_show_chat: bool,
    voice_session_key: Option<String>,
    voice_visual: crate::chat::voice::VoiceVisualState,
    displayed_stream_joined: bool,
    displayed_stream_connecting: bool,
    displayed_stream_fullscreen: bool,
    stream_fullscreen_focus: FocusHandle,
    stream_fullscreen_focused: bool,
    pending_channel_id: Option<ChannelId>,
    prefetched_voice_channel: Option<ChannelId>,
    dm_view_fingerprint: Option<(ChannelId, DirectKind, String)>,
    inbox_context_ids: Option<(Option<ClanId>, Option<ChannelId>)>,
    _voice_frame_pump: Option<Task<()>>,
    _stream_frame_pump: Option<Task<()>>,
    show_member_list: bool,
    ui_state: UiState,
    media_channel_view_mode: bool,
    message_search_expanded: bool,
    show_search_options: bool,
    show_results_panel: bool,
    member_list_before_search: Option<bool>,
    message_search_panel: Option<Entity<MessageSearchPanel>>,
    message_search_input: Option<Entity<InputState>>,
    message_search_context: Option<(ChannelId, ClanId, bool)>,
    search_dropdown_index: usize,
    search_mention_ids: HashMap<String, String>,
    _message_search_input_sub: Option<Subscription>,
    inbox_handle: PopoverMenuHandle<InboxPopoverPanel>,
    pub(crate) thread_popover_handle: PopoverMenuHandle<ThreadsPopoverPanel>,
    pub(crate) thread_search_input: Option<Entity<InputState>>,
    pub(crate) canvas_search_input: Option<Entity<InputState>>,
    thread_name_input: Option<Entity<InputState>>,
    create_thread_message_input: Option<Entity<InputState>>,
    topic_panel: Option<Entity<crate::chat::create_topic_panel::TopicPanel>>,
    pin_popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    canvas_popover_handle: PopoverMenuHandle<CanvasPopoverPanel>,
    canvas_view: Option<Entity<CanvasView>>,
    displayed_active_channel: Option<ActiveChannelSlice>,
    focused_channel_id: Option<ChannelId>,
    displayed_voice_mini: Option<VoiceMiniSlice>,
    displayed_threads_panel: ThreadsPanelSlice,
    threads_creating_gate: bool,
    displayed_inbox: InboxDisplaySlice,
    last_route: Route,
    pending_open_threads_popover: bool,
    pending_open_pin_popover: bool,
    voice_emoji_picker: Option<Entity<ReactionPicker>>,
    _voice_emoji_picker_sub: Option<Subscription>,
    _voice_emoji_picker_dismiss_sub: Option<Subscription>,
    voice_sound_picker: Option<Entity<VoiceSoundPicker>>,
    _voice_sound_picker_sub: Option<Subscription>,
    _voice_sound_picker_dismiss_sub: Option<Subscription>,
    ns_slider: Entity<SliderState>,
    _ns_slider_sub: Subscription,
    stream_volume_slider: Entity<SliderState>,
    _stream_volume_slider_sub: Subscription,
    ns_popover_open: bool,
    ns_hovered: bool,
    ns_dragging: bool,
    _ns_popover_close: Option<Task<()>>,
}

#[derive(Default, PartialEq, Eq)]
struct ThreadsPanelSlice {
    creating: bool,
    submitting: bool,
    create_private: bool,
    name_error: Option<String>,
}

#[derive(Default, PartialEq, Eq)]
struct InboxDisplaySlice {
    clan_id: Option<String>,
    has_badge: bool,
}

struct ActiveChannelSlice {
    id: ChannelId,
    name: String,
    channel_type: ChannelType,
    avatar_url: String,
    voice_members: Vec<VoiceMember>,
}

impl ActiveChannelSlice {
    fn from_channel(channel: &Channel) -> Self {
        Self {
            id: channel.id,
            name: channel.name.clone(),
            channel_type: channel.channel_type,
            avatar_url: channel.avatar_url.clone(),
            voice_members: channel.voice_members.clone(),
        }
    }

    fn differs_from(&self, channel: &Channel) -> bool {
        self.id != channel.id
            || self.channel_type != channel.channel_type
            || self.name != channel.name
            || self.avatar_url != channel.avatar_url
            || self.voice_members != channel.voice_members
    }
}

#[derive(PartialEq, Eq)]
struct VoiceMiniSlice {
    channel_id: String,
    clan_id: String,
    label: String,
    clan_name: String,
    mic_enabled: bool,
    camera_enabled: bool,
    screen_enabled: bool,
    link_copied: bool,
    noise_suppression_enabled: bool,
    noise_suppression_level: u8,
}

impl ChatLayout {
    pub fn new(
        clan_list: Entity<ClanList>,
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        let channel_list = ChannelList::global(cx);

        let clan_list_for_sidebar = clan_list.clone();
        let settings_for_clan = settings.clone();
        let clan_sidebar =
            cx.new(move |cx| ClanSidebar::new(clan_list_for_sidebar, settings_for_clan, cx));

        let clan_list_for_channel = clan_list.clone();
        let channel_list_for_channel = channel_list.clone();
        let settings_for_channel = settings.clone();
        let channel_sidebar = cx.new(move |cx| {
            ChannelSidebar::new(
                clan_list_for_channel,
                channel_list_for_channel,
                settings_for_channel,
                cx,
            )
        });

        let settings_for_direct = settings.clone();
        let direct_sidebar = cx.new(move |cx| DirectSidebar::new(settings_for_direct, cx));

        let settings_for_friends = settings.clone();
        let friends_page =
            cx.new(move |cx| crate::chat::FriendsPage::new(settings_for_friends, cx));
        let members_settings = settings.clone();
        let clan_members_page = cx.new(move |cx| {
            crate::chat::clan_members_page::ClanMembersPage::new(members_settings, cx)
        });

        let user_info_bar = cx.new(|cx| UserInfoBar::new(auth_state.clone(), cx));
        let channels_settings = settings.clone();
        let clan_channels_page = cx.new(move |cx| {
            crate::chat::clan_channels_page::ClanChannelsPage::new(channels_settings, cx)
        });

        let direct_store = DirectMessageStore::global(cx);

        cx.observe(&direct_store, |this, store, cx| {
            let Route::DirectMessage { direct_id, .. } = Router::global(cx).read(cx).route() else {
                return;
            };
            let fingerprint = store
                .read(cx)
                .find(direct_id)
                .map(|dm| (dm.id, dm.kind, dm.label.clone()));
            if this.dm_view_fingerprint == fingerprint {
                return;
            }
            this.dm_view_fingerprint = fingerprint;
            cx.notify();
        })
        .detach();

        if let Some(update_store) = AutoUpdateStore::try_global(cx) {
            cx.observe(&update_store, |_, _, cx| cx.notify()).detach();
        }
        if let Some(update_store) = mezon_store::WinstoreUpdateStore::try_global(cx) {
            cx.observe(&update_store, |_, _, cx| cx.notify()).detach();
        }

        let voice_store = VoiceStore::global(cx);
        cx.observe(&voice_store, |this, voice, cx| {
            if let Some(err) = voice.update(cx, |store, _| store.take_moderation_error()) {
                let locale = this.settings.read(cx).language.clone();
                let key = match err {
                    VoiceModerationError::MuteFailed => "channelVoice.muteMemberFailed",
                    VoiceModerationError::KickFailed => "channelVoice.kickMemberFailed",
                    VoiceModerationError::AgentFailed => "channelVoice.agentActionFailed",
                };
                let msg = mezon_i18n::t(&locale, key).to_string();
                Shell::global(cx).update(cx, |shell, cx| shell.error(msg, cx));
            }
            let mini_changed = this.voice_mini_display_changed(cx);
            this.sync_voice_frame_pump(cx);
            this.sync_stream_frame_pump(cx);
            if mini_changed || this.is_voice_frame_relevant(cx) {
                cx.notify();
            }
        })
        .detach();

        let stream_store = StreamStore::global(cx);
        cx.observe(&stream_store, |this, store, cx| {
            let store = store.read(cx);
            let joined = store.is_joined();
            let joining = store.is_joining();
            let fullscreen = store.fullscreen();
            this.displayed_stream_joined = joined;
            this.displayed_stream_connecting = joining;
            this.displayed_stream_fullscreen = fullscreen;
            if !fullscreen {
                this.stream_fullscreen_focused = false;
            }
            this.sync_stream_frame_pump(cx);
            cx.notify();
        })
        .detach();

        let threads_store = ThreadsStore::global(cx);
        cx.observe(&threads_store, |this, _, cx| {
            if this.threads_panel_state_changed(cx) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&threads_store, |this, _, event, cx| {
            this.on_threads_event(event, cx);
        })
        .detach();

        cx.subscribe(&PinnedMessagesStore::global(cx), |this, _, event, cx| {
            this.on_pinned_event(event, cx);
        })
        .detach();

        cx.subscribe(
            &MessageSearchStore::global(cx),
            |this, _, event: &MessageSearchEvent, cx| {
                if *event == MessageSearchEvent::SearchFailed {
                    let locale = this.settings.read(cx).language.clone();
                    let msg = mezon_i18n::t(&locale, "searchMessageChannel.searchFailed");
                    Shell::global(cx).update(cx, |shell, cx| shell.error(msg, cx));
                }
            },
        )
        .detach();

        cx.subscribe(
            &mezon_store::ClanMembersStore::global(cx),
            |this, _, event: &mezon_store::ClanMembersEvent, cx| {
                let clan_id = event.clan_id();
                if this.visible_media_clan_id(cx) == Some(clan_id) {
                    cx.notify();
                }
            },
        )
        .detach();

        let chat_area = ChatArea::new(settings.clone(), cx);
        cx.observe(&channel_list, |this, _, cx| {
            this.apply_pending_channel(cx);
            this.redirect_archived_thread_route(cx);
            this.ensure_active_channel_for_clan(cx);
            this.sync_inbox_context(cx);
            this.sync_stream_session(cx);
            this.sync_voice_frame_pump(cx);
            this.sync_stream_frame_pump(cx);
            if this.active_channel_display_changed(cx) {
                this.media_channel_view_mode = false;
                this.dismiss_topic_panel(cx);
                this.dismiss_threads_popover(cx);
                this.pin_popover_handle.hide(cx);
                this.canvas_popover_handle.hide(cx);
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&channel_list, |this, _, event, cx| {
            let ChannelEvent::ArchivedByAdministrator { is_thread } = event else {
                return;
            };
            let locale = this.settings.read(cx).language.clone();
            let key = if *is_thread {
                "channelMenu.toastArchivedThreadByAdministrator"
            } else {
                "channelMenu.toastArchivedByAdministrator"
            };
            Shell::global(cx).update(cx, |shell, cx| {
                shell.success(mezon_i18n::t(&locale, key).to_string(), cx);
            });
            this.redirect_archived_thread_route(cx);
            this.ensure_active_channel_for_clan(cx);
        })
        .detach();
        cx.observe(&Router::global(cx), |this, _, cx| {
            let next_route = Router::global(cx).read(cx).route().clone();
            if next_route != this.last_route {
                match &this.last_route {
                    Route::ClanMembers { .. } => this
                        .clan_members_page
                        .update(cx, |page, cx| page.reset_search(cx)),
                    Route::ClanChannels { .. } => this
                        .clan_channels_page
                        .update(cx, |page, cx| page.deactivate(cx)),
                    _ => {}
                }
                this.last_route = next_route.clone();
            }
            if matches!(
                next_route,
                Route::Direct | Route::Friends | Route::DirectMessage { .. }
            ) {
                this.media_channel_view_mode = false;
                this.dismiss_topic_panel(cx);
                this.dismiss_threads_popover(cx);
                this.pin_popover_handle.hide(cx);
                this.canvas_popover_handle.hide(cx);
            }
            this.reset_message_search(cx);
            this.sync_active_from_route(cx);
            this.redirect_archived_thread_route(cx);
            this.ensure_active_channel_for_clan(cx);
            this.sync_stream_session(cx);
            this.sync_voice_frame_pump(cx);
            this.sync_stream_frame_pump(cx);
            this.dismiss_inbox_popover(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&MessageSearchStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(&TopicsStore::global(cx), |this, _, event, cx| match event {
            TopicsEvent::Opened => {
                ThreadsStore::global(cx).update(cx, |threads, cx| threads.cancel_create(cx));
                this.reset_message_search(cx);
                cx.notify();
            }
            TopicsEvent::Closed => cx.notify(),
            TopicsEvent::Updated | TopicsEvent::ReplySent | TopicsEvent::ReplyTargetChanged => {}
        })
        .detach();
        cx.observe(&ThreadsStore::global(cx), |this, store, cx| {
            let creating = store.read(cx).is_creating();
            if creating == this.threads_creating_gate {
                return;
            }
            this.threads_creating_gate = creating;
            if creating {
                TopicsStore::global(cx).update(cx, |topics, cx| topics.close_panel(cx));
            }
        })
        .detach();
        cx.observe(&clan_list, |this, _, cx| {
            this.sync_inbox_context(cx);
            if this.inbox_display_changed(cx) {
                cx.notify();
            }
        })
        .detach();
        let ns_level = voice_store.read(cx).noise_suppression_level();
        let ns_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(ns_level as f32)
        });
        let ns_slider_sub = cx.subscribe(
            &ns_slider,
            |_this, _slider: Entity<SliderState>, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event;
                let level = value.end().round().clamp(0., 100.) as u8;
                VoiceStore::global(cx).update(cx, |store, cx| {
                    store.set_noise_suppression_level(level, cx);
                });
            },
        );
        let stream_volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(1.)
        });
        let stream_volume_slider_sub = cx.subscribe(
            &stream_volume_slider,
            |_this, _slider: Entity<SliderState>, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event;
                StreamStore::global(cx).update(cx, |store, cx| {
                    store.set_volume(value.end().clamp(0., 1.), cx);
                });
            },
        );
        let ui_state = UiState::load_sync();
        let show_member_list = ui_state.show_member_list;
        let mut this = Self {
            channel_list,
            clan_sidebar,
            channel_sidebar,
            direct_sidebar,
            friends_page,
            clan_members_page,
            clan_channels_page,
            direct_store,
            user_info_bar,
            clan_list,
            auth_state,
            chat_area,
            settings,
            voice_store,
            stream_store,
            voice_strip_scroll: ScrollHandle::new(),
            voice_strip_width: px(0.),
            voice_grid_page: 0,
            voice_grid_wheel_accum: 0.,
            voice_grid_size: Size::default(),
            voice_show_members: true,
            voice_show_chat: false,
            voice_session_key: None,
            voice_visual: Default::default(),
            displayed_stream_joined: false,
            displayed_stream_connecting: false,
            displayed_stream_fullscreen: false,
            stream_fullscreen_focus: cx.focus_handle(),
            stream_fullscreen_focused: false,
            pending_channel_id: None,
            prefetched_voice_channel: None,
            dm_view_fingerprint: None,
            inbox_context_ids: None,
            _voice_frame_pump: None,
            _stream_frame_pump: None,
            show_member_list,
            ui_state,
            media_channel_view_mode: false,
            message_search_expanded: false,
            show_search_options: false,
            show_results_panel: false,
            member_list_before_search: None,
            message_search_panel: None,
            message_search_input: None,
            message_search_context: None,
            search_dropdown_index: 0,
            search_mention_ids: HashMap::new(),
            _message_search_input_sub: None,
            inbox_handle: PopoverMenuHandle::default(),
            thread_popover_handle: PopoverMenuHandle::default(),
            thread_search_input: None,
            canvas_search_input: None,
            thread_name_input: None,
            create_thread_message_input: None,
            topic_panel: None,
            pin_popover_handle: PopoverMenuHandle::default(),
            canvas_popover_handle: PopoverMenuHandle::default(),
            canvas_view: None,
            displayed_active_channel: None,
            focused_channel_id: None,
            displayed_voice_mini: None,
            displayed_threads_panel: ThreadsPanelSlice::default(),
            threads_creating_gate: false,
            displayed_inbox: InboxDisplaySlice::default(),
            last_route: Router::global(cx).read(cx).route().clone(),
            pending_open_threads_popover: false,
            pending_open_pin_popover: false,
            voice_emoji_picker: None,
            _voice_emoji_picker_sub: None,
            _voice_emoji_picker_dismiss_sub: None,
            voice_sound_picker: None,
            _voice_sound_picker_sub: None,
            _voice_sound_picker_dismiss_sub: None,
            ns_slider,
            _ns_slider_sub: ns_slider_sub,
            stream_volume_slider,
            _stream_volume_slider_sub: stream_volume_slider_sub,
            ns_popover_open: false,
            ns_hovered: false,
            ns_dragging: false,
            _ns_popover_close: None,
        };
        this.sync_active_from_route(cx);
        this.sync_member_list_visibility(cx);
        this.sync_inbox_context(cx);
        this.sync_voice_frame_pump(cx);
        this.sync_stream_frame_pump(cx);
        register_chat_layout(cx.weak_entity(), cx);
        this
    }

    pub(crate) fn search_dropdown_index(&self) -> usize {
        self.search_dropdown_index
    }

    fn reset_search_dropdown_index(&mut self, cx: &mut Context<Self>) {
        if self.search_dropdown_index != 0 {
            self.search_dropdown_index = 0;
            cx.notify();
        }
    }

    fn move_search_dropdown_index(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(input) = self.message_search_input.as_ref() else {
            return;
        };
        let query = input.read(cx).value().to_string();
        let count = crate::chat::message_search::search_dropdown_item_count(&query, cx);
        if count == 0 {
            return;
        }
        let next =
            (self.search_dropdown_index as isize + delta).rem_euclid(count as isize) as usize;
        if next != self.search_dropdown_index {
            self.search_dropdown_index = next;
            cx.notify();
        }
    }

    pub(crate) fn select_next_search_dropdown_item(&mut self, cx: &mut Context<Self>) {
        if self.show_search_options {
            self.move_search_dropdown_index(1, cx);
        }
    }

    pub(crate) fn select_prev_search_dropdown_item(&mut self, cx: &mut Context<Self>) {
        if self.show_search_options {
            self.move_search_dropdown_index(-1, cx);
        }
    }

    pub(crate) fn try_activate_search_dropdown_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.show_search_options {
            return false;
        }
        let Some(input) = self.message_search_input.as_ref() else {
            return false;
        };
        let query = input.read(cx).value().to_string();
        let mode = mezon_store::search_dropdown_mode(&query);
        if matches!(
            mode,
            mezon_store::SearchDropdownMode::FromUser
                | mezon_store::SearchDropdownMode::Mentions
                | mezon_store::SearchDropdownMode::Has
        ) && mezon_store::autocomplete_needle(&query).is_empty()
        {
            return false;
        }
        let items = crate::chat::message_search::search_dropdown_items(&query, cx);
        if items.is_empty() {
            return false;
        }
        let index = self.search_dropdown_index.min(items.len() - 1);
        apply_search_dropdown_item(self, &items[index], window, cx);
        true
    }

    pub(crate) fn try_finalize_incomplete_search_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(input) = self.message_search_input.clone() else {
            return false;
        };
        let query = input.read(cx).value().to_string();
        let Some(next) = mezon_store::finalize_incomplete_filter_token(&query) else {
            return false;
        };
        if next == query {
            return false;
        }
        input.update(cx, |input, cx| input.set_value(&next, window, cx));
        self.message_search_expanded = true;
        self.show_search_options =
            crate::chat::message_search::should_show_message_search_dropdown(&next);
        self.reset_search_dropdown_index(cx);
        self.sync_search_filter_highlights(cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
        true
    }

    fn sync_search_filter_highlights(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.message_search_input.clone() else {
            return;
        };
        let query = input.read(cx).value().to_string();
        let ranges = mezon_store::search_filter_chip_ranges(&query);
        let color = cx.theme().tokens.bg_item_hover.into();
        input.update(cx, |input, cx| {
            if ranges.is_empty() {
                input.clear_token_backgrounds(cx);
            } else {
                input.set_token_backgrounds(ranges, color, cx);
            }
        });
    }

    pub(crate) fn expand_message_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((channel_id, clan_id, is_direct)) =
            crate::chat::message_search::message_search_available(cx)
        else {
            return;
        };
        self.message_search_context = Some((channel_id, clan_id, is_direct));
        self.ensure_message_search_input(window, cx);
        if self.member_list_before_search.is_none() {
            self.member_list_before_search = Some(self.show_member_list);
        }
        self.message_search_expanded = true;
        let query = self
            .message_search_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.show_search_options =
            crate::chat::message_search::should_show_message_search_dropdown(&query);
        self.reset_search_dropdown_index(cx);
        if let Some(input) = self.message_search_input.clone() {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    pub(crate) fn dismiss_message_search_options(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.message_search_expanded {
            return;
        }
        let query = self
            .message_search_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.show_search_options = false;
        let collapsing = query.is_empty() && !self.show_results_panel;
        if collapsing {
            self.message_search_expanded = false;
            self.blur_message_search(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn execute_message_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((channel_id, clan_id, is_direct)) = self.message_search_context else {
            return;
        };
        self.ensure_message_search_input(window, cx);
        let Some(input) = self.message_search_input.clone() else {
            return;
        };
        let query = input.read(cx).value().to_string();
        if query.trim().is_empty() {
            return;
        }
        let mention_ids = self.search_mention_ids.clone();
        let query = expand_mention_name_tokens(&query, |name| {
            let key = name.to_lowercase();
            if let Some(id) = mention_ids.get(&key) {
                return Some(id.clone());
            }
            resolve_mention_name_to_user_id(name, clan_id, is_direct, channel_id, cx)
        });
        self.show_search_options = false;
        self.show_results_panel = true;
        self.message_search_expanded = true;
        self.show_member_list = false;
        let locale = self.settings.read(cx).language.clone().into();
        let layout = cx.weak_entity();
        MessageSearchStore::global(cx).update(cx, |store, cx| {
            store.search(channel_id, clan_id, is_direct, query, cx);
        });
        let recreate = self.message_search_panel.is_none();
        if recreate {
            self.message_search_panel = Some(cx.new(|cx| {
                MessageSearchPanel::new(layout, channel_id, clan_id, is_direct, locale, cx)
            }));
        }
        cx.notify();
    }

    pub(crate) fn close_results_panel(&mut self, cx: &mut Context<Self>) {
        if !self.show_results_panel {
            return;
        }
        if let Some((channel_id, _, _)) = self.message_search_context {
            MessageSearchStore::global(cx).update(cx, |store, cx| {
                store.clear_channel(channel_id, cx);
            });
        }
        self.show_results_panel = false;
        if let Some(was) = self.member_list_before_search.take() {
            self.show_member_list = was;
        }
        self.message_search_panel = None;
        cx.notify();
    }

    pub(crate) fn reset_message_search(&mut self, cx: &mut Context<Self>) {
        if let Some((channel_id, _, _)) = self.message_search_context {
            MessageSearchStore::global(cx).update(cx, |store, cx| {
                store.clear_channel(channel_id, cx);
            });
        }
        self.message_search_expanded = false;
        self.show_search_options = false;
        self.show_results_panel = false;
        if let Some(was) = self.member_list_before_search.take() {
            self.show_member_list = was;
        }
        self.message_search_panel = None;
        self.message_search_context = None;
        self.search_mention_ids.clear();
        cx.notify();
    }

    fn blur_message_search(&self, window: &mut Window, cx: &App) {
        let Some(input) = &self.message_search_input else {
            return;
        };
        if input.read(cx).focus_handle(cx).is_focused(window) {
            window.blur();
        }
    }

    pub(crate) fn insert_search_option_prefix(
        &mut self,
        prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.message_search_input.clone() else {
            return;
        };
        let current = input.read(cx).value().to_string();
        let next = if current.is_empty() {
            prefix.to_string()
        } else {
            format!("{current}{prefix}")
        };
        input.update(cx, |input, cx| input.set_value(&next, window, cx));
        self.message_search_expanded = true;
        self.show_search_options =
            crate::chat::message_search::should_show_message_search_dropdown(&next);
        self.reset_search_dropdown_index(cx);
        self.sync_search_filter_highlights(cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn insert_search_filter_markup(
        &mut self,
        trigger: char,
        display: &str,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.message_search_input.clone() else {
            return;
        };
        if trigger == '~' && !display.is_empty() && !id.is_empty() {
            self.search_mention_ids
                .insert(display.to_lowercase(), id.to_string());
        }
        let current = input.read(cx).value().to_string();
        let next = mezon_store::insert_filter_markup(&current, trigger, display, id);
        input.update(cx, |input, cx| input.set_value(&next, window, cx));
        self.message_search_expanded = true;
        self.show_search_options =
            crate::chat::message_search::should_show_message_search_dropdown(&next);
        self.reset_search_dropdown_index(cx);
        self.sync_search_filter_highlights(cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn on_search_input_cleared(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_results_panel {
            self.close_results_panel(cx);
            self.message_search_expanded = false;
            self.show_search_options = false;
            self.search_mention_ids.clear();
            self.blur_message_search(window, cx);
        } else {
            self.show_search_options = false;
            if let Some(input) = self.message_search_input.clone() {
                input.update(cx, |input, cx| input.focus(window, cx));
            }
        }
        cx.notify();
    }

    fn ensure_message_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.message_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "searchMessageChannel.searchPlaceholder");
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .embedded(true)
                .filter_token_chips(true)
                .padding_right(px(4.))
        });
        let input_for_sub = input.clone();
        let input_sub = cx.subscribe_in(
            &input,
            window,
            move |this: &mut ChatLayout, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter => {
                    if this.show_search_options
                        && this.try_activate_search_dropdown_item(window, cx)
                    {
                        return;
                    }
                    if this.try_finalize_incomplete_search_filter(window, cx) {
                        return;
                    }
                    this.execute_message_search(window, cx);
                }
                InputEvent::Change => {
                    let query = input_for_sub.read(cx).value().to_string();
                    this.show_search_options = this.message_search_expanded
                        && crate::chat::message_search::should_show_message_search_dropdown(&query);
                    this.reset_search_dropdown_index(cx);
                    this.sync_search_filter_highlights(cx);
                    cx.notify();
                }
            },
        );
        self._message_search_input_sub = Some(input_sub);
        self.message_search_input = Some(input);
    }

    pub(crate) fn toggle_member_list(&mut self, cx: &mut Context<Self>) {
        let dm = self.is_dm_route(cx);
        self.show_member_list = !self.show_member_list;
        if dm {
            self.ui_state.show_member_list_dm = self.show_member_list;
        } else {
            self.ui_state.show_member_list = self.show_member_list;
        }
        self.persist_ui_state(cx);
        cx.notify();
    }

    fn persist_ui_state(&self, cx: &mut Context<Self>) {
        let snapshot = self.ui_state.clone();
        cx.background_executor()
            .spawn(async move {
                snapshot.save_sync();
            })
            .detach();
    }

    pub(crate) fn toggle_media_channel_view(&mut self, cx: &mut Context<Self>) {
        self.media_channel_view_mode = !self.media_channel_view_mode;
        if self.media_channel_view_mode {
            self.show_member_list = false;
        } else {
            self.sync_member_list_visibility(cx);
        }
        cx.notify();
    }

    fn sync_member_list_visibility(&mut self, cx: &Context<Self>) {
        self.show_member_list = if self.is_dm_route(cx) {
            self.ui_state.show_member_list_dm
        } else {
            self.ui_state.show_member_list
        };
    }

    fn channel_supports_timeline_view(&self, cx: &Context<Self>) -> bool {
        if self.is_dm_route(cx) {
            return false;
        }
        self.channel_list
            .read(cx)
            .active_channel()
            .is_some_and(|ch| {
                matches!(
                    ch.channel_type,
                    ChannelType::Text | ChannelType::Thread | ChannelType::App
                )
            })
    }

    fn dismiss_inbox_popover(&self, cx: &mut App) {
        self.inbox_handle.hide(cx);
    }

    fn sync_inbox_context(&mut self, cx: &mut Context<Self>) {
        let clan = self.clan_list.read(cx).active_clan_id;
        let channel = self.channel_list.read(cx).active_channel_id;
        if self.inbox_context_ids == Some((clan, channel)) {
            return;
        }
        self.inbox_context_ids = Some((clan, channel));
        self.stream_store.update(cx, |store, cx| {
            store.set_active_clan(clan, cx);
        });
        let clan_id = clan.map(|id| id.to_string());
        let channel_id = channel.map(|id| id.to_string());
        InboxStore::global(cx).update(cx, |store, cx| {
            store.set_active_context(clan_id, channel_id, cx);
        });
    }

    fn active_clan_id(&self, cx: &Context<Self>) -> Option<String> {
        self.clan_list
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string())
    }

    fn sync_active_from_route(&mut self, cx: &mut Context<Self>) {
        if !matches!(Router::global(cx).read(cx).route(), Route::Canvas { .. }) {
            self.canvas_view = None;
        }
        match Router::global(cx).read(cx).route() {
            Route::Channel {
                clan_id,
                channel_id,
            } => self.sync_channel_route(clan_id, channel_id, cx),
            Route::Thread {
                clan_id,
                channel_id,
                ..
            } => self.sync_channel_route(clan_id, channel_id, cx),
            Route::Canvas {
                clan_id,
                channel_id,
                ..
            } => self.sync_channel_route(clan_id, channel_id, cx),
            Route::DirectMessage {
                direct_id,
                message_type,
            } => {
                self.pending_channel_id = None;
                let already_current =
                    self.direct_store.read(cx).current().map(|(id, _)| id) == Some(direct_id);
                if !already_current {
                    self.channel_list.update(cx, |channel_list, cx| {
                        channel_list.record_previous_channel(ClanId(0), direct_id, cx);
                    });
                }
                self.direct_store
                    .update(cx, |store, cx| store.ensure_loaded(cx));
                let channel_type = message_type.parse::<i32>().unwrap_or_else(|_| {
                    tracing::warn!(
                        "DM route: non-numeric message_type {:?}, defaulting to 3",
                        message_type
                    );
                    3
                });
                self.direct_store
                    .update(cx, |store, _| store.set_current(direct_id, channel_type));
                if channel_type == DirectKind::Group.channel_type() {
                    self.chat_area.bind_group_members(cx);
                    let group_id = direct_id;
                    GroupMembersStore::global(cx)
                        .update(cx, |store, cx| store.ensure_loaded(group_id, cx));
                } else {
                    self.chat_area.clear_member_panel();
                }
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.open_direct(direct_id, channel_type, cx)
                });
            }
            Route::Direct | Route::Friends => {
                self.pending_channel_id = None;
                self.direct_store
                    .update(cx, |store, cx| store.ensure_loaded(cx));
                self.chat_area.clear_member_panel();
                MessagesStore::global(cx).update(cx, |store, cx| store.close(cx));
            }
            Route::Chat => {
                self.pending_channel_id = None;
                self.chat_area.clear_member_panel();
                MessagesStore::global(cx).update(cx, |store, cx| store.close(cx));
            }
            _ => {
                self.pending_channel_id = None;
                self.chat_area.clear_member_panel();
            }
        }
        self.sync_member_list_visibility(cx);
    }

    fn sync_channel_route(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.chat_area.bind_channel_members(cx);
        if self.clan_list.read(cx).active_clan_id != Some(clan_id) {
            self.clan_list
                .update(cx, |clan_list, cx| clan_list.select_clan(clan_id, cx));
        }
        let (present, already_active) = {
            let channels = self.channel_list.read(cx);
            (
                channels.find_channel_in_active_clan(channel_id).is_some(),
                channels.active_channel_id == Some(channel_id),
            )
        };
        if present {
            self.pending_channel_id = None;
            if !already_active {
                self.channel_list.update(cx, |channel_list, cx| {
                    channel_list.record_previous_channel(clan_id, channel_id, cx);
                    channel_list.select_channel(channel_id, cx);
                });
            }
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.open_channel_in_clan(clan_id, channel_id, cx);
            });
        } else {
            self.pending_channel_id = Some(channel_id);
            self.channel_list.update(cx, |channel_list, cx| {
                channel_list.load_for_clan(clan_id, cx);
            });
        }
    }

    fn apply_pending_channel(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.pending_channel_id else {
            return;
        };
        let Some(clan_id) = self.clan_list.read(cx).active_clan_id else {
            return;
        };
        if self
            .channel_list
            .read(cx)
            .find_channel_in_active_clan(channel_id)
            .is_some()
        {
            self.pending_channel_id = None;
            self.channel_list.update(cx, |channel_list, cx| {
                if channel_list.active_channel_id != Some(channel_id) {
                    channel_list.record_previous_channel(clan_id, channel_id, cx);
                }
                channel_list.select_channel(channel_id, cx);
            });
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.open_channel_in_clan(clan_id, channel_id, cx);
            });
        }
    }

    fn redirect_archived_thread_route(&mut self, cx: &mut Context<Self>) {
        let Route::Thread {
            clan_id,
            channel_id,
            thread_id,
        } = Router::global(cx).read(cx).route()
        else {
            return;
        };
        {
            let channel_list = self.channel_list.read(cx);
            if !channel_list.is_clan_cache_loaded(clan_id) {
                return;
            }
            if channel_list.channel_in_clan(clan_id, thread_id) {
                return;
            }
            if channel_list.is_locally_archived(thread_id) {
                crate::router::replace(
                    cx,
                    Route::Channel {
                        clan_id,
                        channel_id,
                    },
                );
                return;
            }
        }
        let resolving = self.channel_list.update(cx, |store, cx| {
            store.ensure_channel_in_clan(clan_id, thread_id, cx)
        });
        if resolving {
            return;
        }
        crate::router::replace(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }

    fn ensure_active_channel_for_clan(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            Router::global(cx).read(cx).route(),
            Route::Chat | Route::Channel { .. }
        ) {
            return;
        }
        let Some(clan_id) = self.clan_list.read(cx).active_clan_id else {
            return;
        };

        if let Route::Channel {
            clan_id: route_clan,
            channel_id,
        } = Router::global(cx).read(cx).route()
            && route_clan == clan_id
        {
            if self
                .channel_list
                .read(cx)
                .channel_in_clan(clan_id, channel_id)
            {
                return;
            }
            let resolving = self.channel_list.update(cx, |store, cx| {
                store.ensure_channel_in_clan(clan_id, channel_id, cx)
            });
            if resolving {
                return;
            }
        }

        let welcome = self.clan_list.read(cx).welcome_channel_id(clan_id);
        let target = {
            let channels = self.channel_list.read(cx);
            channels
                .remembered_channel(clan_id)
                .filter(|c| channels.channel_in_clan(clan_id, *c))
                .or_else(|| welcome.filter(|w| channels.channel_in_clan(clan_id, *w)))
                .or_else(|| channels.default_channel_id(clan_id))
        };
        let Some(channel_id) = target else {
            return;
        };

        crate::router::navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }

    fn maybe_prefetch_voice_token(&mut self, cx: &mut Context<Self>) {
        let active_voice_channel = self
            .channel_list
            .read(cx)
            .active_channel()
            .filter(|ch| ch.channel_type == ChannelType::Voice)
            .map(|ch| ch.id);

        if active_voice_channel == self.prefetched_voice_channel {
            return;
        }
        self.prefetched_voice_channel = active_voice_channel;

        if let Some(channel_id) = active_voice_channel {
            self.voice_store.update(cx, |store, cx| {
                store.prefetch_meet_token(channel_id.to_string(), cx);
            });
        }
    }

    fn threads_panel_state_changed(&mut self, cx: &Context<Self>) -> bool {
        let store = ThreadsStore::global(cx);
        let store = store.read(cx);
        let next = ThreadsPanelSlice {
            creating: store.is_creating(),
            submitting: store.is_submitting(),
            create_private: store.create_private(),
            name_error: store.name_error().map(str::to_string),
        };
        if self.displayed_threads_panel == next {
            return false;
        }
        self.displayed_threads_panel = next;
        true
    }

    fn inbox_display_changed(&mut self, cx: &Context<Self>) -> bool {
        let clan_id = self
            .clan_list
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string());
        let has_badge = clan_id
            .as_deref()
            .is_some_and(|id| clan_has_inbox_badge(id, cx));
        let next = InboxDisplaySlice { clan_id, has_badge };
        if self.displayed_inbox == next {
            return false;
        }
        self.displayed_inbox = next;
        true
    }

    fn active_channel_display_changed(&mut self, cx: &Context<Self>) -> bool {
        let changed = {
            let channels = self.channel_list.read(cx);
            match (&self.displayed_active_channel, channels.active_channel()) {
                (None, None) => false,
                (Some(slice), Some(channel)) => slice.differs_from(channel),
                _ => true,
            }
        };
        if changed {
            let next = self
                .channel_list
                .read(cx)
                .active_channel()
                .map(ActiveChannelSlice::from_channel);
            self.displayed_active_channel = next;
        }
        changed
    }

    fn sync_composer_on_channel_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_channel_id = Router::global(cx).read(cx).conversation_channel_id();
        if active_channel_id == self.focused_channel_id {
            return;
        }
        self.focused_channel_id = active_channel_id;
        let Some(input) = self.chat_area.mention_input.clone() else {
            return;
        };
        input.update(cx, |input, cx| {
            input.bind_channel(active_channel_id, window, cx)
        });
        if active_channel_id.is_none() {
            return;
        }
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus_input(window, cx));
        });
    }

    fn sync_voice_frame_pump(&mut self, cx: &mut Context<Self>) {
        const VOICE_FRAME_FALLBACK: std::time::Duration = std::time::Duration::from_millis(200);
        let want_pump =
            self.is_voice_frame_relevant(cx) && self.voice_store.read(cx).has_active_video();
        if !want_pump {
            self._voice_frame_pump = None;
            return;
        }
        if self._voice_frame_pump.is_some() {
            return;
        }
        self._voice_frame_pump = Some(cx.spawn(async move |this, cx| {
            let mut last_seq = 0u64;
            loop {
                let store =
                    match this.update(cx, |_, cx| VoiceStore::global(cx).read(cx).frame_store()) {
                        Ok(store) => store,
                        Err(_) => break,
                    };
                let Some(store) = store else {
                    cx.background_executor().timer(VOICE_FRAME_FALLBACK).await;
                    continue;
                };
                let mut rx = store.frame_watch();
                loop {
                    let seq = store.publish_seq();
                    if seq != last_seq {
                        last_seq = seq;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                    let frame_published = {
                        let changed = std::pin::pin!(rx.changed());
                        let fallback =
                            std::pin::pin!(cx.background_executor().timer(VOICE_FRAME_FALLBACK));
                        matches!(
                            futures::future::select(changed, fallback).await,
                            futures::future::Either::Left((Ok(()), _))
                        )
                    };
                    if !frame_published {
                        break;
                    }
                }
            }
        }));
    }

    fn sync_stream_session(&mut self, cx: &mut Context<Self>) {
        let active = self
            .channel_list
            .read(cx)
            .active_channel()
            .map(|ch| (ch.channel_type, ch.id));
        self.stream_store.update(cx, |store, cx| {
            if store.should_leave_for_active_channel(active) {
                store.leave_stream(cx);
            }
            store.clear_error_on_active_channel_change(active, cx);
        });
    }

    fn sync_stream_frame_pump(&mut self, cx: &mut Context<Self>) {
        const STREAM_FRAME_FALLBACK: std::time::Duration = std::time::Duration::from_millis(200);
        let stream = self.stream_store.read(cx);
        let on_stream = self
            .channel_list
            .read(cx)
            .active_channel()
            .is_some_and(|ch| ch.channel_type == ChannelType::Stream);
        let want_pump =
            stream.is_joined() && stream.remote_video() && (on_stream || stream.fullscreen());
        if !want_pump {
            self._stream_frame_pump = None;
            return;
        }
        if self._stream_frame_pump.is_some() {
            return;
        }
        let frame_store = stream.frame_store().clone();
        self._stream_frame_pump = Some(cx.spawn(async move |this, cx| {
            let mut last_seq = 0u64;
            loop {
                let mut rx = frame_store.frame_watch();
                loop {
                    let seq = frame_store.publish_seq();
                    if seq != last_seq {
                        last_seq = seq;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                    let frame_published = {
                        let changed = std::pin::pin!(rx.changed());
                        let fallback =
                            std::pin::pin!(cx.background_executor().timer(STREAM_FRAME_FALLBACK));
                        matches!(
                            futures::future::select(changed, fallback).await,
                            futures::future::Either::Left((Ok(()), _))
                        )
                    };
                    if !frame_published {
                        break;
                    }
                }
            }
        }));
    }

    fn is_voice_frame_relevant(&self, cx: &Context<Self>) -> bool {
        if self.is_dm_route(cx) {
            return false;
        }
        let Some(active) = self.channel_list.read(cx).active_channel() else {
            return false;
        };
        if active.channel_type != ChannelType::Voice {
            return false;
        }
        match self.voice_store.read(cx).connection().active_channel_id() {
            Some(id) => active.id.to_string() == id,
            None => true,
        }
    }

    fn connected_call_is_active(&self, cx: &Context<Self>) -> bool {
        if self.is_dm_route(cx) {
            return false;
        }
        let Some((connected_id, _)) = self.voice_store.read(cx).connection().connected_channel()
        else {
            return false;
        };
        self.channel_list
            .read(cx)
            .active_channel()
            .is_some_and(|ch| {
                ch.channel_type == ChannelType::Voice && ch.id.to_string() == connected_id
            })
    }

    fn visible_media_clan_id(&self, cx: &Context<Self>) -> Option<ClanId> {
        if self.is_dm_route(cx) {
            return None;
        }
        self.channel_list
            .read(cx)
            .active_channel()
            .filter(|ch| {
                matches!(
                    ch.channel_type,
                    ChannelType::Voice | ChannelType::Stream | ChannelType::App
                )
            })
            .map(|ch| ch.clan_id)
    }

    fn voice_mini_display_changed(&mut self, cx: &Context<Self>) -> bool {
        let store = self.voice_store.read(cx);
        let Some((channel_id, clan_id)) = store.connection().connected_channel() else {
            return self.displayed_voice_mini.take().is_some();
        };

        if let Some(prev) = self.displayed_voice_mini.as_mut()
            && prev.channel_id == channel_id
            && prev.clan_id == clan_id
        {
            let label = store.channel_label();
            let mic_enabled = store.mic_enabled();
            let camera_enabled = store.camera_enabled();
            let screen_enabled = store.screen_share_enabled();
            let link_copied = store.link_copied();
            let noise_suppression_enabled = store.noise_suppression_enabled();
            let noise_suppression_level = store.noise_suppression_level();
            let changed = prev.label != label
                || prev.mic_enabled != mic_enabled
                || prev.camera_enabled != camera_enabled
                || prev.screen_enabled != screen_enabled
                || prev.link_copied != link_copied
                || prev.noise_suppression_enabled != noise_suppression_enabled
                || prev.noise_suppression_level != noise_suppression_level;
            if changed {
                if prev.label != label {
                    prev.label = label.to_string();
                }
                prev.mic_enabled = mic_enabled;
                prev.camera_enabled = camera_enabled;
                prev.screen_enabled = screen_enabled;
                prev.link_copied = link_copied;
                prev.noise_suppression_enabled = noise_suppression_enabled;
                prev.noise_suppression_level = noise_suppression_level;
            }
            return changed;
        }

        let clan_name = clan_id
            .parse::<ClanId>()
            .ok()
            .and_then(|cid| self.clan_list.read(cx).clan(cid).map(|c| c.name.clone()))
            .unwrap_or_default();
        self.displayed_voice_mini = Some(VoiceMiniSlice {
            channel_id: channel_id.to_string(),
            clan_id: clan_id.to_string(),
            label: store.channel_label().to_string(),
            clan_name,
            mic_enabled: store.mic_enabled(),
            camera_enabled: store.camera_enabled(),
            screen_enabled: store.screen_share_enabled(),
            link_copied: store.link_copied(),
            noise_suppression_enabled: store.noise_suppression_enabled(),
            noise_suppression_level: store.noise_suppression_level(),
        });
        true
    }
}

impl Render for ChatLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("ChatLayout");
        self.chat_area.ensure_input(window, cx);
        self.chat_area.bind_window(window, cx);
        self.sync_composer_on_channel_switch(window, cx);
        self.maybe_prefetch_voice_token(cx);
        self.voice_store
            .update(cx, |store, cx| store.flush_texture_drops(Some(window), cx));
        self.stream_store
            .update(cx, |store, cx| store.flush_texture_drops(Some(window), cx));

        if std::mem::take(&mut self.pending_open_threads_popover) {
            let handle = self.thread_popover_handle.clone();
            window.defer(cx, move |window, cx| handle.show(window, cx));
        }
        if std::mem::take(&mut self.pending_open_pin_popover) {
            let handle = self.pin_popover_handle.clone();
            window.defer(cx, move |window, cx| handle.show(window, cx));
        }

        if self.stream_store.read(cx).fullscreen() && !self.stream_fullscreen_focused {
            window.focus(&self.stream_fullscreen_focus, cx);
            self.stream_fullscreen_focused = true;
        }

        let nav_body = self.render_nav_body(cx);
        let locale = self.settings.read(cx).language.clone();
        let topic_panel = self.build_topic_panel(window, cx);
        let create_panel = if topic_panel.is_some() {
            None
        } else {
            self.build_create_thread_panel(&locale, window, cx)
        };
        let right_panel = topic_panel.or(create_panel);
        let chat_content = self.render_content(window, cx);
        let main_content = if let Some(panel) = right_panel {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .w_full()
                .min_w_0()
                .h_full()
                .min_h_0()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .min_h_0()
                        .overflow_hidden()
                        .child(chat_content),
                )
                .child(panel)
                .into_any_element()
        } else {
            chat_content
        };
        let voice_mini_bar = self.render_voice_mini_bar(cx);
        let stream_connected_bar = crate::chat::stream::render_stream_connected_bar(
            cx.theme(),
            self.stream_store.read(cx),
            self.stream_store.clone(),
            cx,
        );
        fn update_banner_bg(hover: bool) -> gpui::Background {
            if hover {
                linear_gradient(
                    90.,
                    linear_color_stop(gpui::rgb(0x7c3aed), 0.),
                    linear_color_stop(gpui::rgb(0xdb2777), 1.),
                )
            } else {
                linear_gradient(
                    90.,
                    linear_color_stop(gpui::rgb(0x8b5cf6), 0.),
                    linear_color_stop(gpui::rgb(0xec4899), 1.),
                )
            }
        }
        let update_pill = mezon_store::effective_update_status(cx)
            .filter(|status| matches!(status, AutoUpdateStatus::Updated { .. }))
            .map(|_| {
                let locale = self.settings.read(cx).language.clone();
                div()
                    .id("update-mezon-pill")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .mx_2()
                    .mt_3()
                    .mb_2()
                    .h(px(36.0))
                    .flex_none()
                    .rounded(px(8.0))
                    .bg(update_banner_bg(false))
                    .cursor_pointer()
                    .hover(|s| s.bg(update_banner_bg(true)))
                    .on_click(|_, _, cx| mezon_store::update_restart_clicked(cx))
                    .child(
                        Icon::new(IconName::ReloadIcon)
                            .size(px(16.0))
                            .text_color(gpui::white()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(gpui::white())
                            .child(mezon_i18n::t(&locale, "common.updateMezon").to_string()),
                    )
            });
        let update_available_pill = mezon_store::effective_update_status(cx)
            .and_then(|status| match status {
                AutoUpdateStatus::UpdateAvailable { version } => Some(version),
                _ => None,
            })
            .map(|version| {
                let locale = self.settings.read(cx).language.clone();
                div()
                    .id("update-available-pill")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .mx_2()
                    .mt_3()
                    .mb_2()
                    .h(px(36.0))
                    .flex_none()
                    .rounded(px(8.0))
                    .bg(update_banner_bg(false))
                    .cursor_pointer()
                    .hover(|s| s.bg(update_banner_bg(true)))
                    .on_click(|_, _, cx| mezon_store::update_available_clicked(cx))
                    .child(
                        Icon::new(IconName::Download)
                            .size(px(16.0))
                            .text_color(gpui::white()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(gpui::white())
                            .child(format!(
                                "{} (v{})",
                                mezon_i18n::t(&locale, "common.newUpdateAvailable"),
                                version
                            )),
                    )
            });
        let manual_install_pill = mezon_store::effective_update_status(cx)
            .and_then(|status| match status {
                AutoUpdateStatus::ManualInstall { version, .. } => Some(version),
                _ => None,
            })
            .map(|version| {
                let locale = self.settings.read(cx).language.clone();
                div()
                    .id("update-manual-install-pill")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .mx_2()
                    .mt_3()
                    .mb_2()
                    .h(px(36.0))
                    .flex_none()
                    .rounded(px(8.0))
                    .bg(update_banner_bg(false))
                    .cursor_pointer()
                    .hover(|s| s.bg(update_banner_bg(true)))
                    .on_click(|_, _, cx| {
                        mezon_store::update_manual_install_clicked(cx);
                        crate::router::navigate(cx, Route::SettingsAccount);
                    })
                    .child(
                        Icon::new(IconName::Download)
                            .size(px(16.0))
                            .text_color(gpui::white()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(gpui::white())
                            .child(format!(
                                "{} (v{})",
                                mezon_i18n::t(&locale, "setting.update.readyToInstall"),
                                version
                            )),
                    )
            });
        let fullscreen = if self.connected_call_is_active(cx) {
            let chat = cx.entity();
            crate::chat::voice::render_screen_fullscreen_overlay(
                cx.theme(),
                &locale,
                &self.voice_store,
                &self.settings,
                self.voice_store.read(cx),
                &chat,
                cx,
            )
        } else if self.stream_store.read(cx).fullscreen() {
            crate::chat::stream::render_stream_fullscreen_overlay(
                window,
                cx.theme(),
                self.stream_store.read(cx),
                self.stream_store.clone(),
                &self.stream_volume_slider,
                &self.stream_fullscreen_focus,
                cx,
            )
        } else {
            None
        };
        let theme = cx.theme();

        div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .relative()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(344.0))
                    .h_full()
                    .bg(theme.bg_tertiary)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .bg(theme.bg_tertiary)
                            .overflow_hidden()
                            .child(
                                div().w(px(72.0)).h_full().child(
                                    AnyView::from(self.clan_sidebar.clone())
                                        .cached(StyleRefinement::default().size_full()),
                                ),
                            )
                            .child(div().w(px(272.0)).h_full().child(nav_body)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .ml(px(12.))
                            .mr_2()
                            .mb_3()
                            .flex()
                            .flex_col()
                            .max_h(relative(0.5))
                            .min_h_0()
                            .rounded(px(12.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.tokens.border_primary)
                            .shadow_lg()
                            .bg(theme.tokens.bg_surface)
                            .child(
                                div()
                                    .id("clan-footer-bars")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .children(stream_connected_bar)
                                    .children(voice_mini_bar)
                                    .children(update_available_pill)
                                    .children(update_pill)
                                    .children(manual_install_pill),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w_full()
                                    .h(px(56.0))
                                    .child(AnyView::from(self.user_info_bar.clone())),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .overflow_hidden()
                    .child(main_content),
            )
            .children(fullscreen)
    }
}

impl ChatLayout {
    pub(crate) fn send_current_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mention_input) = self.chat_area.mention_input.clone() else {
            return;
        };
        let Some((content, content_tokens, attachments, ogp)) = mention_input
            .update(cx, |mention_input, cx| {
                mention_input.take_payload(window, cx)
            })
        else {
            return;
        };
        let ephemeral_receiver = mention_input.update(cx, |mention_input, cx| {
            mention_input.take_ephemeral_receiver(cx)
        });
        if let Some(receiver_id) = ephemeral_receiver {
            crate::chat::ChatSending::send_ephemeral(
                receiver_id,
                content,
                content_tokens,
                attachments,
                cx,
            );
            return;
        }
        crate::chat::ChatSending::send_text(
            content,
            content_tokens,
            attachments,
            ogp,
            &self.auth_state,
            cx,
        );
    }

    pub(crate) fn open_create_thread(&mut self, cx: &mut Context<Self>) {
        self.thread_popover_handle.hide(cx);
        self.clear_create_thread_inputs(cx);
        ThreadsStore::global(cx).update(cx, |store, cx| store.start_create(cx));
        cx.notify();
    }

    pub(crate) fn close_create_thread(&mut self, cx: &mut Context<Self>) {
        ThreadsStore::global(cx).update(cx, |store, cx| store.cancel_create(cx));
        self.clear_create_thread_inputs(cx);
        cx.notify();
    }

    pub(crate) fn set_create_thread_private(&mut self, private: bool, cx: &mut Context<Self>) {
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.set_create_private(private, cx);
        });
    }

    pub(crate) fn dismiss_threads_popover(&mut self, cx: &mut Context<Self>) {
        self.thread_popover_handle.hide(cx);
        self.clear_thread_search(cx);
        self.close_create_thread(cx);
    }

    fn dismiss_topic_panel(&self, cx: &mut Context<Self>) {
        if TopicsStore::global(cx).read(cx).is_panel_open() {
            TopicsStore::global(cx).update(cx, |store, cx| store.close_panel(cx));
        }
    }

    fn clear_create_thread_inputs(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = &self.thread_name_input {
            input.update(cx, |state, cx| state.clear(cx));
        }
        if let Some(input) = &self.create_thread_message_input {
            input.update(cx, |state, cx| state.clear(cx));
        }
    }

    fn clear_thread_search(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = &self.thread_search_input {
            input.update(cx, |state, cx| state.clear(cx));
        }
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.set_search_query(String::new(), cx);
        });
    }

    pub(crate) fn submit_create_thread(
        &mut self,
        name: String,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.submit_create(name, message, cx);
        });
        let _ = window;
    }

    pub(crate) fn navigate_to_thread(
        &mut self,
        channel_id: &str,
        clan_id: &str,
        parent_id: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(channel_id) = channel_id.parse::<ChannelId>() else {
            return;
        };
        let Ok(clan_id) = clan_id.parse::<ClanId>() else {
            return;
        };
        self.dismiss_threads_popover(cx);
        let label = label.to_string();
        let parent = parent_id.parse::<ChannelId>().ok();
        let (active, active_confirmed) = match ThreadsStore::global(cx)
            .read(cx)
            .thread_active(&channel_id.to_string())
        {
            Some(status) => (
                if status == THREAD_STATUS_ARCHIVED {
                    CHANNEL_ACTIVE_ARCHIVED
                } else {
                    CHANNEL_ACTIVE_JOINED
                },
                true,
            ),
            None => (CHANNEL_ACTIVE_JOINED, false),
        };
        self.channel_list.update(cx, |list, cx| {
            if let Some(parent) = parent {
                list.ensure_thread_with_parent_active(
                    channel_id,
                    parent,
                    clan_id,
                    label.clone(),
                    active,
                    active_confirmed,
                    cx,
                );
            } else {
                list.ensure_thread_channel_with_active(
                    channel_id,
                    label.clone(),
                    active,
                    active_confirmed,
                    cx,
                );
            }
        });
        crate::router::navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }

    fn on_threads_event(&mut self, event: &ThreadsEvent, cx: &mut Context<Self>) {
        match event {
            ThreadsEvent::ThreadCreated {
                channel_id,
                clan_id,
            } => {
                self.close_create_thread(cx);
                self.navigate_to_thread(channel_id, clan_id, "", "", cx);
                ThreadsStore::global(cx).update(cx, |store, cx| store.refresh(cx));
            }
            ThreadsEvent::CreateFailed { .. } | ThreadsEvent::LeaveFailed => {}
            ThreadsEvent::OpenPopoverRequested => {
                self.pending_open_threads_popover = true;
                cx.notify();
            }
        }
    }

    fn on_pinned_event(&mut self, event: &PinnedEvent, cx: &mut Context<Self>) {
        if matches!(event, PinnedEvent::OpenPopoverRequested) {
            self.pending_open_pin_popover = true;
            cx.notify();
        }
    }

    pub(crate) fn ensure_thread_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.thread_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "channelMenu.menu.thread.searchThreads");
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .embedded(true)
        });
        let input_for_sub = input.clone();
        cx.subscribe_in(&input, window, move |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input_for_sub.read(cx).value().to_string();
                ThreadsStore::global(cx).update(cx, |store, cx| {
                    store.set_search_query(query, cx);
                });
            }
        })
        .detach();
        self.thread_search_input = Some(input);
    }

    pub(crate) fn ensure_canvas_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.canvas_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "channelMenu.menu.thread.searchCanvas");
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .embedded(true)
        });
        self.canvas_search_input = Some(input);
    }

    fn ensure_canvas_view(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        canvas_id: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CanvasView> {
        if let Some(existing) = &self.canvas_view {
            existing.update(cx, |view, cx| {
                view.sync_route(clan_id, channel_id, canvas_id, window, cx);
            });
            return existing.clone();
        }
        let settings = self.settings.clone();
        let view =
            cx.new(|cx| CanvasView::new(settings, clan_id, channel_id, canvas_id, window, cx));
        self.canvas_view = Some(view.clone());
        view
    }

    pub(crate) fn settings_language(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn ensure_create_thread_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_name_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let ph = mezon_i18n::t(&locale, "channelTopbar.createThread.placeholder.threadName");
            self.thread_name_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder(ph).embedded(true)));
        }
        if self.create_thread_message_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let ph = mezon_i18n::t(&locale, "chat.messagePlaceholder");
            self.create_thread_message_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder(ph).embedded(true)));
        }
    }

    fn build_create_thread_panel(
        &mut self,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !ThreadsStore::global(cx).read(cx).is_creating() {
            return None;
        }
        self.ensure_create_thread_inputs(window, cx);
        let name_input = self.thread_name_input.clone()?;
        let message_input = self.create_thread_message_input.clone()?;
        let theme = cx.theme().clone();
        let name_error = ThreadsStore::global(cx)
            .read(cx)
            .name_error()
            .map(|s| s.to_string());
        let submitting = ThreadsStore::global(cx).read(cx).is_submitting();
        let create_private = ThreadsStore::global(cx).read(cx).create_private();

        Some(
            crate::chat::create_thread_panel::render_create_thread_panel(
                crate::chat::create_thread_panel::CreateThreadPanelParams {
                    thread_name_input: name_input,
                    message_input,
                    name_error: name_error.as_deref(),
                    submitting,
                    create_private,
                    locale,
                    theme: &theme,
                    layout: cx.entity(),
                },
            ),
        )
    }

    fn build_topic_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !TopicsStore::global(cx).read(cx).is_panel_open() {
            self.topic_panel = None;
            return None;
        }
        if self.topic_panel.is_none() {
            let settings = self.settings.clone();
            self.topic_panel =
                Some(cx.new(|cx| {
                    crate::chat::create_topic_panel::TopicPanel::new(settings, window, cx)
                }));
        }
        self.topic_panel
            .clone()
            .map(|panel| panel.into_any_element())
    }

    pub(crate) fn send_sticker(&mut self, url: String, filename: String, cx: &mut Context<Self>) {
        crate::chat::ChatSending::send_sticker(url, filename, &self.auth_state, cx);
    }

    pub(crate) fn send_gif(
        &mut self,
        url: String,
        width: u32,
        height: u32,
        cx: &mut Context<Self>,
    ) {
        crate::chat::ChatSending::send_gif(url, width, height, &self.auth_state, cx);
    }

    pub(crate) fn send_sound(&mut self, url: String, filename: String, cx: &mut Context<Self>) {
        crate::chat::ChatSending::send_sound(url, filename, &self.auth_state, cx);
    }

    fn current_dm(&self, cx: &Context<Self>) -> Option<DirectChannel> {
        let Route::DirectMessage { direct_id, .. } = Router::global(cx).read(cx).route() else {
            return None;
        };
        self.direct_store.read(cx).find(direct_id).cloned()
    }

    fn is_dm_route(&self, cx: &Context<Self>) -> bool {
        matches!(
            Router::global(cx).read(cx).route(),
            Route::Direct | Route::Friends | Route::DirectMessage { .. }
        )
    }

    fn render_voice_mini_bar(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let store = self.voice_store.read(cx);
        let (channel_id, clan_id) = store.connection().connected_channel()?;
        let channel_id = channel_id.to_string();
        let clan_id = clan_id.to_string();
        let clan_name = clan_id
            .parse::<ClanId>()
            .ok()
            .and_then(|cid| self.clan_list.read(cx).clan(cid).map(|c| c.name.clone()))
            .unwrap_or_default();
        let label = store.channel_label().to_string();
        let mic_enabled = store.mic_enabled();
        let camera_enabled = store.camera_enabled();
        let screen_enabled = store.screen_share_enabled();
        let link_copied = store.link_copied();
        let noise_control = self.render_noise_control(cx);
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        Some(crate::chat::voice::render_mini_bar(
            theme,
            &locale,
            &label,
            &clan_name,
            &channel_id,
            &clan_id,
            &self.voice_store,
            &self.settings,
            mic_enabled,
            camera_enabled,
            screen_enabled,
            link_copied,
            noise_control,
        ))
    }

    fn render_nav_body(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let view: AnyView = if self.is_dm_route(cx) {
            self.direct_sidebar.clone().into()
        } else {
            self.channel_sidebar.clone().into()
        };
        view.cached(StyleRefinement::default().size_full())
            .into_any_element()
    }

    fn sync_voice_session_defaults(&mut self, cx: &App) {
        let key = match self.voice_store.read(cx).connection() {
            VoiceConnection::Connecting { channel_id, .. }
            | VoiceConnection::Connected { channel_id, .. } => Some(channel_id.clone()),
            _ => None,
        };
        if self.voice_session_key != key {
            self.voice_session_key = key;
            self.voice_show_members = true;
            self.voice_show_chat = false;
            self.voice_grid_page = 0;
            self.voice_grid_wheel_accum = 0.;
            self.voice_visual = Default::default();
            self.voice_strip_width = px(0.);
            self.voice_strip_scroll
                .set_offset(gpui::point(px(0.), px(0.)));
        }
    }

    pub(crate) fn toggle_voice_member_strip(&mut self, cx: &mut Context<Self>) {
        self.voice_show_members = !self.voice_show_members;
        cx.notify();
    }

    pub(crate) fn toggle_voice_chat(&mut self, cx: &mut Context<Self>) {
        self.voice_show_chat = !self.voice_show_chat;
        cx.notify();
    }

    pub(crate) fn voice_show_chat(&self) -> bool {
        self.voice_show_chat
    }

    pub(crate) fn record_voice_strip_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if (self.voice_strip_width - width).abs() > px(0.5) {
            self.voice_strip_width = width;
            cx.notify();
        }
    }

    pub(crate) fn record_voice_grid_size(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        if self.voice_grid_size != size {
            self.voice_grid_size = size;
            cx.notify();
        }
    }

    pub(crate) fn scroll_voice_grid_page(
        &mut self,
        delta: f32,
        total_pages: usize,
        cx: &mut Context<Self>,
    ) {
        const STEP: f32 = 100.;
        let last_page = total_pages.saturating_sub(1);
        self.voice_grid_wheel_accum += delta;
        let mut page = self.voice_grid_page.min(last_page);
        while self.voice_grid_wheel_accum <= -STEP {
            self.voice_grid_wheel_accum += STEP;
            page = (page + 1).min(last_page);
        }
        while self.voice_grid_wheel_accum >= STEP {
            self.voice_grid_wheel_accum -= STEP;
            page = page.saturating_sub(1);
        }
        if page != self.voice_grid_page {
            self.voice_grid_page = page;
            cx.notify();
        }
    }

    pub(crate) fn toggle_voice_emoji_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.voice_emoji_picker.is_some() {
            self.close_voice_emoji_picker(cx);
            return;
        }
        self.close_voice_sound_picker(cx);
        let picker = cx.new(|cx| ReactionPicker::new(window, cx));
        let focus_handle = picker.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        self._voice_emoji_picker_sub = Some(cx.subscribe(&picker, |_this, _picker, event, cx| {
            let ReactionPickerEvent::Picked { emoji_id, .. } = event;
            let emoji_id = emoji_id.clone();
            VoiceStore::global(cx).update(cx, |store, cx| {
                store.send_emoji_reaction(emoji_id, cx);
            });
        }));
        self._voice_emoji_picker_dismiss_sub = Some(
            cx.subscribe(&picker, |this, _picker, _: &DismissEvent, cx| {
                this.close_voice_emoji_picker(cx)
            }),
        );
        self.voice_emoji_picker = Some(picker);
        cx.notify();
    }

    fn close_voice_emoji_picker(&mut self, cx: &mut Context<Self>) {
        self.voice_emoji_picker = None;
        self._voice_emoji_picker_sub = None;
        self._voice_emoji_picker_dismiss_sub = None;
        cx.notify();
    }

    pub(crate) fn toggle_voice_sound_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.voice_sound_picker.is_some() {
            self.close_voice_sound_picker(cx);
            return;
        }
        self.close_voice_emoji_picker(cx);
        let picker = cx.new(VoiceSoundPicker::new);
        let focus_handle = picker.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        self._voice_sound_picker_sub = Some(cx.subscribe(&picker, |this, _picker, event, cx| {
            let VoiceSoundPickerEvent::Picked { url } = event;
            let url = url.clone();
            VoiceStore::global(cx).update(cx, |store, cx| {
                store.send_sound_reaction(url, cx);
            });
            this.close_voice_sound_picker(cx);
        }));
        self._voice_sound_picker_dismiss_sub = Some(
            cx.subscribe(&picker, |this, _picker, _: &DismissEvent, cx| {
                this.close_voice_sound_picker(cx)
            }),
        );
        self.voice_sound_picker = Some(picker);
        cx.notify();
    }

    fn close_voice_sound_picker(&mut self, cx: &mut Context<Self>) {
        if self.voice_sound_picker.take().is_some() {
            VoiceStore::global(cx).update(cx, |store, cx| store.stop_sound_preview(cx));
        }
        self._voice_sound_picker_sub = None;
        self._voice_sound_picker_dismiss_sub = None;
        cx.notify();
    }

    fn set_ns_popover_hover(&mut self, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        if hovered {
            self.ns_hovered = true;
            self._ns_popover_close = None;
            if !self.ns_popover_open {
                let level = self.voice_store.read(cx).noise_suppression_level();
                self.ns_slider.update(cx, |slider, cx| {
                    slider.set_value(level as f32, window, cx);
                });
                self.ns_popover_open = true;
                cx.notify();
            }
        } else {
            self.ns_hovered = false;
            if !self.ns_dragging {
                self.schedule_ns_close(cx);
            }
        }
    }

    fn schedule_ns_close(&mut self, cx: &mut Context<Self>) {
        if !self.ns_popover_open || self._ns_popover_close.is_some() {
            return;
        }
        self._ns_popover_close = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            this.update(cx, |this, cx| {
                this.ns_popover_open = false;
                this._ns_popover_close = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn ns_drag_started(&mut self) {
        self.ns_dragging = true;
        self._ns_popover_close = None;
    }

    fn ns_drag_ended(&mut self, inside: bool, cx: &mut Context<Self>) {
        let was_dragging = std::mem::take(&mut self.ns_dragging);
        if inside {
            self.ns_hovered = true;
            self._ns_popover_close = None;
        } else if was_dragging || !self.ns_hovered {
            self.ns_hovered = false;
            self.schedule_ns_close(cx);
        }
    }

    fn render_noise_control(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let store = self.voice_store.read(cx);
        let enabled = store.noise_suppression_enabled();
        let level = store.noise_suppression_level();
        let icon = if enabled {
            Icon::new(IconName::NoiseSupressionIcon)
                .size(px(20.))
                .text_color(theme.text_primary)
        } else {
            Icon::new(IconName::NoiseSupressionDisabledIcon)
                .size(px(20.))
                .text_color(theme.danger_text)
        };
        let button = div()
            .id("voice-ns-btn")
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .child(icon)
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                this.set_ns_popover_hover(*hovered, window, cx);
            }))
            .on_click(cx.listener(|_this, _, _, cx| {
                VoiceStore::global(cx).update(cx, |store, cx| {
                    store.toggle_noise_suppression(cx);
                });
            }));

        let mut root = div().relative().child(button);
        if self.ns_popover_open && enabled {
            root = root.child(deferred(
                div()
                    .id("voice-ns-popover")
                    .absolute()
                    .bottom(px(28.))
                    .right(px(-16.))
                    .w(px(240.))
                    .p_3()
                    .rounded_md()
                    .bg(theme.tokens.bg_theme_contexify)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .occlude()
                    .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                        this.set_ns_popover_hover(*hovered, window, cx);
                    }))
                    .capture_any_mouse_down(
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, _| this.ns_drag_started()),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                            this.ns_drag_ended(true, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                            this.ns_drag_ended(false, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .mb_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child("Noise Suppression"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.text_primary)
                                    .child(format!("{level}%")),
                            ),
                    )
                    .child(Slider::new(&self.ns_slider).horizontal()),
            ));
        }
        root.into_any_element()
    }

    fn get_app_channel_bar(&self, cx: &Context<Self>) -> Option<ChannelAppBarTarget> {
        let channel = self.channel_list.read(cx).active_channel()?;
        if channel.channel_type != ChannelType::App {
            return None;
        }
        let app = self
            .channel_list
            .read(cx)
            .app_channel_for_id(channel.clan_id, channel.id)?;
        let app_id = app.app_id.parse().ok()?;
        Some(ChannelAppBarTarget {
            app_id,
            app_url: app.app_url.clone(),
            clan_id: channel.clan_id,
            clan_name: channel.clan_name.clone(),
            channel_list: self.channel_list.clone(),
        })
    }

    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let window_width = window.viewport_size().width;
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let inbox_handle = self.inbox_handle.clone();
        let active_clan_id = self.active_clan_id(cx);
        let pin_handle = self.pin_popover_handle.clone();
        let canvas_handle = self.canvas_popover_handle.clone();
        let show_results_panel = self.show_results_panel;
        let topic_open = TopicsStore::global(cx).read(cx).is_panel_open();
        let create_thread_open = ThreadsStore::global(cx).read(cx).is_creating();
        let side_panel_open = topic_open || create_thread_open;
        let search_expanded = self.message_search_expanded;
        let show_search_options = self.show_search_options;
        let search_input = self.message_search_input.clone();
        let show_search_bar = crate::chat::message_search::message_search_available(cx).is_some();
        let search_panel = if self.show_results_panel {
            self.message_search_panel.clone()
        } else {
            None
        };

        if let Route::ClanMembers { clan_id } = Router::global(cx).read(cx).route() {
            self.clan_members_page
                .update(cx, |page, cx| page.set_clan(clan_id, cx));
            return self.clan_members_page.clone().into_any_element();
        }

        if let Route::ClanChannels { clan_id } = Router::global(cx).read(cx).route() {
            self.clan_channels_page
                .update(cx, |page, cx| page.set_clan(clan_id, cx));
            return self.clan_channels_page.clone().into_any_element();
        }

        if self.is_dm_route(cx) {
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::Friends | Route::Direct
            ) {
                return self.friends_page.clone().into_any_element();
            }
            if let Some(dm) = self.current_dm(cx) {
                let is_group = dm.kind == DirectKind::Group;
                let in_voice = if is_group {
                    None
                } else {
                    dm.peer_user_id
                        .and_then(|user_id| self.channel_list.read(cx).in_voice_status(user_id))
                };
                return self
                    .chat_area
                    .render(
                        &locale,
                        Some(dm.label.as_str()),
                        None,
                        true,
                        in_voice,
                        Some(dm.id),
                        is_group,
                        is_group
                            && self.show_member_list
                            && !show_results_panel
                            && !side_panel_open,
                        false,
                        false,
                        false,
                        false,
                        None,
                        None,
                        None,
                        Some(pin_handle),
                        None,
                        show_search_bar,
                        search_expanded,
                        show_search_options,
                        search_input.clone(),
                        show_results_panel,
                        search_panel.clone(),
                        None,
                        false,
                        cx,
                    )
                    .into_any_element();
            }
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::DirectMessage { .. }
            ) {
                return self
                    .chat_area
                    .render(
                        &locale,
                        None,
                        None,
                        true,
                        None,
                        None,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        None,
                        None,
                        None,
                        Some(pin_handle),
                        None,
                        false,
                        false,
                        false,
                        None,
                        show_results_panel,
                        search_panel.clone(),
                        None,
                        false,
                        cx,
                    )
                    .into_any_element();
            }
            return self.friends_page.clone().into_any_element();
        }

        if let Some(ch) = self.channel_list.read(cx).active_channel() {
            if let Route::Canvas {
                clan_id,
                channel_id,
                canvas_id,
            } = Router::global(cx).read(cx).route().clone()
            {
                let channel_name = ch.name.clone();
                let active_channel_id = ch.id;
                let header_icon = channel_icon(ch.channel_type, ch.private);
                let canvas = self
                    .ensure_canvas_view(clan_id, channel_id, canvas_id, window, cx)
                    .into_any_element();
                return self
                    .chat_area
                    .render_canvas(
                        &locale,
                        Some(channel_name.as_str()),
                        Some(header_icon),
                        Some(active_channel_id),
                        true,
                        self.show_member_list && !show_results_panel && !topic_open,
                        true,
                        Some(inbox_handle.clone()),
                        active_clan_id.clone(),
                        Some(pin_handle.clone()),
                        Some(canvas_handle.clone()),
                        show_search_bar,
                        search_expanded,
                        show_search_options,
                        search_input.clone(),
                        canvas,
                        cx,
                    )
                    .into_any_element();
            }

            if ch.channel_type == ChannelType::Voice {
                self.sync_voice_session_defaults(cx);
                let channel = ch.clone();
                let (input_device_id, output_device_id, camera_device_id) = {
                    let settings = self.settings.read(cx);
                    (
                        settings.input_device_id.clone(),
                        settings.output_device_id.clone(),
                        settings.camera_device_id.clone(),
                    )
                };
                let show_chat = self.voice_show_chat;
                let voice_view = crate::chat::voice::render_voice_channel(
                    &locale,
                    &channel,
                    &self.voice_store,
                    &self.settings,
                    input_device_id,
                    output_device_id,
                    camera_device_id,
                    &self.voice_strip_scroll,
                    self.voice_strip_width,
                    self.voice_grid_page,
                    self.voice_grid_size,
                    self.voice_show_members,
                    show_chat,
                    self.inbox_handle.clone(),
                    &mut self.voice_visual,
                    window_width,
                    window,
                    cx,
                );
                let chat_panel = if show_chat {
                    Some(
                        self.chat_area
                            .render(
                                &locale,
                                Some(channel.name.as_str()),
                                Some(channel_icon(channel.channel_type, channel.private)),
                                false,
                                None,
                                Some(channel.id),
                                false,
                                false,
                                false,
                                false,
                                false,
                                false,
                                None,
                                None,
                                active_clan_id,
                                None,
                                None,
                                false,
                                false,
                                false,
                                None,
                                false,
                                None,
                                None,
                                true,
                                cx,
                            )
                            .into_any_element(),
                    )
                } else {
                    None
                };
                return div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(voice_view)
                            .when_some(self.voice_emoji_picker.clone(), |el, picker| {
                                el.child(deferred(
                                    div()
                                        .absolute()
                                        .bottom(px(76.))
                                        .left(px(16.))
                                        .occlude()
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.close_voice_emoji_picker(cx)
                                        }))
                                        .child(picker),
                                ))
                            })
                            .when_some(self.voice_sound_picker.clone(), |el, picker| {
                                el.child(deferred(
                                    div()
                                        .absolute()
                                        .bottom(px(76.))
                                        .left(px(72.))
                                        .occlude()
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.close_voice_sound_picker(cx)
                                        }))
                                        .child(picker),
                                ))
                            }),
                    )
                    .when_some(chat_panel, |row, panel| {
                        row.child(
                            div()
                                .flex()
                                .flex_col()
                                .w(px(500.))
                                .min_w(px(360.))
                                .max_w(px(500.))
                                .h_full()
                                .flex_shrink_0()
                                .overflow_hidden()
                                .border_l_1()
                                .border_color(theme.border)
                                .bg(theme.bg_primary)
                                .child(panel),
                        )
                    })
                    .into_any_element();
            }

            if ch.channel_type == ChannelType::Stream {
                let channel = ch.clone();
                let show_chat = self.stream_store.read(cx).show_chat();
                let layout_entity = cx.entity();
                let output_device_id = self.settings.read(cx).output_device_id.clone();
                let stream_view = crate::chat::stream::render_stream_channel(
                    window,
                    &theme,
                    &locale,
                    &channel,
                    &self.stream_store,
                    &self.auth_state,
                    &layout_entity,
                    &self.stream_volume_slider,
                    output_device_id,
                    f32::from(window_width),
                    cx,
                );
                let chat_panel = if show_chat {
                    Some(
                        self.chat_area
                            .render(
                                &locale,
                                Some(channel.name.as_str()),
                                Some(channel_icon(channel.channel_type, channel.private)),
                                false,
                                None,
                                Some(channel.id),
                                false,
                                false,
                                false,
                                false,
                                false,
                                false,
                                None,
                                None,
                                active_clan_id,
                                None,
                                None,
                                false,
                                false,
                                false,
                                None,
                                false,
                                None,
                                None,
                                true,
                                cx,
                            )
                            .into_any_element(),
                    )
                } else {
                    None
                };
                return div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .gap_2()
                    .bg(theme.bg_secondary)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(stream_view),
                    )
                    .when_some(chat_panel, |row, panel| {
                        row.child(
                            div()
                                .flex()
                                .flex_col()
                                .w(px(420.))
                                .min_w(px(360.))
                                .max_w(px(420.))
                                .h_full()
                                .flex_shrink_0()
                                .overflow_hidden()
                                .bg(theme.bg_primary)
                                .child(panel),
                        )
                    })
                    .into_any_element();
            }

            let channel_name = ch.name.clone();
            let channel_id = ch.id;
            let timeline_action = self.channel_supports_timeline_view(cx);
            let timeline_active = self.media_channel_view_mode;
            let media_channel_view = timeline_action && timeline_active;
            let app_channel_bar = self.get_app_channel_bar(cx);
            return self
                .chat_area
                .render(
                    &locale,
                    Some(channel_name.as_str()),
                    Some(channel_icon(ch.channel_type, ch.private)),
                    false,
                    None,
                    Some(channel_id),
                    true,
                    self.show_member_list
                        && !show_results_panel
                        && !side_panel_open
                        && !media_channel_view,
                    true,
                    timeline_action,
                    timeline_active,
                    media_channel_view,
                    if media_channel_view {
                        Some(ch.clan_id)
                    } else {
                        None
                    },
                    Some(inbox_handle),
                    active_clan_id,
                    Some(pin_handle),
                    Some(canvas_handle),
                    show_search_bar,
                    search_expanded,
                    show_search_options,
                    search_input.clone(),
                    show_results_panel,
                    search_panel.clone(),
                    app_channel_bar,
                    false,
                    cx,
                )
                .into_any_element();
        }

        let router = Router::global(cx);
        let route = router.read(cx).route();

        let has_active_clan = self.clan_list.read(cx).active_clan_id.is_some();
        if matches!(route, Route::Channel { .. })
            || (matches!(route, Route::Chat) && has_active_clan)
        {
            return self
                .chat_area
                .render(
                    &locale,
                    None,
                    None,
                    false,
                    None,
                    None,
                    true,
                    self.show_member_list && !show_results_panel && !side_panel_open,
                    true,
                    false,
                    false,
                    false,
                    None,
                    Some(inbox_handle),
                    active_clan_id,
                    None,
                    Some(canvas_handle),
                    show_search_bar,
                    search_expanded,
                    show_search_options,
                    search_input.clone(),
                    show_results_panel,
                    search_panel,
                    None,
                    false,
                    cx,
                )
                .into_any_element();
        }

        let current_path = router.read(cx).current_path();
        let theme = cx.theme();

        let placeholder = match route {
            Route::Chat => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Inbox,
                mezon_i18n::t(&locale, "nav.chat"),
                &current_path,
            ),
            Route::Direct => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                mezon_i18n::t(&locale, "dm.title"),
                &current_path,
            ),
            Route::DirectMessage {
                direct_id,
                message_type: _,
            } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                &format!("Direct {direct_id}"),
                &current_path,
            ),
            Route::Channel { .. } | Route::ClanMembers { .. } | Route::ClanChannels { .. } => {
                div().into_any_element()
            }
            Route::Friends => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::IconFriends,
                mezon_i18n::t(&locale, "directMessage.friends"),
                &current_path,
            ),
            Route::Thread { channel_id, .. } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Hashtag,
                &format!("Thread #{channel_id}"),
                &current_path,
            ),
            Route::Canvas { channel_id, .. } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Hashtag,
                &format!("Canvas #{channel_id}"),
                &current_path,
            ),
            Route::AddFriend { username } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                &format!("Add Friend: {username}"),
                &current_path,
            ),
            Route::Invite { invite_id } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                &format!("Invite: {invite_id}"),
                &current_path,
            ),
            Route::SettingsAccount
            | Route::SettingsProfile
            | Route::SettingsClanProfile { .. }
            | Route::SettingsDevices
            | Route::SettingsAppearance
            | Route::SettingsActivity
            | Route::SettingsNotifications
            | Route::SettingsLanguage
            | Route::SettingsVoice
            | Route::SettingsAdvanced
            | Route::ClanSettings { .. }
            | Route::ChannelSettings { .. }
            | Route::NotFound { .. } => div().into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .p_6()
            .child(placeholder)
            .into_any_element()
    }

    fn render_placeholder(
        &self,
        theme: &Theme,
        icon: crate::components::primitives::IconName,
        title: &str,
        _path: &str,
    ) -> gpui::AnyElement {
        use crate::components::primitives::Icon;

        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .flex_col()
            .gap_4()
            .child(Icon::new(icon).size_8().text_color(theme.text_muted))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(title.to_string()),
            )
            .into_any_element()
    }
}

fn resolve_mention_name_to_user_id(
    name: &str,
    clan_id: ClanId,
    is_direct: bool,
    channel_id: ChannelId,
    cx: &App,
) -> Option<String> {
    let needle = name.to_lowercase();
    if is_direct {
        let store = GroupMembersStore::global(cx);
        let store = store.read(cx);
        return store.members(channel_id).iter().find_map(|member| {
            let username_match = member.user.username.eq_ignore_ascii_case(name);
            let name_match = member.name().to_lowercase() == needle;
            (username_match || name_match).then(|| member.id().to_string())
        });
    }
    let store = ClanMembersStore::global(cx);
    let store = store.read(cx);
    store.members(clan_id).into_iter().find_map(|member| {
        let username_match = member.user.username.eq_ignore_ascii_case(name);
        let name_match = member.name().to_lowercase() == needle;
        (username_match || name_match).then(|| member.id().to_string())
    })
}
