use std::collections::HashMap;

use gpui::{
    AnyView, App, Context, DismissEvent, Entity, Focusable, StyleRefinement, Subscription, Task,
    Window, deferred, div, prelude::*, px,
};
use mezon_store::{
    AuthState, Channel, ChannelId, ChannelList, ChannelType, ClanId, ClanList, ClanMembersStore,
    DirectChannel, DirectKind, DirectMessageStore, GroupMembersStore, InboxStore,
    MessageSearchEvent, MessageSearchStore, MessagesStore, Settings, ThreadsEvent, ThreadsStore,
    VoiceMember, VoiceModerationError, VoiceStore, expand_mention_name_tokens,
};
use ui::PopoverMenuHandle;
use ui::utils::ROUNDED_BORDER_WINDOW;

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
    direct_store: Entity<DirectMessageStore>,
    user_info_bar: Entity<UserInfoBar>,
    clan_list: Entity<ClanList>,
    auth_state: Entity<AuthState>,
    settings: Entity<Settings>,
    voice_store: Entity<VoiceStore>,
    pending_channel_id: Option<ChannelId>,
    prefetched_voice_channel: Option<ChannelId>,
    show_member_list: bool,
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
    thread_name_input: Option<Entity<InputState>>,
    create_thread_message_input: Option<Entity<InputState>>,
    pin_popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    displayed_active_channel: Option<ActiveChannelSlice>,
    displayed_voice_mini: Option<VoiceMiniSlice>,
    displayed_threads_panel: ThreadsPanelSlice,
    displayed_inbox: InboxDisplaySlice,
    pending_open_threads_popover: bool,
    voice_emoji_picker: Option<Entity<ReactionPicker>>,
    _voice_emoji_picker_sub: Option<Subscription>,
    _voice_emoji_picker_dismiss_sub: Option<Subscription>,
    voice_sound_picker: Option<Entity<VoiceSoundPicker>>,
    _voice_sound_picker_sub: Option<Subscription>,
    _voice_sound_picker_dismiss_sub: Option<Subscription>,
    ns_slider: Entity<SliderState>,
    _ns_slider_sub: Subscription,
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
    voice_members: Vec<VoiceMember>,
}

impl ActiveChannelSlice {
    fn from_channel(channel: &Channel) -> Self {
        Self {
            id: channel.id,
            name: channel.name.clone(),
            channel_type: channel.channel_type,
            voice_members: channel.voice_members.clone(),
        }
    }

    fn differs_from(&self, channel: &Channel) -> bool {
        self.id != channel.id
            || self.channel_type != channel.channel_type
            || self.name != channel.name
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

        let user_info_bar = cx.new(|cx| UserInfoBar::new(auth_state.clone(), cx));

        let direct_store = DirectMessageStore::global(cx);

        // TODO: break global listener into smaller pieces
        cx.observe(&direct_store, |_, _, cx| {
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::DirectMessage { .. }
            ) {
                cx.notify();
            }
        })
        .detach();

        let voice_store = VoiceStore::global(cx);
        cx.observe(&voice_store, |this, voice, cx| {
            if let Some(err) = voice.update(cx, |store, _| store.take_moderation_error()) {
                let locale = this.settings.read(cx).language.clone();
                let key = match err {
                    VoiceModerationError::MuteFailed => "channelVoice.muteMemberFailed",
                    VoiceModerationError::KickFailed => "channelVoice.kickMemberFailed",
                };
                let msg = mezon_i18n::t(&locale, key).to_string();
                Shell::global(cx).update(cx, |shell, cx| shell.error(msg, cx));
            }
            let mini_changed = this.voice_mini_display_changed(cx);
            if mini_changed || this.is_voice_frame_relevant(cx) {
                cx.notify();
            }
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
                let mezon_store::ClanMembersEvent::Changed { clan_id } = event;
                if this.visible_voice_clan_id(cx) == Some(*clan_id) {
                    cx.notify();
                }
            },
        )
        .detach();

