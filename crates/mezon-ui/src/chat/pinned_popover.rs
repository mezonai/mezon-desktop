use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, ClickEvent, ClipboardItem, Context, DismissEvent, DispatchPhase, Element, ElementId,
    Entity, EventEmitter, FocusHandle, Focusable, FontWeight, GlobalElementId, HighlightStyle,
    Hitbox, HitboxBehavior, InspectorElementId, InteractiveText, IntoElement, KeyDownEvent,
    LayoutId, ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, SharedString, StyledText, TextLayout, UnderlineStyle, WeakEntity, Window, div, img,
    list, prelude::*, px, rems, rgba,
};
use mezon_store::{
    AccountStore, AttachmentSeedInput, ChannelId, ClanMembersStore, DirectMessageStore, Embed,
    Message, MessageAttachment, MessageId, MessageSpan, MessagesStore, PinnedMessage,
    PinnedMessagesStore, PollData, RichLayout, Settings, UserId, UsersByUserStore,
    strip_code_fence,
};
use ui::{PopoverMenuHandle, ScrollAxes, Scrollbars, WithScrollbar};

use crate::app::shell::Shell;
use crate::chat::file_type_icon::file_type_icon_for;
use crate::chat::message::parts::{
    effective_clan_id, open_viewer_from_message, resolve_pin_avatar_url,
    resolve_pin_sender_label_with_message,
};
use crate::chat::message::selection::{
    MessageSelectionState, SelPoint, SelectableRegion, SharedSelection, TextSegment,
    merge_selection_background, word_range,
};
use crate::chat::message::{
    ConfirmUnpinMessageModal, SELECTION_BG, code_block_copy_overlay, heading_line_height,
    heading_size, open_message_link, pin_link_element, render_ogp_preview,
    render_pin_rich_layout_element, render_poll_card_readonly, resolve_message_link_url,
};
use crate::components::primitives::text_actions::Copy;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Sizable, Size, Spinner, h_flex, v_flex,
};
use crate::image_cache::{
    LruImageCache, MESSAGE_ENTRY_MAX_BYTES, MESSAGE_IMAGE_CACHE_BYTES, MESSAGE_IMAGE_CACHE_CAPACITY,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::download::save_with_progress_toast;

const POPOVER_WIDTH: f32 = 420.;
const HEADER_HEIGHT: f32 = 48.;
const PANEL_MIN_HEIGHT: f32 = 200.;
const LIST_BODY_HEIGHT: f32 = 500.;
const PANEL_MAX_VIEWPORT_OFFSET: f32 = 180.;
const LIST_OVERDRAW: f32 = 200.;
const LIST_PAD_X: f32 = 16.;
const LIST_PAD_Y: f32 = 8.;
const EMPTY_BODY_HEIGHT: f32 = 144.;
const FILE_NAME_COLOR: u32 = 0x3b_82_f6;
const ATTACHMENT_PREVIEW_SIZE: f32 = 120.;

#[derive(Clone)]
struct PinCardVm {
    pin_id: SharedString,
    message_id: SharedString,
    create_time: i64,
    sender_label: SharedString,
    is_anonymous: bool,
    avatar_src: Option<SharedString>,
    avatar_fallback: Option<SharedString>,
    pin: Arc<PinnedMessage>,
    text_spans: Arc<[MessageSpan]>,
    selectable_text: SharedString,
    poll: Option<Arc<PollData>>,
    poll_my_vote: Arc<[i32]>,
}

impl PinCardVm {
    fn resolve(
        msg: &PinnedMessage,
        clan_id: Option<mezon_store::ClanId>,
        channel_id: Option<ChannelId>,
        cx: &App,
    ) -> Self {
        let sender_label = resolve_pin_sender_label_with_message(
            &msg.sender_id,
            &msg.sender_name,
            Some(msg.message_id.as_str()),
            clan_id,
            channel_id,
            cx,
        );
        let (avatar_src, avatar_fallback) = resolve_pin_avatar_urls(msg, clan_id, channel_id, cx);
        let (poll, poll_my_vote) = resolve_pin_poll(msg, channel_id, cx);
        let text_spans = prepare_pin_text_spans(msg);
        let selectable_text = pin_canonical_text(msg, &text_spans);
        Self {
            pin_id: msg.id.clone().into(),
            message_id: msg.message_id.clone().into(),
            create_time: msg.create_time,
            sender_label,
            is_anonymous: mezon_store::is_anonymous_sender_id(&msg.sender_id, cx),
            avatar_src,
            avatar_fallback,
            pin: Arc::new(msg.clone()),
            text_spans,
            selectable_text,
            poll,
            poll_my_vote,
        }
    }
}

fn resolve_pin_poll(
    pin: &PinnedMessage,
    channel_id: Option<ChannelId>,
    cx: &App,
) -> (Option<Arc<PollData>>, Arc<[i32]>) {
    if pin.poll.is_none() {
        return (None, Arc::default());
    }
    let Ok(message_id) = pin.message_id.parse::<MessageId>() else {
        return (pin.poll.clone().map(Arc::from), Arc::default());
    };
    let store = MessagesStore::global(cx).read(cx);
    let live = channel_id
        .and_then(|channel_id| store.message_in_channel(channel_id, message_id))
        .and_then(|msg| msg.poll.clone());
    let my_vote: Arc<[i32]> = store
        .poll_my_vote(message_id)
        .map(Arc::from)
        .unwrap_or_default();
    (live.or_else(|| pin.poll.clone()).map(Arc::from), my_vote)
}

pub(crate) fn render_pinned_message_preview(
    pin: &PinnedMessage,
    theme: &Theme,
    locale: &str,
    image_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
) -> gpui::AnyElement {
    let text_spans = prepare_pin_text_spans(pin);
    let selectable_text = pin_canonical_text(pin, &text_spans);
    let poll = pin.poll.as_deref().map(|poll| (poll, &[] as &[i32]));
    render_pin_body(
        pin,
        &text_spans,
        &selectable_text,
        poll,
        theme,
        locale,
        image_cache,
        ogp_cache,
        None,
        None,
    )
}

fn pinned_message_from_chat_message(msg: &Message) -> PinnedMessage {
    PinnedMessage {
        id: msg.id.to_string(),
        message_id: msg.id.to_string(),
        sender_id: msg.sender_id.clone(),
        sender_name: msg.sender_name.to_string(),
        avatar_url: msg.avatar_url.to_string(),
        avatar_proxied: msg.avatar_proxied.clone(),
        content: msg.content.clone(),
        raw_content: msg.raw_content.as_deref().unwrap_or("").to_string(),
        spans: msg.spans.clone().into(),
        rich_layout: msg.rich_layout.clone(),
        ogp: msg.ogp.clone(),
        embeds: msg.embeds.clone(),
        attachments: msg.attachments.clone(),
        poll: msg.poll.clone(),
        create_time: msg.create_time,
    }
}

pub(crate) fn render_pin_message_preview(
    msg: &Message,
    theme: &Theme,
    locale: &str,
    image_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
) -> gpui::AnyElement {
    render_pinned_message_preview(
        &pinned_message_from_chat_message(msg),
        theme,
        locale,
        image_cache,
        ogp_cache,
    )
}

pub struct PinnedPopoverPanel {
    settings: Entity<Settings>,
    popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    list_state: ListState,
    focus_handle: FocusHandle,
    avatar_image_cache: Entity<LruImageCache>,
    message_image_cache: Entity<LruImageCache>,
    ogp_image_cache: Entity<LruImageCache>,
    pin_cards: Vec<PinCardVm>,
    selection: SharedSelection,
    _subs: Vec<gpui::Subscription>,
}

impl PinnedPopoverPanel {
    pub fn new(
        settings: Entity<Settings>,
        popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let subs = vec![
            cx.observe(&PinnedMessagesStore::global(cx), |this, _, cx| {
                this.pin_cards = this.compute_pin_cards(cx);
                cx.notify();
            }),
            cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
                this.refresh_name_rows(cx);
            }),
            cx.observe(&DirectMessageStore::global(cx), |this, _, cx| {
                this.refresh_name_rows(cx);
            }),
            cx.observe(&UsersByUserStore::global(cx), |this, _, cx| {
                this.refresh_name_rows(cx);
            }),
            cx.observe(&AccountStore::global(cx), |this, _, cx| {
                this.refresh_name_rows(cx);
            }),
            cx.observe(&settings, |_, _, cx| cx.notify()),
        ];

        let avatar_image_cache = crate::image_cache::shared_avatar_cache(cx);
        let message_image_cache = cx.new(|cx| {
            LruImageCache::message(
                "pinned-image",
                MESSAGE_IMAGE_CACHE_CAPACITY,
                MESSAGE_IMAGE_CACHE_BYTES,
                MESSAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let ogp_image_cache = crate::image_cache::ogp_aux_cache("pinned-ogp", cx);
        let list_state = ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW)).measure_all();

        let mut panel = Self {
            settings,
            popover_handle,
            list_state,
            focus_handle,
            avatar_image_cache,
            message_image_cache,
            ogp_image_cache,
            pin_cards: Vec::new(),
            selection: MessageSelectionState::new_shared(),
            _subs: subs,
        };
        panel.pin_cards = panel.compute_pin_cards(cx);
        panel
    }

    fn refresh_name_rows(&mut self, cx: &mut Context<Self>) {
        let store = PinnedMessagesStore::global(cx).read(cx);
        if self.pin_cards.len() != store.pinned().len() {
            self.pin_cards = self.compute_pin_cards(cx);
            cx.notify();
            return;
        }
        let clan_id = effective_clan_id(store.clan_id(), cx);
        let channel_id = store.channel_id();
        for (vm, pin) in self.pin_cards.iter_mut().zip(store.pinned()) {
            if vm.message_id.as_ref() != pin.message_id.as_str() {
                self.pin_cards = self.compute_pin_cards(cx);
                cx.notify();
                return;
            }
            vm.sender_label = resolve_pin_sender_label_with_message(
                &pin.sender_id,
                &pin.sender_name,
                Some(pin.message_id.as_str()),
                clan_id,
                channel_id,
                cx,
            );
            vm.is_anonymous = mezon_store::is_anonymous_sender_id(&pin.sender_id, cx);
            (vm.avatar_src, vm.avatar_fallback) =
                resolve_pin_avatar_urls(pin, clan_id, channel_id, cx);
        }
        cx.notify();
    }

    fn compute_pin_cards(&self, cx: &App) -> Vec<PinCardVm> {
        let store = PinnedMessagesStore::global(cx).read(cx);
        let clan_id = effective_clan_id(store.clan_id(), cx);
        let channel_id = store.channel_id();
        store
            .pinned()
            .iter()
            .map(|msg| PinCardVm::resolve(msg, clan_id, channel_id, cx))
            .collect()
    }

    fn sync_selection_order(&self) {
        let mut state = self.selection.borrow_mut();
        state.order_map.clear();
        for (index, vm) in self.pin_cards.iter().enumerate() {
            if let Ok(message_id) = vm.message_id.parse::<MessageId>() {
                state.order_map.insert(message_id, index);
            }
        }
    }

    fn pin_point_at(&self, position: gpui::Point<gpui::Pixels>) -> Option<SelPoint> {
        let state = self.selection.borrow();
        state
            .registry
            .iter()
            .find_map(|(id, layout)| {
                text_layout_offset_at(layout, position).map(|offset| SelPoint {
                    message_id: *id,
                    offset,
                })
            })
            .or_else(|| {
                state.segment_registry.iter().find_map(|(id, entry)| {
                    entry.segments.iter().find_map(|segment| {
                        segment.offset_at(position).map(|offset| SelPoint {
                            message_id: *id,
                            offset,
                        })
                    })
                })
            })
            .or_else(|| {
                state.segment_registry.iter().find_map(|(id, entry)| {
                    let Some((top, bottom)) = entry.vertical_bounds() else {
                        return None;
                    };
                    if position.y < top || position.y > bottom {
                        return None;
                    }
                    let mut best: Option<(gpui::Pixels, usize)> = None;
                    for segment in &entry.segments {
                        if let Some((distance, offset)) = segment.snapped_offset(position)
                            && best.is_none_or(|(best_distance, _)| distance < best_distance)
                        {
                            best = Some((distance, offset));
                        }
                    }
                    best.map(|(_, offset)| SelPoint {
                        message_id: *id,
                        offset,
                    })
                })
            })
            .or_else(|| {
                let mut best: Option<(gpui::Pixels, SelPoint)> = None;
                let mut consider =
                    |id: MessageId, top: gpui::Pixels, bottom: gpui::Pixels, end: usize| {
                        let (dy, offset) = if position.y < top {
                            (top - position.y, 0usize)
                        } else if position.y > bottom {
                            (position.y - bottom, end)
                        } else {
                            return;
                        };
                        if best.as_ref().is_none_or(|(best_dy, _)| dy < *best_dy) {
                            best = Some((
                                dy,
                                SelPoint {
                                    message_id: id,
                                    offset,
                                },
                            ));
                        }
                    };
                for (id, layout) in &state.registry {
                    if let (Some(bounds), Some(len)) = (layout.try_bounds(), layout.try_len()) {
                        consider(*id, bounds.top(), bounds.bottom(), len);
                    }
                }
                for (id, entry) in &state.segment_registry {
                    if let Some((top, bottom)) = entry.vertical_bounds() {
                        consider(*id, top, bottom, entry.text.len());
                    }
                }
                best.map(|(_, point)| point)
            })
    }

    fn selectable_text_for(&self, message_id: MessageId) -> SharedString {
        self.pin_cards
            .iter()
            .find(|vm| vm.message_id.parse::<MessageId>().ok() == Some(message_id))
            .map(|vm| vm.selectable_text.clone())
            .unwrap_or_default()
    }

    fn on_selection_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.sync_selection_order();
        let hit = self.pin_point_at(event.position);
        match hit {
            Some(point) => {
                let text = self.selectable_text_for(point.message_id);
                let range = if event.click_count >= 2 {
                    let offset = point.offset.min(text.len());
                    if event.click_count >= 3 {
                        0..text.len()
                    } else {
                        word_range(&text, offset)
                    }
                } else {
                    point.offset..point.offset
                };
                let mut state = self.selection.borrow_mut();
                state.selecting = true;
                state.anchor = Some(SelPoint {
                    message_id: point.message_id,
                    offset: range.start,
                });
                state.head = Some(SelPoint {
                    message_id: point.message_id,
                    offset: range.end,
                });
                cx.notify();
            }
            None => {
                if self.selection.borrow().has_selection() {
                    self.selection.borrow_mut().clear();
                    cx.notify();
                }
            }
        }
    }

    fn on_selection_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .selection
            .try_borrow()
            .is_ok_and(|selection| selection.selecting)
        {
            return;
        }
        self.sync_selection_order();
        let Some(point) = self.pin_point_at(event.position) else {
            return;
        };
        let changed = self.selection.try_borrow_mut().is_ok_and(|mut state| {
            let changed = state.head != Some(point);
            state.head = Some(point);
            changed
        });
        if changed {
            cx.notify();
        }
    }

    fn on_selection_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection.try_borrow_mut().is_ok_and(|mut state| {
            let was = state.selecting;
            state.selecting = false;
            was
        }) {
            cx.notify();
        }
    }

    fn copy_selection(&mut self, cx: &mut App) -> bool {
        self.sync_selection_order();
        let text = {
            let state = self.selection.borrow();
            if !state.has_selection() {
                return false;
            }
            let mut parts = Vec::new();
            for vm in &self.pin_cards {
                let Ok(message_id) = vm.message_id.parse::<MessageId>() else {
                    continue;
                };
                if !state.includes_message(message_id) {
                    continue;
                }
                let full = state
                    .segment_registry
                    .get(&message_id)
                    .map(|entry| entry.text.to_string())
                    .or_else(|| {
                        state
                            .registry
                            .get(&message_id)
                            .and_then(TextLayout::try_text)
                    })
                    .unwrap_or_else(|| vm.selectable_text.to_string());
                let Some(range) = state.range_for_message(message_id, &full) else {
                    continue;
                };
                if range.start < range.end {
                    parts.push(full[range].to_string());
                }
            }
            (!parts.is_empty()).then(|| parts.join("\n"))
        };
        let Some(text) = text else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn on_pin_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        if event.keystroke.key == "c" {
            let modifiers = &event.keystroke.modifiers;
            let copy_combo = if cfg!(target_os = "macos") {
                modifiers.platform
            } else {
                modifiers.control
            };
            if copy_combo && !modifiers.alt && self.copy_selection(cx) {
                cx.stop_propagation();
            }
        }
    }
}

