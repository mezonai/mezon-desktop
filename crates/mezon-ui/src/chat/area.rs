use crate::app::shell::Shell;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyView, App, Context, Entity, ExternalPaths, FontWeight, SharedString, StyleRefinement,
    Subscription, Task, Window, div, prelude::*, px, rgb, rgba,
};
use mezon_store::{
    ChannelId, ChannelPermissionsStore, ClanId, InVoiceInfo, MessagesEvent, MessagesStore,
    PERMISSION_SEND_MESSAGE, Settings,
};
use ui::PopoverMenuHandle;

use crate::chat::CanvasPopoverPanel;
use crate::chat::ReplyTarget;
use crate::chat::channel_app_bar::{ChannelAppBarTarget, render_channel_app_bar};
use crate::chat::channel_header::ChatHeader;
use crate::chat::channel_typing::ChannelTyping;
use crate::chat::inbox::InboxPopoverPanel;
use crate::chat::input_bar::{InputBar, ReplyClearSource};
use crate::chat::media_channel::MediaChannelPanel;
use crate::chat::member_list::{MemberListPanel, MemberSource};
use crate::chat::mention_input::{MentionInput, MentionInputEvent};
use crate::chat::message::{ChannelMessages, ChannelMessagesEvent};
use crate::chat::message_search::{MESSAGE_SEARCH_PANEL_WIDTH, MessageSearchPanel};
use crate::chat::pinned_popover::PinnedPopoverPanel;
use crate::components::compositions::channel_row::ChannelIcon;
use crate::components::primitives::{Icon, IconName, InputState};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;

pub struct ChatArea {
    pub(crate) timeline: Entity<ChannelMessages>,
    pub(crate) mention_input: Option<Entity<MentionInput>>,
    input_bar: Option<Entity<InputBar>>,
    member_panel: Option<Entity<MemberListPanel>>,
    member_source: Option<MemberSource>,
    member_avatar_cache: Entity<LruImageCache>,
    settings: Entity<Settings>,
    header: Entity<ChatHeader>,
    typing: Entity<ChannelTyping>,
    media_channel_panel: Option<Entity<MediaChannelPanel>>,
    media_channel_context: Option<(ClanId, ChannelId)>,
    replying_to: Option<ReplyTarget>,
    send_permission_key: Option<(ClanId, ChannelId)>,
    send_permission_live: Option<bool>,
    can_send_message: Option<bool>,
    no_permission_label: Option<(SharedString, SharedString)>,
    _submit_sub: Option<Subscription>,
    _reply_sub: Option<Subscription>,
    _edit_closed_sub: Option<Subscription>,
    _send_permission_sub: Subscription,
    _send_permission_channel_sub: Subscription,
    _send_permission_debounce: Option<Task<()>>,
    drop_title_cache: Option<(SharedString, SharedString, SharedString)>,
    drop_body_cache: Option<(SharedString, SharedString)>,
}

const SEND_PERMISSION_DEBOUNCE: Duration = Duration::from_millis(500);