        let chat_area = ChatArea::new(settings.clone(), cx);
        cx.observe(&channel_list, |this, _, cx| {
            this.apply_pending_channel(cx);
            this.ensure_active_channel_for_clan(cx);
            this.sync_inbox_context(cx);
            if this.active_channel_display_changed(cx) {
                this.dismiss_threads_popover(cx);
                this.pin_popover_handle.hide(cx);
                cx.notify();
            }
        })
        .detach();
        cx.observe(&Router::global(cx), |this, _, cx| {
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::Direct | Route::Friends | Route::DirectMessage { .. }
            ) {
                this.dismiss_threads_popover(cx);
                this.pin_popover_handle.hide(cx);
            }
            this.reset_message_search(cx);
            this.sync_active_from_route(cx);
            this.ensure_active_channel_for_clan(cx);
            this.dismiss_inbox_popover(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&MessageSearchStore::global(cx), |_, _, cx| cx.notify())
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
        let mut this = Self {
            channel_list,
            clan_sidebar,
            channel_sidebar,
            direct_sidebar,
            friends_page,
            direct_store,
            user_info_bar,
            clan_list,
            auth_state,
            chat_area,
            settings,
            voice_store,
            pending_channel_id: None,
            prefetched_voice_channel: None,
            show_member_list: true,
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
            thread_name_input: None,
            create_thread_message_input: None,
            pin_popover_handle: PopoverMenuHandle::default(),
            displayed_active_channel: None,
            displayed_voice_mini: None,
            displayed_threads_panel: ThreadsPanelSlice::default(),
            displayed_inbox: InboxDisplaySlice::default(),
            pending_open_threads_popover: false,
            voice_emoji_picker: None,
            _voice_emoji_picker_sub: None,
            _voice_emoji_picker_dismiss_sub: None,
            voice_sound_picker: None,
            _voice_sound_picker_sub: None,
            _voice_sound_picker_dismiss_sub: None,
            ns_slider,
            _ns_slider_sub: ns_slider_sub,
            ns_popover_open: false,
            ns_hovered: false,
            ns_dragging: false,
            _ns_popover_close: None,
        };
        this.sync_active_from_route(cx);
        this.sync_inbox_context(cx);
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
        self.show_member_list = !self.show_member_list;
        cx.notify();
    }

    fn dismiss_inbox_popover(&self, cx: &mut App) {
        self.inbox_handle.hide(cx);
    }