fn text_layout_offset_at(
    layout: &TextLayout,
    position: gpui::Point<gpui::Pixels>,
) -> Option<usize> {
    let bounds = layout.try_bounds()?;
    if bounds.contains(&position) {
        Some(
            layout
                .try_index_for_position(position)?
                .unwrap_or_else(|err| err),
        )
    } else {
        None
    }
}

fn register_pin_selection_listeners(
    window: &mut Window,
    host: WeakEntity<PinnedPopoverPanel>,
    selection: SharedSelection,
    hitbox: Hitbox,
) {
    let down_host = host.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || event.button != MouseButton::Left
            || !hitbox.is_hovered(window)
        {
            return;
        }
        let event = event.clone();
        let host = down_host.clone();
        window.defer(cx, move |window, cx| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.on_selection_down(&event, window, cx));
            }
        });
    });
    let move_host = host.clone();
    let move_selection = selection.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || !move_selection
                .try_borrow()
                .is_ok_and(|selection| selection.selecting)
        {
            return;
        }
        let event = event.clone();
        let host = move_host.clone();
        window.defer(cx, move |window, cx| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.on_selection_move(&event, window, cx));
            }
        });
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || event.button != MouseButton::Left
            || !selection
                .try_borrow()
                .is_ok_and(|selection| selection.selecting)
        {
            return;
        }
        let event = event.clone();
        let host = host.clone();
        window.defer(cx, move |window, cx| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.on_selection_up(&event, window, cx));
            }
        });
    });
}