impl ChatArea {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<crate::ChatLayout>) -> Self {
        let timeline = cx.new({
            let settings = settings.clone();
            move |cx| ChannelMessages::new(settings, cx)
        });
        ChannelMessages::register_as_active_timeline(&timeline, cx);
        let layout = cx.weak_entity();
        let header = cx.new(|cx| ChatHeader::new(layout, &settings, cx));
        let typing = cx.new(|cx| ChannelTyping::new(&settings, cx));
        let member_avatar_cache = crate::image_cache::shared_avatar_cache(cx);
        let send_permission_sub = cx.subscribe(
            &ChannelPermissionsStore::global(cx),
            |this: &mut crate::ChatLayout, _, _event: &mezon_store::ChannelPermissionsEvent, cx| {
                this.chat_area.sync_send_permission(cx);
            },
        );
        let send_permission_channel_sub = cx.subscribe(
            &MessagesStore::global(cx),
            |this: &mut crate::ChatLayout, _, event: &mezon_store::MessagesEvent, cx| {
                if matches!(event, mezon_store::MessagesEvent::Reset { .. }) {
                    this.chat_area.sync_send_permission(cx);
                }
            },
        );
        Self {
            timeline,
            mention_input: None,
            input_bar: None,
            member_panel: None,
            member_source: None,
            member_avatar_cache,
            settings,
            header,
            typing,
            media_channel_panel: None,
            media_channel_context: None,
            replying_to: None,
            send_permission_key: None,
            send_permission_live: None,
            can_send_message: None,
            no_permission_label: None,
            _submit_sub: None,
            _reply_sub: None,
            _edit_closed_sub: None,
            _send_permission_sub: send_permission_sub,
            _send_permission_channel_sub: send_permission_channel_sub,
            _send_permission_debounce: None,
            drop_title_cache: None,
            drop_body_cache: None,
        }
    }

    pub fn sync_send_permission(&mut self, cx: &mut Context<crate::ChatLayout>) {
        let key = {
            let messages = MessagesStore::global(cx).read(cx);
            if messages.is_dm() {
                None
            } else {
                messages
                    .active_clan_id()
                    .filter(|clan_id| !clan_id.is_zero())
                    .zip(messages.active_channel_id())
            }
        };
        let live = key.and_then(|(clan_id, channel_id)| {
            ChannelPermissionsStore::try_global(cx).and_then(|store| {
                store
                    .read(cx)
                    .permission_value(PERMISSION_SEND_MESSAGE, clan_id, channel_id)
            })
        });
        if key == self.send_permission_key && live == self.send_permission_live {
            return;
        }
        let switched = key != self.send_permission_key;
        self.send_permission_key = key;
        self.send_permission_live = live;
        if switched {
            self._send_permission_debounce = None;
            if self.can_send_message != live {
                self.can_send_message = live;
                cx.notify();
            }
            return;
        }
        self._send_permission_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SEND_PERMISSION_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, cx| {
                let live = this.chat_area.send_permission_live;
                if this.chat_area.can_send_message != live {
                    this.chat_area.can_send_message = live;
                    cx.notify();
                }
            });
        }));
    }

    pub fn bind_channel_members(&mut self, cx: &mut Context<crate::ChatLayout>) {
        self.set_member_source(Some(MemberSource::Channel), cx);
    }

    pub fn bind_group_members(&mut self, cx: &mut Context<crate::ChatLayout>) {
        self.set_member_source(Some(MemberSource::Group), cx);
    }

    pub fn clear_member_panel(&mut self) {
        self.member_source = None;
        self.member_panel = None;
    }

    fn set_member_source(
        &mut self,
        source: Option<MemberSource>,
        cx: &mut Context<crate::ChatLayout>,
    ) {
        if self.member_source == source {
            return;
        }
        self.member_source = source;
        self.member_panel = source.map(|source| {
            let settings = self.settings.clone();
            let avatar_cache = self.member_avatar_cache.clone();
            cx.new(move |cx| MemberListPanel::new(source, settings, avatar_cache, cx))
        });
    }

    pub fn bind_window(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        self.timeline
            .update(cx, |timeline, cx| timeline.bind_window(window, cx));
        if let Some(panel) = self.media_channel_panel.clone() {
            panel.update(cx, |panel, cx| panel.bind_window(window, cx));
        }
    }

    pub fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        if self.mention_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let placeholder = mezon_i18n::t(&locale, "messageBox.placeholder");
            let settings = self.settings.clone();
            let mention_input = cx.new(|cx| MentionInput::new(placeholder, settings, window, cx));
            MentionInput::register_as_active_composer(&mention_input, cx);
            let submit_sub = cx.subscribe_in(
                &mention_input,
                window,
                |this: &mut crate::ChatLayout, _, event: &MentionInputEvent, window, cx| match event
                {
                    MentionInputEvent::Submit => this.send_current_message(window, cx),
                    MentionInputEvent::Cancel => {
                        MessagesStore::global(cx).update(cx, |store, cx| store.clear_reply(cx));
                    }
                    MentionInputEvent::SendSticker { url, filename } => {
                        this.send_sticker(url.clone(), filename.clone(), cx)
                    }
                    MentionInputEvent::SendGif { url, width, height } => {
                        this.send_gif(url.clone(), *width, *height, cx)
                    }
                    MentionInputEvent::SendSound { url, filename } => {
                        this.send_sound(url.clone(), filename.clone(), cx)
                    }
                    MentionInputEvent::EditLastMessage => {
                        let timeline = this.chat_area.timeline.clone();
                        timeline.update(cx, |timeline, cx| {
                            timeline.edit_last_own_message(window, cx)
                        });
                    }
                },
            );
            self._submit_sub = Some(submit_sub);
            self.input_bar = Some(cx.new(|cx| {
                InputBar::new(
                    mention_input.clone(),
                    SharedString::from(locale.clone()),
                    &self.settings,
                    false,
                    cx,
                )
            }));
            self.mention_input = Some(mention_input);
            self.replying_to = MessagesStore::global(cx)
                .read(cx)
                .reply_target()
                .map(|draft| ReplyTarget {
                    sender_name: SharedString::from(draft.sender_name.clone()),
                });

            let reply_sub = cx.subscribe_in(
                &MessagesStore::global(cx),
                window,
                |this: &mut crate::ChatLayout, store, event: &MessagesEvent, window, cx| {
                    if matches!(event, MessagesEvent::SendFailedWithoutRow) {
                        let locale = this.chat_area.settings.read(cx).language.clone();
                        let message =
                            SharedString::from(mezon_i18n::t(&locale, "message.toast.sendFailed"));
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        return;
                    }
                    if matches!(event, MessagesEvent::ReplyTargetChanged) {
                        let replying_to = store.read(cx).reply_target().map(|draft| ReplyTarget {
                            sender_name: SharedString::from(draft.sender_name.clone()),
                        });
                        if replying_to.is_some()
                            && let Some(input) = this.chat_area.mention_input.clone()
                        {
                            window.defer(cx, move |window, cx| {
                                input.update(cx, |input, cx| input.focus_input(window, cx));
                            });
                        }
                        this.chat_area.replying_to = replying_to;
                        cx.notify();
                    }
                },
            );
            self._reply_sub = Some(reply_sub);

            let edit_closed_sub = cx.subscribe_in(
                &self.timeline,
                window,
                |this: &mut crate::ChatLayout, _, event: &ChannelMessagesEvent, window, cx| {
                    let ChannelMessagesEvent::EditClosed = event;
                    if this.chat_area.can_send_message == Some(false) {
                        return;
                    }
                    if let Some(input) = this.chat_area.mention_input.clone() {
                        window.defer(cx, move |window, cx| {
                            input.update(cx, |input, cx| input.focus_input(window, cx));
                        });
                    }
                },
            );
            self._edit_closed_sub = Some(edit_closed_sub);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_canvas(
        &mut self,
        locale: &str,
        channel_name: Option<&str>,
        channel_icon: Option<ChannelIcon>,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        canvas_body: gpui::AnyElement,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name,
                channel_icon,
                false,
                None,
                show_members_button,
                show_member_panel,
                show_inbox,
                inbox_handle,
                clan_id,
                pin_handle,
                canvas_handle,
                false,
                false,
                show_search_bar,
                search_expanded,
                show_search_options,
                search_input,
                false,
                Some(locale),
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        let header = AnyView::from(self.header.clone()).cached(
            StyleRefinement::default()
                .w_full()
                .h(px(crate::app::window_controls::APP_HEADER_HEIGHT))
                .flex_shrink_0(),
        );

        let canvas_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(canvas_body);

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .child(canvas_column)
            .when(show_member_panel, |row| match &self.member_panel {
                Some(panel) => row.child(
                    AnyView::from(panel.clone()).cached(
                        StyleRefinement::default()
                            .w(px(245.))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                ),
                None => row.child(div().w(px(245.)).h_full().flex_shrink_0()),
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        locale: &str,
        channel_name: Option<&str>,
        channel_icon: Option<ChannelIcon>,
        is_dm: bool,
        in_voice: Option<InVoiceInfo>,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        show_inbox: bool,
        timeline_action: bool,
        timeline_active: bool,
        media_channel_view: bool,
        media_clan_id: Option<ClanId>,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        show_results_panel: bool,
        message_search_panel: Option<Entity<MessageSearchPanel>>,
        app_channel_bar: Option<ChannelAppBarTarget>,
        stream_sidebar: bool,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        let (input_bar, mention_input) = if media_channel_view {
            (None, None)
        } else {
            match (self.input_bar.clone(), self.mention_input.clone()) {
                (Some(input_bar), Some(mention_input)) => (Some(input_bar), Some(mention_input)),
                _ => {
                    return div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .into_any_element();
                }
            }
        };

        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name,
                channel_icon,
                is_dm,
                in_voice,
                show_members_button,
                show_member_panel,
                show_inbox,
                inbox_handle,
                clan_id,
                pin_handle,
                canvas_handle,
                timeline_action,
                timeline_active,
                show_search_bar,
                search_expanded,
                show_search_options,
                search_input,
                stream_sidebar,
                Some(locale),
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        if media_channel_view && let (Some(clan_id), Some(channel_id)) = (media_clan_id, channel_id)
        {
            let needs_new = self.media_channel_context != Some((clan_id, channel_id));
            if needs_new || self.media_channel_panel.is_none() {
                self.media_channel_context = Some((clan_id, channel_id));
                self.media_channel_panel = Some(cx.new(|cx| {
                    MediaChannelPanel::new(clan_id, channel_id, self.settings.clone(), cx)
                }));
                if let Some(panel) = self.media_channel_panel.clone() {
                    panel.update(cx, |panel, cx| panel.on_enter(cx));
                }
            } else if let Some(panel) = self.media_channel_panel.clone() {
                panel.update(cx, |panel, cx| {
                    panel.sync_channel(clan_id, channel_id, cx);
                });
            }
        } else {
            self.media_channel_panel = None;
            self.media_channel_context = None;
        }

        if let Some(input_bar) = input_bar.clone() {
            input_bar.update(cx, |input_bar, cx| {
                input_bar.sync(
                    locale,
                    self.replying_to.clone(),
                    ReplyClearSource::Messages,
                    cx,
                )
            });
        }

        let header = AnyView::from(self.header.clone()).cached(
            StyleRefinement::default()
                .w_full()
                .h(px(crate::app::window_controls::APP_HEADER_HEIGHT))
                .flex_shrink_0(),
        );

        let channel_label = channel_name.unwrap_or_default();
        let drop_title = match &self.drop_title_cache {
            Some((cached_locale, cached_channel, title))
                if cached_locale.as_ref() == locale && cached_channel.as_ref() == channel_label =>
            {
                title.clone()
            }
            _ => {
                let title: SharedString = mezon_i18n::t(locale, "common.uploadToChannel")
                    .replace("{{channelName}}", channel_label)
                    .into();
                self.drop_title_cache = Some((
                    SharedString::from(locale.to_string()),
                    SharedString::from(channel_label.to_string()),
                    title.clone(),
                ));
                title
            }
        };
        let drop_body = match &self.drop_body_cache {
            Some((cached_locale, body)) if cached_locale.as_ref() == locale => body.clone(),
            _ => {
                let body: SharedString = mezon_i18n::t(locale, "common.uploadInstructions").into();
                self.drop_body_cache = Some((SharedString::from(locale.to_string()), body.clone()));
                body
            }
        };
        let drop_overlay = if media_channel_view {
            None
        } else {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .invisible()
                    .group_drag_over::<ExternalPaths>("chat-drop-zone", |style| style.visible())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x000000e6))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(16.))
                            .w(px(400.))
                            .h(px(240.))
                            .rounded(px(8.))
                            .border_2()
                            .border_dashed()
                            .border_color(rgb(0xffffff))
                            .bg(rgb(0x5865f2))
                            .child(
                                Icon::new(IconName::FileAndFolder)
                                    .size(px(48.))
                                    .text_color(rgb(0xffffff)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(18.))
                                    .text_color(rgb(0xffffff))
                                    .child(drop_title),
                            )
                            .child(
                                div()
                                    .px(px(24.))
                                    .text_center()
                                    .text_size(px(14.))
                                    .text_color(rgb(0xffffff))
                                    .child(drop_body),
                            ),
                    ),
            )
        };

        let send_denied = !media_channel_view && self.can_send_message == Some(false);
        let no_permission_notice = if send_denied {
            let label = match &self.no_permission_label {
                Some((cached_locale, label)) if cached_locale.as_ref() == locale => label.clone(),
                _ => {
                    let label: SharedString =
                        mezon_i18n::t(locale, "common.noPermissionToSendMessage").into();
                    self.no_permission_label =
                        Some((SharedString::from(locale.to_string()), label.clone()));
                    label
                }
            };
            let theme = cx.theme();
            div()
                .flex_shrink_0()
                .h(px(44.))
                .ml(px(16.))
                .mr(px(14.))
                .mb(px(16.))
                .py(px(8.))
                .pl(px(8.))
                .rounded(px(4.))
                .opacity(0.8)
                .bg(theme.tokens.bg_tertiary)
                .text_color(theme.tokens.text_theme_primary)
                .overflow_hidden()
                .child(label)
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let timeline_popover_open = self.timeline.read(cx).profile_popover_open();
        let member_popover_open = self
            .member_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).profile_popover_open());

        let message_column = div()
            .relative()
            .group("chat-drop-zone")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .when(!media_channel_view, |col| {
                let drop_input = mention_input;
                let input_visible = !send_denied;
                col.on_drop(
                    move |paths: &ExternalPaths, window: &mut Window, cx: &mut App| {
                        if let Some(drop_input) = drop_input.clone() {
                            let dropped: Vec<PathBuf> = paths.paths().to_vec();
                            drop_input.update(cx, |input, cx| {
                                if input_visible {
                                    input.focus_input(window, cx);
                                }
                                input.add_dropped_paths(dropped, window, cx)
                            });
                        }
                    },
                )
            })
            .when(media_channel_view, |col| {
                if let Some(panel) = self.media_channel_panel.clone() {
                    col.child(div().size_full().child(AnyView::from(panel)))
                } else {
                    col
                }
            })
            .when(!media_channel_view, |col| {
                col.child(div().flex_1().min_h_0().overflow_hidden().child(
                    if timeline_popover_open {
                        div()
                            .size_full()
                            .child(AnyView::from(self.timeline.clone()))
                            .into_any_element()
                    } else {
                        AnyView::from(self.timeline.clone())
                            .cached(StyleRefinement::default().size_full())
                            .into_any_element()
                    },
                ))
                .when(send_denied, |col| col.child(no_permission_notice))
                .when(!send_denied, |col| {
                    col.when_some(input_bar.clone(), |col, input_bar| col.child(input_bar))
                        .when_some(app_channel_bar.as_ref(), |col, target| {
                            col.child(render_channel_app_bar(locale, target.clone(), cx.theme()))
                        })
                        .child(
                            AnyView::from(self.typing.clone()).cached(
                                StyleRefinement::default()
                                    .w_full()
                                    .h(px(16.))
                                    .flex_shrink_0(),
                            ),
                        )
                })
                .when_some(drop_overlay, |col, overlay| col.child(overlay))
            });

        let has_search_panel = show_results_panel && message_search_panel.is_some();
        let member_visible = show_member_panel && !has_search_panel && !media_channel_view;
        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .child(message_column)
            .when_some(message_search_panel, |row, panel| {
                row.child(
                    AnyView::from(panel).cached(
                        StyleRefinement::default()
                            .w(px(MESSAGE_SEARCH_PANEL_WIDTH))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                )
            })
            .when_some(self.member_panel.clone(), |row, panel| {
                row.child(
                    div()
                        .h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .when(member_visible, |slot| slot.w(px(245.)))
                        .when(!member_visible, |slot| slot.w(px(0.)).invisible())
                        .child(if member_popover_open {
                            div()
                                .w(px(245.))
                                .h_full()
                                .child(AnyView::from(panel))
                                .into_any_element()
                        } else {
                            AnyView::from(panel)
                                .cached(StyleRefinement::default().w(px(245.)).h_full())
                                .into_any_element()
                        }),
                )
            });

        let hide_header = is_dm
            && channel_id.is_some_and(|cid| {
                crate::chat::call_window::call_panel_active_for_dm(cid.get(), cx)
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .h_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .when(!hide_header, |this| this.child(header))
            .child(body)
            .into_any_element()
    }
}