    fn sync_inbox_context(&self, cx: &mut Context<Self>) {
        let clan_id = self
            .clan_list
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string());
        let channel_id = self
            .channel_list
            .read(cx)
            .active_channel_id
            .map(|id| id.to_string());
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
            MessagesStore::global(cx).update(cx, |store, cx| store.open_channel(channel_id, cx));
        } else {
            self.pending_channel_id = Some(channel_id);
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
        }
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
            && self
                .channel_list
                .read(cx)
                .channel_in_clan(clan_id, channel_id)
        {
            return;
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

    fn drive_voice_video(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.voice_store
            .update(cx, |store, cx| store.flush_texture_drops(Some(window), cx));
        if self.is_voice_frame_relevant(cx) && self.voice_store.read(cx).has_active_video() {
            window.request_animation_frame();
        }
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

    fn visible_voice_clan_id(&self, cx: &Context<Self>) -> Option<ClanId> {
        if self.is_dm_route(cx) {
            return None;
        }
        self.channel_list
            .read(cx)
            .active_channel()
            .filter(|ch| ch.channel_type == ChannelType::Voice)
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
        self.maybe_prefetch_voice_token(cx);
        self.drive_voice_video(window, cx);

        if std::mem::take(&mut self.pending_open_threads_popover) {
            let handle = self.thread_popover_handle.clone();
            window.defer(cx, move |window, cx| handle.show(window, cx));
        }

        let nav_body = self.render_nav_body(cx);
        let locale = self.settings.read(cx).language.clone();
        let create_panel = self.build_create_thread_panel(&locale, window, cx);
        let chat_content = self.render_content(cx);
        let main_content = if let Some(panel) = create_panel {
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
        let fullscreen = if self.connected_call_is_active(cx) {
            let chat = cx.entity();
            crate::chat::voice::render_screen_fullscreen_overlay(
                cx.theme(),
                &locale,
                &self.voice_store,
                &self.settings,
                self.voice_store.read(cx),
                &chat,
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
                    .relative()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .bg(theme.bg_tertiary)
                            .rounded_bl(px(ROUNDED_BORDER_WINDOW))
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
                            .absolute()
                            .left(px(12.0))
                            .right(px(8.0))
                            .bottom(px(12.0))
                            .flex()
                            .flex_col()
                            .rounded(px(12.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.tokens.border_theme_primary)
                            .shadow_lg()
                            .bg(theme.tokens.bg_surface)
                            .occlude()
                            .children(voice_mini_bar)
                            .child(
                                AnyView::from(self.user_info_bar.clone())
                                    .cached(StyleRefinement::default().w_full().h(px(56.0))),
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
        let Some((content, content_tokens, attachments)) = mention_input
            .update(cx, |mention_input, cx| {
                mention_input.take_payload(window, cx)
            })
        else {
            return;
        };
        crate::chat::ChatSending::send_text(
            content,
            content_tokens,
            attachments,
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
        self.thread_popover_handle.hide(cx);
        self.clear_thread_search(cx);
        let label = label.to_string();
        let parent = parent_id.parse::<ChannelId>().ok();
        self.channel_list.update(cx, |list, cx| {
            if let Some(parent) = parent {
                list.ensure_thread_with_parent(channel_id, parent, clan_id, label.clone(), cx);
            } else {
                list.ensure_thread_channel(channel_id, label.clone(), cx);
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
            ThreadsEvent::CreateFailed { message } => {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.error(message.clone(), cx);
                });
                cx.notify();
            }
            ThreadsEvent::OpenPopoverRequested => {
                self.pending_open_threads_popover = true;
                cx.notify();
            }
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

    pub(crate) fn send_sticker(&mut self, url: String, filename: String, cx: &mut Context<Self>) {
        crate::chat::ChatSending::send_sticker(url, filename, &self.auth_state, cx);
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
                .text_color(theme.status_dnd)
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

    fn render_content(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let inbox_handle = self.inbox_handle.clone();
        let active_clan_id = self.active_clan_id(cx);
        let pin_handle = self.pin_popover_handle.clone();
        let show_results_panel = self.show_results_panel;
        let search_expanded = self.message_search_expanded;
        let show_search_options = self.show_search_options;
        let search_input = self.message_search_input.clone();
        let show_search_bar = crate::chat::message_search::message_search_available(cx).is_some();
        let search_panel = if self.show_results_panel {
            self.message_search_panel.clone()
        } else {
            None
        };

        if self.is_dm_route(cx) {
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::Friends | Route::Direct
            ) {
                return self.friends_page.clone().into_any_element();
            }
            if let Some(dm) = self.current_dm(cx) {
                let is_group = dm.kind == DirectKind::Group;
                return self
                    .chat_area
                    .render(
                        &locale,
                        Some(dm.label.as_str()),
                        true,
                        Some(dm.id),
                        is_group,
                        is_group && self.show_member_list && !show_results_panel,
                        false,
                        None,
                        None,
                        None,
                        show_search_bar,
                        search_expanded,
                        show_search_options,
                        search_input.clone(),
                        show_results_panel,
                        search_panel.clone(),
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
                        true,
                        None,
                        false,
                        false,
                        false,
                        None,
                        None,
                        None,
                        false,
                        false,
                        false,
                        None,
                        show_results_panel,
                        search_panel.clone(),
                        cx,
                    )
                    .into_any_element();
            }
            return self.friends_page.clone().into_any_element();
        }

        if let Some(ch) = self.channel_list.read(cx).active_channel() {
            if ch.channel_type == ChannelType::Voice {
                let channel = ch.clone();
                let (input_device_id, output_device_id) = {
                    let settings = self.settings.read(cx);
                    (
                        settings.input_device_id.clone(),
                        settings.output_device_id.clone(),
                    )
                };
                let voice_view = crate::chat::voice::render_voice_channel(
                    theme,
                    &locale,
                    &channel,
                    &self.voice_store,
                    &self.settings,
                    input_device_id,
                    output_device_id,
                    cx,
                );
                return div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(voice_view)
                    .when_some(self.voice_emoji_picker.clone(), |el, picker| {
                        el.child(deferred(
                            div()
                                .absolute()
                                .bottom(px(76.))
                                .left(px(16.))
                                .occlude()
                                .on_mouse_down_out(
                                    cx.listener(|this, _, _, cx| this.close_voice_emoji_picker(cx)),
                                )
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
                                .on_mouse_down_out(
                                    cx.listener(|this, _, _, cx| this.close_voice_sound_picker(cx)),
                                )
                                .child(picker),
                        ))
                    })
                    .into_any_element();
            }

            let channel_name = ch.name.clone();
            let channel_id = ch.id;
            return self
                .chat_area
                .render(
                    &locale,
                    Some(channel_name.as_str()),
                    false,
                    Some(channel_id),
                    true,
                    self.show_member_list && !show_results_panel,
                    true,
                    Some(inbox_handle),
                    active_clan_id,
                    Some(pin_handle),
                    show_search_bar,
                    search_expanded,
                    show_search_options,
                    search_input.clone(),
                    show_results_panel,
                    search_panel.clone(),
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
                    false,
                    None,
                    true,
                    self.show_member_list && !show_results_panel,
                    true,
                    Some(inbox_handle),
                    active_clan_id,
                    None,
                    show_search_bar,
                    search_expanded,
                    show_search_options,
                    search_input.clone(),
                    show_results_panel,
                    search_panel,
                    cx,
                )
                .into_any_element();
        }

        let current_path = router.read(cx).current_path();

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
            Route::Channel { .. } => div().into_any_element(),
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
            | Route::SettingsDevices
            | Route::SettingsAppearance
            | Route::SettingsActivity
            | Route::SettingsNotifications
            | Route::SettingsLanguage
            | Route::SettingsVoice
            | Route::SettingsAdvanced
            | Route::ClanSettings { .. }
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