struct PinSelectionCapture {
    child: gpui::AnyElement,
    host: WeakEntity<PinnedPopoverPanel>,
    selection: SharedSelection,
}

impl PinSelectionCapture {
    fn new(
        child: gpui::AnyElement,
        host: WeakEntity<PinnedPopoverPanel>,
        selection: SharedSelection,
    ) -> Self {
        Self {
            child,
            host,
            selection,
        }
    }
}

impl Element for PinSelectionCapture {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: gpui::Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        register_pin_selection_listeners(
            window,
            self.host.clone(),
            self.selection.clone(),
            hitbox.clone(),
        );
        self.child.paint(window, cx);
    }
}

impl IntoElement for PinSelectionCapture {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Focusable for PinnedPopoverPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PinnedPopoverPanel {}

impl Render for PinnedPopoverPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.message_image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        self.selection.borrow_mut().begin_render();
        self.sync_selection_order();
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let store = PinnedMessagesStore::global(cx);
        let cards = Rc::new(self.pin_cards.clone());
        let loading = store.read(cx).is_loading();
        let clan_id = store.read(cx).clan_id();
        let handle = self.popover_handle.clone();
        let avatar_cache = self.avatar_image_cache.clone();
        let message_cache = self.message_image_cache.clone();
        let ogp_cache = self.ogp_image_cache.clone();
        let selection = self.selection.clone();
        let settings = self.settings.clone();
        let tokens = &theme.tokens;

        if let Some(clan_id) = clan_id {
            ClanMembersStore::global(cx).update(cx, |members, cx| {
                members.ensure_loaded(clan_id, cx);
            });
        }

        let current = self.list_state.item_count();
        if cards.len() > current {
            self.list_state.splice(0..0, cards.len() - current);
        } else if cards.len() < current {
            self.list_state.reset(cards.len());
        }
        let list_state = self.list_state.clone();
        let viewport_h = f32::from(window.viewport_size().height);
        let panel_max_h = (viewport_h - PANEL_MAX_VIEWPORT_OFFSET).max(PANEL_MIN_HEIGHT);
        let host = cx.entity().downgrade();
        let key_listener = cx.listener(Self::on_pin_key);
        let copy_listener = cx.listener(|this, _: &Copy, window, cx| {
            if this.focus_handle.is_focused(window) && this.copy_selection(cx) {
                cx.stop_propagation();
            }
        });

        v_flex()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_action(copy_listener)
            .on_key_down(key_listener)
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                if Shell::global(cx).read(cx).has_modal() {
                    return;
                }
                cx.emit(DismissEvent);
            }))
            .w(px(POPOVER_WIDTH))
            .min_h(px(PANEL_MIN_HEIGHT.min(HEADER_HEIGHT + EMPTY_BODY_HEIGHT)))
            .max_h(px(panel_max_h))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .text_color(tokens.text_theme_message)
            .child(render_header(&theme, &locale))
            .child(PinSelectionCapture::new(
                render_body(
                    cards,
                    loading,
                    theme.clone(),
                    locale,
                    handle,
                    list_state,
                    avatar_cache,
                    message_cache,
                    ogp_cache,
                    selection.clone(),
                    settings,
                    panel_max_h,
                    window,
                    cx,
                )
                .into_any_element(),
                host,
                selection,
            ))
    }
}

