use std::path::PathBuf;

use gpui::{
    AnyView, App, Context, Entity, ExternalPaths, FontWeight, SharedString, StyleRefinement,
    Subscription, Window, div, prelude::*, px, rgb, rgba,
};
use mezon_store::{ChannelId, ClanId, InVoiceInfo, MessagesEvent, MessagesStore, Settings};
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
use crate::chat::message::ChannelMessages;
use crate::chat::message_search::{MESSAGE_SEARCH_PANEL_WIDTH, MessageSearchPanel};
use crate::chat::pinned_popover::PinnedPopoverPanel;
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
    _submit_sub: Option<Subscription>,
    _reply_sub: Option<Subscription>,
    drop_title_cache: Option<(SharedString, SharedString, SharedString)>,
    drop_body_cache: Option<(SharedString, SharedString)>,
}

impl ChatArea {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<crate::ChatLayout>) -> Self {
        let timeline = cx.new({
            let settings = settings.clone();
            move |cx| ChannelMessages::new(settings, cx)
        });
        let layout = cx.weak_entity();
        let header = cx.new(|cx| ChatHeader::new(layout, &settings, cx));
        let typing = cx.new(|cx| ChannelTyping::new(&settings, cx));
        let member_avatar_cache = crate::image_cache::shared_avatar_cache(cx);
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
            _submit_sub: None,
            _reply_sub: None,
            drop_title_cache: None,
            drop_body_cache: None,
        }
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
                },
            );
            self._submit_sub = Some(submit_sub);
            self.input_bar = Some(cx.new(|cx| {
                InputBar::new(
                    mention_input.clone(),
                    SharedString::from(locale.clone()),
                    &self.settings,
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
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_canvas(
        &mut self,
        locale: &str,
        channel_name: Option<&str>,
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
                col.on_drop(
                    move |paths: &ExternalPaths, window: &mut Window, cx: &mut App| {
                        if let Some(drop_input) = drop_input.clone() {
                            let dropped: Vec<PathBuf> = paths.paths().to_vec();
                            drop_input.update(cx, |input, cx| {
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
                col.child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        AnyView::from(self.timeline.clone())
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
                .when_some(app_channel_bar.as_ref(), |col, target| {
                    col.child(render_channel_app_bar(
                        locale,
                        target.clone(),
                        cx.theme(),
                        cx,
                    ))
                })
                .when(app_channel_bar.is_none(), |col| {
                    col.when_some(input_bar.clone(), |col, input_bar| col.child(input_bar))
                })
                .child(
                    AnyView::from(self.typing.clone()).cached(
                        StyleRefinement::default()
                            .w_full()
                            .h(px(16.))
                            .flex_shrink_0(),
                    ),
                )
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
                        .child(
                            AnyView::from(panel)
                                .cached(StyleRefinement::default().w(px(245.)).h_full()),
                        ),
                )
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
            .child(header)
            .child(body)
            .into_any_element()
    }
}