fn render_header(theme: &Theme, locale: &str) -> impl IntoElement {
    let tokens = &theme.tokens;
    h_flex()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap_3()
        .px(px(16.))
        .h(px(HEADER_HEIGHT))
        .border_b_1()
        .border_color(tokens.border_primary)
        .bg(tokens.theme_setting_nav)
        .child(
            Icon::new(IconName::PinRight)
                .size_4()
                .text_color(tokens.bg_icon_theme),
        )
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(
                    locale,
                    "channelTopbar.modals.pinnedMessages.title",
                )),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_body(
    cards: Rc<Vec<PinCardVm>>,
    loading: bool,
    theme: Arc<Theme>,
    locale: String,
    popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    list_state: ListState,
    avatar_cache: Entity<LruImageCache>,
    message_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
    selection: SharedSelection,
    settings: Entity<Settings>,
    panel_max_h: f32,
    window: &mut Window,
    cx: &mut Context<PinnedPopoverPanel>,
) -> impl IntoElement {
    let tokens = &theme.tokens;
    let max_body = (panel_max_h - HEADER_HEIGHT).max(EMPTY_BODY_HEIGHT);
    let body_h = if cards.is_empty() {
        EMPTY_BODY_HEIGHT.min(max_body)
    } else {
        LIST_BODY_HEIGHT.min(max_body)
    };

    let body: gpui::AnyElement = if cards.is_empty() {
        let inner = if loading {
            Spinner::new().with_size(Size::Small).into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(tokens.text_secondary)
                .child(mezon_i18n::t(
                    &locale,
                    "channelTopbar.pinnedMessages.emptyTitle",
                ))
                .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .child(inner)
            .into_any_element()
    } else {
        let cards_for_list = cards.clone();
        let theme_for_list = theme.clone();
        let locale_for_list = locale.clone();
        let handle_for_list = popover_handle.clone();
        let avatar_for_list = avatar_cache.clone();
        let message_for_list = message_cache.clone();
        let ogp_for_list = ogp_cache.clone();
        let selection_for_list = selection.clone();
        let settings_for_list = settings.clone();
        div()
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .pl(px(LIST_PAD_X))
            .pr(px(LIST_PAD_X))
            .py(px(LIST_PAD_Y))
            .child(
                list(list_state.clone(), move |ix, _window, _cx| {
                    let Some(vm) = cards_for_list.get(ix) else {
                        return div().into_any_element();
                    };
                    div()
                        .w_full()
                        .pb(px(8.))
                        .child(pin_card(
                            ix,
                            vm,
                            &theme_for_list,
                            &locale_for_list,
                            handle_for_list.clone(),
                            avatar_for_list.clone(),
                            message_for_list.clone(),
                            ogp_for_list.clone(),
                            selection_for_list.clone(),
                            settings_for_list.clone(),
                        ))
                        .into_any_element()
                })
                .size_full(),
            )
            .custom_scrollbars(
                Scrollbars::always_visible(ScrollAxes::Vertical).tracked_scroll_handle(&list_state),
                window,
                cx,
            )
            .into_any_element()
    };

    div()
        .w_full()
        .h(px(body_h))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(tokens.theme_setting_primary)
        .child(body)
}

fn resolve_pin_avatar_urls(
    pin: &PinnedMessage,
    clan_id: Option<mezon_store::ClanId>,
    channel_id: Option<ChannelId>,
    cx: &App,
) -> (Option<SharedString>, Option<SharedString>) {
    let mut avatar_raw =
        resolve_pin_avatar_url(&pin.sender_id, &pin.avatar_url, clan_id, channel_id, cx);
    if avatar_raw.is_empty()
        && let Ok(message_id) = pin.message_id.parse::<MessageId>()
    {
        let store = MessagesStore::global(cx).read(cx);
        if let Some(message) = store
            .messages()
            .iter()
            .find(|m| m.id == message_id)
            .or_else(|| {
                channel_id.and_then(|channel_id| store.message_in_channel(channel_id, message_id))
            })
            && !message.avatar_url.is_empty()
        {
            avatar_raw = message.avatar_url.to_string();
        }
    }

    if !avatar_raw.is_empty() {
        let proxied = crate::util::imgproxy::avatar_url(cx, &avatar_raw);
        (Some(proxied.into()), Some(avatar_raw.into()))
    } else if !pin.avatar_proxied.is_empty() {
        let fallback = (!pin.avatar_url.is_empty()
            && pin.avatar_url != pin.avatar_proxied.as_ref())
        .then(|| SharedString::from(pin.avatar_url.clone()));
        (Some(pin.avatar_proxied.clone()), fallback)
    } else {
        (None, None)
    }
}

fn pin_card(
    index: usize,
    vm: &PinCardVm,
    theme: &Theme,
    locale: &str,
    popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    avatar_cache: Entity<LruImageCache>,
    message_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
    selection: SharedSelection,
    settings: Entity<Settings>,
) -> gpui::AnyElement {
    let tokens = &theme.tokens;
    let group_name = SharedString::from(format!("pin-card-{index}"));
    let sender_color = tokens.text_theme_primary;
    let sender_label = vm.sender_label.clone();
    let avatar_src = vm.avatar_src.clone();
    let avatar_fallback = vm.avatar_fallback.clone();

    let mut avatar = Avatar::new()
        .name(&sender_label)
        .with_size(Size::Small)
        .anonymous(vm.is_anonymous)
        .image_cache(avatar_cache.clone());
    if !vm.is_anonymous {
        if let Some(src) = &avatar_src {
            avatar = avatar.src(src.clone());
        }
        if let Some(fallback) = &avatar_fallback {
            avatar = avatar.fallback_src(fallback.clone());
        }
    }

    let name_row = h_flex()
        .items_center()
        .gap_4()
        .min_w_0()
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(200.))
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(sender_color)
                .overflow_hidden()
                .text_ellipsis()
                .child(sender_label.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(tokens.text_theme_primary)
                .child(format_pin_time(vm.create_time, locale)),
        );

    let poll = vm
        .poll
        .as_deref()
        .map(|poll| (poll, vm.poll_my_vote.as_ref()));
    let content = render_pin_body(
        &vm.pin,
        &vm.text_spans,
        &vm.selectable_text,
        poll,
        theme,
        locale,
        message_cache,
        ogp_cache,
        Some(selection),
        Some(settings),
    );

    let jump_message_id = vm.message_id.clone();
    let jump_handle = popover_handle.clone();
    let jump = Button::new(("pin-jump", index))
        .label(mezon_i18n::t(locale, "channelTopbar.tooltips.jump"))
        .ghost()
        .with_size(Size::XSmall)
        .on_click(move |_: &ClickEvent, _window, cx| {
            if let Ok(jump_target) = jump_message_id.parse::<MessageId>() {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.jump_to_message(jump_target, None, cx);
                });
            }
            jump_handle.hide(cx);
        });

    let pin_id = vm.pin_id.clone();
    let message_id = vm.message_id.clone();
    let sender_label_for_modal = sender_label.clone();
    let locale_owned: SharedString = locale.to_string().into();
    let delete = Button::new(("pin-del", index))
        .label("✕")
        .ghost()
        .with_size(Size::XSmall)
        .on_click(move |_: &ClickEvent, window, cx| {
            ConfirmUnpinMessageModal::open(
                pin_id.clone(),
                message_id.clone(),
                sender_label_for_modal.clone(),
                locale_owned.clone(),
                window,
                cx,
            );
        });

    let actions = h_flex()
        .absolute()
        .top(px(8.))
        .right(px(8.))
        .items_center()
        .gap_1()
        .invisible()
        .group_hover(group_name.clone(), |s| s.visible())
        .child(jump)
        .child(delete);

    h_flex()
        .id(("pin-item", index))
        .group(group_name)
        .relative()
        .w_full()
        .items_start()
        .gap_2()
        .px(px(12.))
        .py(px(12.))
        .rounded(px(4.))
        .border_1()
        .border_color(tokens.border_primary)
        .bg(tokens.bg_active_member_channel)
        .child(avatar)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(name_row)
                .child(content),
        )
        .child(actions)
        .into_any_element()
}

fn render_pin_body(
    pin: &PinnedMessage,
    text_spans: &[MessageSpan],
    selectable_text: &SharedString,
    poll: Option<(&PollData, &[i32])>,
    theme: &Theme,
    locale: &str,
    image_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
    selection: Option<SharedSelection>,
    settings: Option<Entity<Settings>>,
) -> gpui::AnyElement {
    let message_id = pin.message_id.parse::<MessageId>().unwrap_or(MessageId(0));
    let text_body = match poll {
        Some((poll, voted)) => render_poll_card_readonly(
            poll,
            voted,
            theme,
            locale,
            mezon_store::message_time::unix_now_seconds(),
            &image_cache,
        ),
        None => render_pin_text_body(pin, text_spans, selectable_text, theme, selection.clone()),
    };
    let image_preview = pin
        .attachments
        .iter()
        .find(|att| pin_image_attachment_has_src(att))
        .map(|att| {
            render_pin_image_attachment(
                att,
                pin,
                image_cache.clone(),
                settings.clone(),
                selection.clone(),
            )
        });
    let file_preview = pin
        .attachments
        .iter()
        .find(|att| !att.is_image())
        .map(|att| render_pin_file_attachment(att, theme));
    let ogp = pin
        .ogp
        .as_ref()
        .and_then(|ogp| render_ogp_preview(ogp, message_id, theme, ogp_cache));
    let embeds = render_pin_embeds(&pin.embeds, theme, image_cache);

    v_flex()
        .w_full()
        .min_w_0()
        .max_w_full()
        .gap_1()
        .child(text_body)
        .children(ogp)
        .children(image_preview)
        .children(file_preview)
        .children(embeds)
        .into_any_element()
}

fn render_pin_text_body(
    pin: &PinnedMessage,
    text_spans: &[MessageSpan],
    selectable_text: &SharedString,
    theme: &Theme,
    selection: Option<SharedSelection>,
) -> gpui::AnyElement {
    if !text_spans.is_empty() {
        return render_pin_spans(
            text_spans,
            selectable_text,
            &pin.message_id,
            theme,
            selection,
        );
    }
    if let Some(layout) = pin.rich_layout.as_ref()
        && !layout.text.is_empty()
        && !layout.text.contains("```")
    {
        return render_pin_rich_layout_selectable(layout, &pin.message_id, theme, selection);
    }
    if pin.content.is_empty() {
        return div().into_any_element();
    }
    render_pin_plain_selectable(&pin.content, &pin.message_id, theme, selection)
}

fn prepare_pin_text_spans(pin: &PinnedMessage) -> Arc<[MessageSpan]> {
    let expanded = expand_pin_spans(&pin.spans);
    if !expanded.is_empty() {
        return expanded.into();
    }
    if !pin.spans.is_empty() {
        return Arc::clone(&pin.spans);
    }
    if pin.content.is_empty() {
        return Arc::new([]);
    }
    if pin.content.contains('`') || text_has_block_markup(&pin.content) {
        let fallback = expand_pin_spans(&[MessageSpan::Text(pin.content.clone().into())]);
        if !fallback.is_empty() {
            return fallback.into();
        }
    }
    Arc::new([])
}

fn text_has_block_markup(text: &str) -> bool {
    text.contains("```")
        || text
            .split('\n')
            .any(|line| parse_pin_heading_line(line).is_some())
}

fn expand_pin_spans(spans: &[MessageSpan]) -> Vec<MessageSpan> {
    let mut out = Vec::with_capacity(spans.len());
    let mut changed = false;
    for span in spans {
        match span {
            MessageSpan::Text(text) if text.contains('`') || text_has_block_markup(text) => {
                let before = out.len();
                split_pin_plain_text(text, &mut out);
                if out.len() != before + 1
                    || !matches!(out.last(), Some(MessageSpan::Text(t)) if t.as_ref() == text.as_ref())
                {
                    changed = true;
                }
            }
            other => out.push(other.clone()),
        }
    }
    if changed { out } else { Vec::new() }
}

fn split_pin_plain_text(text: &str, out: &mut Vec<MessageSpan>) {
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        if start > 0 {
            split_pin_text_lines(&rest[..start], out);
        }
        let after_open = &rest[start + 3..];
        if let Some(end) = after_open.find("```") {
            let fence_body = &after_open[..end];
            let wrapped = format!("```{fence_body}```");
            let (language, text) = strip_code_fence(&wrapped);
            out.push(MessageSpan::CodeBlock {
                language,
                text: text.into(),
                fenced_source: fence_body.into(),
            });
            rest = &after_open[end + 3..];
        } else {
            split_pin_text_lines(&rest[start..], out);
            return;
        }
    }
    if !rest.is_empty() {
        split_pin_text_lines(rest, out);
    }
}

fn split_pin_text_lines(text: &str, out: &mut Vec<MessageSpan>) {
    if text.is_empty() {
        return;
    }
    let mut buf = String::new();
    for line in text.split('\n') {
        if let Some((level, body)) = parse_pin_heading_line(line) {
            flush_pin_text_buf(&mut buf, out);
            out.push(MessageSpan::Heading {
                level,
                text: body.to_string().into(),
            });
        } else {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
        }
    }
    flush_pin_text_buf(&mut buf, out);
}

fn flush_pin_text_buf(buf: &mut String, out: &mut Vec<MessageSpan>) {
    if buf.is_empty() {
        return;
    }
    out.push(MessageSpan::Text(std::mem::take(buf).into()));
}

fn parse_pin_heading_line(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    let body = rest.trim_start_matches([' ', '\t']);
    if body.len() == rest.len() || body.is_empty() {
        return None;
    }
    Some((hashes as u8, body))
}

fn render_pin_rich_layout(layout: &RichLayout, theme: &Theme) -> gpui::AnyElement {
    render_pin_rich_layout_element(layout, theme)
}

fn pin_is_http_url(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("http://") || text.starts_with("https://")
}

fn pin_canonical_text(pin: &PinnedMessage, spans: &[MessageSpan]) -> SharedString {
    if !spans.is_empty() {
        return pin_spans_canonical_text(spans);
    }
    if let Some(layout) = pin.rich_layout.as_ref()
        && !layout.text.is_empty()
    {
        return layout.text.clone();
    }
    SharedString::from(pin.content.clone())
}

fn pin_spans_canonical_text(spans: &[MessageSpan]) -> SharedString {
    let mut text = String::new();
    for span in spans {
        match span {
            MessageSpan::Text(value)
            | MessageSpan::Bold(value)
            | MessageSpan::Code(value)
            | MessageSpan::CodeBlock { text: value, .. }
            | MessageSpan::Link { text: value, .. }
            | MessageSpan::Mention { display: value, .. }
            | MessageSpan::Emoji { name: value, .. }
            | MessageSpan::Heading { text: value, .. }
            | MessageSpan::Hashtag { display: value, .. }
            | MessageSpan::Canvas { title: value, .. } => text.push_str(value),
        }
    }
    SharedString::from(text)
}

fn pin_inline_row() -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .max_w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(4.))
}

fn pin_selectable_text_chunks(line: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0usize;
    std::iter::from_fn(move || {
        if start >= line.len() {
            return None;
        }

        let mut has_non_whitespace = false;
        let mut previous_was_whitespace = false;
        for (relative_index, character) in line[start..].char_indices() {
            let is_whitespace = character.is_whitespace();
            if !is_whitespace && previous_was_whitespace && has_non_whitespace {
                let end = start + relative_index;
                let range = start..end;
                start = end;
                return Some(range);
            }
            has_non_whitespace |= !is_whitespace;
            previous_was_whitespace = is_whitespace;
        }

        let range = start..line.len();
        start = line.len();
        Some(range)
    })
}

fn pin_split_unbreakable(text: &str) -> Vec<String> {
    const MAX_SEGMENT_LEN: usize = 32;
    let mut parts = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if matches!(
            ch,
            '/' | '-' | '_' | '.' | '?' | '&' | '#' | '=' | '@' | ':'
        ) || buf.chars().count() >= MAX_SEGMENT_LEN
        {
            parts.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

fn pin_push_inline_text_chunks(
    mut row: gpui::Div,
    line: &str,
    start: usize,
    selected: Option<&Range<usize>>,
    segments: &mut Option<Vec<TextSegment>>,
    body_color: gpui::Rgba,
) -> gpui::Div {
    for range in pin_selectable_text_chunks(line) {
        let chunk = &line[range.clone()];
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }
        if chunk.chars().any(char::is_whitespace) || trimmed.chars().count() <= 32 {
            let chunk_start = start + range.start;
            let chunk_end = start + range.end;
            let styled = pin_selectable_segment(chunk, chunk_start, selected);
            push_pin_text_segment(segments, &styled, chunk_start..chunk_end);
            row = row.child(
                div()
                    .text_sm()
                    .line_height(rems(1.25))
                    .text_color(body_color)
                    .child(styled),
            );
            continue;
        }
        let leading = chunk.len() - chunk.trim_start().len();
        let mut part_offset = leading;
        for part in pin_split_unbreakable(trimmed) {
            let part_len = part.len();
            let part_start = start + range.start + part_offset;
            let part_end = part_start + part_len;
            let styled = pin_selectable_segment(&part, part_start, selected);
            push_pin_text_segment(segments, &styled, part_start..part_end);
            row = row.child(
                div()
                    .text_sm()
                    .line_height(rems(1.25))
                    .text_color(body_color)
                    .child(styled),
            );
            part_offset += part_len;
        }
    }
    row
}

fn pin_wrap_selectable_link(
    text: &str,
    base: usize,
    selected: Option<&Range<usize>>,
    segments: &mut Option<Vec<TextSegment>>,
    url: String,
    selection_gate: Option<SharedSelection>,
    link_key: usize,
    link_color: gpui::Rgba,
    full_width: bool,
) -> gpui::AnyElement {
    let mut row = div()
        .id(("pin-wrap-link", link_key))
        .min_w_0()
        .max_w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .cursor_pointer()
        .text_sm()
        .line_height(rems(1.25))
        .text_color(link_color)
        .on_click(move |_, _, cx| {
            if selection_gate
                .as_ref()
                .is_some_and(|state| state.borrow().has_selection())
            {
                return;
            }
            open_message_link(url.clone(), cx);
        });
    if full_width {
        row = row.w_full();
    }
    let mut part_base = 0usize;
    for part in pin_split_unbreakable(text) {
        let part_start = base + part_base;
        let part_end = part_start + part.len();
        let styled = pin_selectable_segment(&part, part_start, selected);
        push_pin_text_segment(segments, &styled, part_start..part_end);
        row = row.child(styled);
        part_base += part.len();
    }
    row.into_any_element()
}

fn pin_selectable_segment(text: &str, base: usize, selected: Option<&Range<usize>>) -> StyledText {
    let Some(selected) = selected else {
        return StyledText::new(text.to_string());
    };
    let start = selected.start.max(base);
    let end = selected.end.min(base + text.len());
    if start >= end {
        return StyledText::new(text.to_string());
    }
    StyledText::new(text.to_string()).with_highlights(merge_selection_background(
        &[],
        start - base..end - base,
        rgba(SELECTION_BG).into(),
    ))
}

fn push_pin_text_segment(
    segments: &mut Option<Vec<TextSegment>>,
    styled: &StyledText,
    range: Range<usize>,
) {
    if let Some(segments) = segments {
        segments.push(TextSegment::text(styled.layout().clone(), range));
    }
}

fn register_pin_text_layout(message_id: &str, layout: TextLayout, selection: &SharedSelection) {
    let Ok(message_id) = message_id.parse::<MessageId>() else {
        return;
    };
    let mut state = selection.borrow_mut();
    state.registry.insert(message_id, layout);
    if !state.order_map.contains_key(&message_id) {
        let next = state.order_map.len();
        state.order_map.insert(message_id, next);
    }
}

fn ensure_pin_order(message_id: MessageId, selection: &SharedSelection) {
    let mut state = selection.borrow_mut();
    if !state.order_map.contains_key(&message_id) {
        let next = state.order_map.len();
        state.order_map.insert(message_id, next);
    }
}

fn render_pin_selectable_styled(
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
    message_id: &str,
    theme: &Theme,
    selection: Option<SharedSelection>,
) -> gpui::AnyElement {
    let body_color = theme.tokens.text_theme_message;
    let selected = selection.as_ref().and_then(|state| {
        let message_id = message_id.parse::<MessageId>().ok()?;
        state.borrow().range_for_message(message_id, &text)
    });
    let styled = if let Some(range) = selected {
        StyledText::new(text.clone()).with_highlights(merge_selection_background(
            &highlights,
            range,
            rgba(SELECTION_BG).into(),
        ))
    } else if highlights.is_empty() {
        StyledText::new(text.clone())
    } else {
        StyledText::new(text.clone()).with_highlights(highlights)
    };

    if let Some(selection) = &selection {
        register_pin_text_layout(message_id, styled.layout().clone(), selection);
    }

    let content = if links.is_empty() {
        styled.into_any_element()
    } else {
        let link_ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
        let actions: Arc<[(Range<usize>, SharedString)]> = links
            .into_iter()
            .map(|(range, url)| (range, SharedString::from(url)))
            .collect::<Vec<_>>()
            .into();
        let selection_gate = selection.clone();
        InteractiveText::new(SharedString::from(format!("pin-text-{message_id}")), styled)
            .on_click(link_ranges, move |range_ix, _, cx| {
                if selection_gate
                    .as_ref()
                    .is_some_and(|state| state.borrow().has_selection())
                {
                    return;
                }
                if let Some((_, url)) = actions.get(range_ix) {
                    open_message_link(url.to_string(), cx);
                }
            })
            .into_any_element()
    };

    div()
        .w_full()
        .min_w_0()
        .max_w_full()
        .cursor_text()
        .text_sm()
        .line_height(rems(1.25))
        .text_color(body_color)
        .child(content)
        .into_any_element()
}

fn render_pin_spans(
    spans: &[MessageSpan],
    selectable_text: &SharedString,
    message_id: &str,
    theme: &Theme,
    selection: Option<SharedSelection>,
) -> gpui::AnyElement {
    let link_color = theme.tokens.mention_color;
    let mention_bg = theme.tokens.mention_primary;
    let mention_color = theme.tokens.mention_color;
    let code_bg = theme.tokens.bg_markdown_code;
    let body_color = theme.tokens.text_theme_message;
    let msg_id = message_id.parse::<MessageId>().ok();
    let selected = selection.as_ref().and_then(|state| {
        let message_id = msg_id?;
        state
            .borrow()
            .range_for_message(message_id, selectable_text)
    });
    let mut segments = selection.as_ref().zip(msg_id).map(|(state, message_id)| {
        ensure_pin_order(message_id, state);
        state
            .borrow_mut()
            .take_segment_buffer(message_id, selectable_text.clone())
    });

    let mut col = v_flex().w_full().min_w_0().max_w_full().cursor_text();
    let mut row = pin_inline_row();
    let mut has_inline = false;
    let mut link_key = 0usize;
    let mut code_key = 0usize;
    let mut base = 0usize;

    for span in spans {
        match span {
            MessageSpan::Text(text) => {
                let mut line_base = 0usize;
                for (line_index, line) in text.split('\n').enumerate() {
                    if line_index > 0 {
                        if has_inline {
                            col = col.child(row);
                            row = pin_inline_row();
                            has_inline = false;
                        } else if line.is_empty() {
                            col = col.child(div().w_full().h(px(8.)));
                            line_base += 1;
                            continue;
                        }
                        line_base += 1;
                    }
                    if line.is_empty() {
                        continue;
                    }
                    let start = base + line_base;
                    let end = start + line.len();
                    if pin_is_http_url(line) {
                        if has_inline {
                            col = col.child(row);
                            row = pin_inline_row();
                            has_inline = false;
                        }
                        let url = line.to_string();
                        let selection_gate = selection.clone();
                        let key = link_key;
                        link_key += 1;
                        col = col.child(pin_wrap_selectable_link(
                            line,
                            start,
                            selected.as_ref(),
                            &mut segments,
                            url,
                            selection_gate,
                            key,
                            link_color,
                            true,
                        ));
                    } else if has_inline {
                        row = pin_push_inline_text_chunks(
                            row,
                            line,
                            start,
                            selected.as_ref(),
                            &mut segments,
                            body_color,
                        );
                    } else {
                        let styled = pin_selectable_segment(line, start, selected.as_ref());
                        push_pin_text_segment(&mut segments, &styled, start..end);
                        col = col.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_sm()
                                .line_height(rems(1.25))
                                .text_color(body_color)
                                .child(styled),
                        );
                    }
                    line_base += line.len();
                }
                base += text.len();
            }
            MessageSpan::Bold(text) => {
                has_inline = true;
                let end = base + text.len();
                let styled = pin_selectable_segment(text, base, selected.as_ref());
                push_pin_text_segment(&mut segments, &styled, base..end);
                row = row.child(
                    div()
                        .text_sm()
                        .line_height(rems(1.25))
                        .font_weight(FontWeight::BOLD)
                        .text_color(body_color)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::Code(text) => {
                has_inline = true;
                let end = base + text.len();
                let styled = pin_selectable_segment(text, base, selected.as_ref());
                push_pin_text_segment(&mut segments, &styled, base..end);
                row = row.child(
                    div()
                        .text_sm()
                        .line_height(rems(1.25))
                        .px_1()
                        .rounded_sm()
                        .bg(code_bg)
                        .text_color(body_color)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::Link { text, url, .. } => {
                has_inline = true;
                let resolved = resolve_message_link_url(url, text);
                let end = base + text.len();
                let selection_gate = selection.clone();
                let key = link_key;
                link_key += 1;
                row = row.child(pin_wrap_selectable_link(
                    text,
                    base,
                    selected.as_ref(),
                    &mut segments,
                    resolved,
                    selection_gate,
                    key,
                    link_color,
                    false,
                ));
                base = end;
            }
            MessageSpan::Mention { display, .. } | MessageSpan::Hashtag { display, .. } => {
                has_inline = true;
                let end = base + display.len();
                let is_selected = selected
                    .as_ref()
                    .is_some_and(|range| range.start < end && range.end > base);
                let styled = pin_selectable_segment(display, base, None);
                let chip = div()
                    .text_sm()
                    .line_height(rems(1.25))
                    .px_1()
                    .rounded_sm()
                    .bg(mention_bg)
                    .text_color(mention_color)
                    .child(styled);
                if let Some(segments) = segments.as_mut() {
                    let bounds = Rc::new(Cell::new(None));
                    segments.push(TextSegment::bounded(base..end, bounds.clone()));
                    row = row.child(SelectableRegion::new(
                        chip.into_any_element(),
                        bounds,
                        is_selected.then(|| rgba(SELECTION_BG)),
                    ));
                } else {
                    row = row.child(chip);
                }
                base = end;
            }
            MessageSpan::Emoji { name, .. } => {
                has_inline = true;
                let end = base + name.len();
                let styled = pin_selectable_segment(name, base, selected.as_ref());
                push_pin_text_segment(&mut segments, &styled, base..end);
                row = row.child(
                    div()
                        .text_sm()
                        .line_height(rems(1.25))
                        .text_color(body_color)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::Canvas { title, .. } => {
                has_inline = true;
                let end = base + title.len();
                let styled = pin_selectable_segment(title, base, selected.as_ref());
                push_pin_text_segment(&mut segments, &styled, base..end);
                row = row.child(
                    div()
                        .text_sm()
                        .line_height(rems(1.25))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(link_color)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::Heading { level, text } => {
                if has_inline {
                    col = col.child(row);
                    row = pin_inline_row();
                    has_inline = false;
                }
                let end = base + text.len();
                let styled = pin_selectable_segment(text, base, selected.as_ref());
                push_pin_text_segment(&mut segments, &styled, base..end);
                col = col.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .max_w_full()
                        .overflow_hidden()
                        .my(px(2.))
                        .text_size(heading_size(*level))
                        .line_height(heading_line_height(*level))
                        .font_weight(FontWeight::BOLD)
                        .text_color(body_color)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::CodeBlock {
                text,
                fenced_source,
                ..
            } => {
                if has_inline {
                    col = col.child(row);
                    row = pin_inline_row();
                    has_inline = false;
                }
                let end = base + text.len();
                let mut code_col = v_flex().w_full().min_w_0().overflow_hidden();
                let mut line_base = 0usize;
                for (i, line) in text.split('\n').enumerate() {
                    if i > 0 {
                        line_base += 1;
                    }
                    if i > 0 && line.is_empty() {
                        code_col = code_col.child(div().w_full().h(px(8.)));
                        continue;
                    }
                    let start = base + line_base;
                    let line_end = start + line.len();
                    let styled = pin_selectable_segment(line, start, selected.as_ref());
                    push_pin_text_segment(&mut segments, &styled, start..line_end);
                    code_col = code_col.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(14.))
                            .line_height(rems(1.25))
                            .text_color(body_color)
                            .child(styled),
                    );
                    line_base += line.len();
                }
                let copy_id = SharedString::from(format!("pin-code-copy-{message_id}-{code_key}"));
                code_key += 1;
                col = col.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .max_w_full()
                        .overflow_hidden()
                        .mt(px(4.))
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.tokens.border_primary)
                        .bg(code_bg)
                        .child(code_col)
                        .child(code_block_copy_overlay(
                            copy_id,
                            fenced_source.clone(),
                            theme,
                        )),
                );
                base = end;
            }
        }
    }
    if has_inline {
        col = col.child(row);
    }
    if let (Some(selection), Some(message_id), Some(segments)) =
        (selection.as_ref(), msg_id, segments)
    {
        selection
            .borrow_mut()
            .store_segment_buffer(message_id, selectable_text.clone(), segments);
    }
    col.into_any_element()
}

fn render_pin_plain_selectable(
    content: &str,
    message_id: &str,
    theme: &Theme,
    selection: Option<SharedSelection>,
) -> gpui::AnyElement {
    if pin_is_http_url(content) && selection.is_none() {
        return pin_link_element(content, content, theme.tokens.mention_color, true, 0);
    }
    let mut highlights = Vec::new();
    let mut links = Vec::new();
    if pin_is_http_url(content) {
        let link_color: gpui::Hsla = theme.tokens.mention_color.into();
        highlights.push((
            0..content.len(),
            HighlightStyle {
                color: Some(link_color),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(link_color),
                    wavy: false,
                }),
                ..Default::default()
            },
        ));
        links.push((0..content.len(), content.to_string()));
    }
    render_pin_selectable_styled(
        SharedString::from(content.to_string()),
        highlights,
        links,
        message_id,
        theme,
        selection,
    )
}

fn render_pin_rich_layout_selectable(
    layout: &RichLayout,
    message_id: &str,
    theme: &Theme,
    selection: Option<SharedSelection>,
) -> gpui::AnyElement {
    if selection.is_none() {
        return render_pin_rich_layout(layout, theme);
    }
    render_pin_selectable_styled(
        layout.text.clone(),
        Vec::new(),
        Vec::new(),
        message_id,
        theme,
        selection,
    )
}

fn pin_image_attachment_has_src(att: &MessageAttachment) -> bool {
    att.is_image() && (!att.proxied_src.is_empty() || !att.url.is_empty())
}

fn render_pin_image_attachment(
    att: &MessageAttachment,
    pin: &PinnedMessage,
    image_cache: Entity<LruImageCache>,
    settings: Option<Entity<Settings>>,
    selection: Option<SharedSelection>,
) -> gpui::AnyElement {
    let src = if att.proxied_src.is_empty() {
        SharedString::from(att.url.clone())
    } else {
        att.proxied_src.clone()
    };
    let seed = AttachmentSeedInput::from_message(att);
    let message_id = pin.message_id.parse::<MessageId>().unwrap_or(MessageId(0));
    let create_time = pin.create_time;
    let uploader_id = pin
        .sender_id
        .parse::<i64>()
        .ok()
        .map(UserId)
        .unwrap_or(UserId(0));
    let image_id = SharedString::from(format!("pin-image-{}", att.url));
    let can_open = settings.is_some();

    div()
        .id(image_id)
        .mt_1()
        .w(px(ATTACHMENT_PREVIEW_SIZE))
        .h(px(ATTACHMENT_PREVIEW_SIZE))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(4.))
        .when(can_open, |el| el.cursor_pointer())
        .child(
            img(src)
                .image_cache(&image_cache)
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        )
        .when(can_open, |el| {
            el.on_click(move |_: &ClickEvent, window, cx| {
                cx.stop_propagation();
                if selection
                    .as_ref()
                    .is_some_and(|state| state.borrow().has_selection())
                {
                    return;
                }
                let Some(settings) = settings.clone() else {
                    return;
                };
                open_viewer_from_message(
                    &settings,
                    seed.clone(),
                    message_id,
                    create_time,
                    uploader_id,
                    window,
                    cx,
                );
            })
        })
        .into_any_element()
}

fn render_pin_file_attachment(att: &MessageAttachment, theme: &Theme) -> gpui::AnyElement {
    let filename = if att.filename.is_empty() {
        SharedString::from("Attachment")
    } else {
        SharedString::from(att.filename.clone())
    };
    let size_line = if att.size_label.is_empty() {
        SharedString::from(format!("size: {}", mezon_store::format_file_size(att.size)))
    } else {
        SharedString::from(format!("size: {}", att.size_label))
    };
    let icon = file_type_icon_for(&att.filetype, &att.filename);
    let file_id = SharedString::from(att.url.clone());
    let download_url = file_id.clone();
    let download_name = filename.clone();
    let open_url = download_url.clone();
    let open_name = download_name.clone();
    let group_name = SharedString::from(format!("pin-file-{}", att.url));
    let body_id = group_name.clone();
    let dl_id = SharedString::from(format!("pin-file-dl-{}", att.url));

    div()
        .id(group_name.clone())
        .group(group_name.clone())
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .w_full()
        .max_w_full()
        .min_w_0()
        .mt(px(10.))
        .p_3()
        .rounded_lg()
        .bg(theme.tokens.bg_item_theme_hover)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .overflow_hidden()
        .child(
            div()
                .relative()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(40.))
                .child(img(icon.path()).w(px(32.)).h(px(40.)).flex_none()),
        )
        .child(
            div()
                .id(body_id)
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .cursor_pointer()
                .on_click(move |_: &ClickEvent, _window, cx| {
                    save_with_progress_toast(open_url.clone(), open_name.clone(), cx);
                })
                .child(
                    div()
                        .truncate()
                        .text_size(px(16.))
                        .text_color(gpui::rgb(FILE_NAME_COLOR))
                        .hover(|s| s.underline())
                        .child(filename),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.tokens.text_theme_primary)
                        .child(size_line),
                ),
        )
        .child(
            div()
                .absolute()
                .right(px(12.))
                .top_0()
                .bottom_0()
                .flex()
                .items_center()
                .opacity(0.)
                .group_hover(group_name, |s| s.opacity(1.))
                .child(
                    div()
                        .id(dl_id)
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(32.))
                        .rounded_md()
                        .bg(theme.tokens.bg_theme_contexify)
                        .border_1()
                        .border_color(theme.tokens.border_primary)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.8))
                        .on_click(move |_: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            save_with_progress_toast(
                                download_url.clone(),
                                download_name.clone(),
                                cx,
                            );
                        })
                        .child(
                            Icon::new(IconName::Download)
                                .size(px(16.))
                                .text_color(theme.tokens.text_theme_primary),
                        ),
                ),
        )
        .into_any_element()
}

fn render_pin_embeds(
    embeds: &[Embed],
    theme: &Theme,
    image_cache: Entity<LruImageCache>,
) -> Vec<gpui::AnyElement> {
    embeds
        .iter()
        .filter(|embed| {
            !embed.title.is_empty()
                || !embed.description_spans.is_empty()
                || !embed.thumbnail_proxied.is_empty()
        })
        .map(|embed| {
            let description = embed
                .description_spans
                .iter()
                .filter_map(|span| match span {
                    MessageSpan::Text(text) | MessageSpan::Bold(text) | MessageSpan::Code(text) => {
                        Some(text.as_ref())
                    }
                    MessageSpan::Link { text, .. }
                    | MessageSpan::Mention { display: text, .. }
                    | MessageSpan::Hashtag { display: text, .. } => Some(text.as_ref()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            let mut card = v_flex()
                .w_full()
                .min_w_0()
                .mt_1()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.theme_setting_primary);
            if !embed.title.is_empty() {
                card = card.child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_message)
                        .child(embed.title.clone()),
                );
            }
            if !description.is_empty() {
                card = card.child(
                    div()
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(description),
                );
            }
            if !embed.thumbnail_proxied.is_empty() {
                card = card.child(
                    div()
                        .w(px(ATTACHMENT_PREVIEW_SIZE))
                        .h(px(ATTACHMENT_PREVIEW_SIZE))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(px(4.))
                        .child(
                            img(embed.thumbnail_proxied.clone())
                                .image_cache(&image_cache)
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
                        ),
                );
            }
            card.into_any_element()
        })
        .collect()
}

/// Format a pin's create time (unix seconds): Today at HH:MM, Yesterday at HH:MM, otherwise dd/MM/yyyy, HH:MM (in the local timezone).
fn format_pin_time(create_time: i64, locale: &str) -> String {
    let Some(utc) = chrono::DateTime::from_timestamp(create_time, 0) else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    let date = local.date_naive();
    let today = chrono::Local::now().date_naive();
    let time = local.format("%H:%M").to_string();

    if date == today {
        format!("{} {}", mezon_i18n::t(locale, "common.todayAt"), time)
    } else if Some(date) == today.pred_opt() {
        format!("{} {}", mezon_i18n::t(locale, "common.yesterdayAt"), time)
    } else {
        local.format("%d/%m/%Y, %H:%M").to_string()
    }
}

pub fn pin_popover_on_open() -> Rc<dyn Fn(&mut Window, &mut App)> {
    Rc::new(|_window, cx| {
        PinnedMessagesStore::global(cx).update(cx, |store, cx| {
            store.clear_active_pin_badge(cx);
            store.ensure_loaded(cx);
            if let Some(clan_id) = store.clan_id() {
                ClanMembersStore::global(cx).update(cx, |members, cx| {
                    members.ensure_loaded(clan_id, cx);
                });
            }
        });
    })
}
