use std::path::PathBuf;

use gpui::{
    AnyView, App, Context, Entity, ExternalPaths, FontWeight, SharedString, StyleRefinement,
    Subscription, Window, div, prelude::*, px, rgb, rgba,
};
use mezon_store::{ChannelId, MessagesEvent, MessagesStore, Settings};
use ui::PopoverMenuHandle;

use crate::chat::ReplyTarget;
use crate::chat::channel_header::ChatHeader;
use crate::chat::channel_typing::ChannelTyping;
use crate::chat::inbox::InboxPopoverPanel;
use crate::chat::input_bar::InputBar;
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
    member_panel: Option<Entity<MemberListPanel>>,
    member_source: Option<MemberSource>,
    member_avatar_cache: Entity<LruImageCache>,
    settings: Entity<Settings>,
    header: Entity<ChatHeader>,
    typing: Entity<ChannelTyping>,
    _submit_sub: Option<Subscription>,
    _reply_sub: Option<Subscription>,
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
            member_panel: None,
            member_source: None,
            member_avatar_cache,
            settings,
            header,
            typing,
            _submit_sub: None,
            _reply_sub: None,
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
                },
            );
            self._submit_sub = Some(submit_sub);
            self.mention_input = Some(mention_input);

            let reply_sub = cx.subscribe_in(
                &MessagesStore::global(cx),
                window,
                |this: &mut crate::ChatLayout, store, event: &MessagesEvent, window, cx| {
                    if matches!(event, MessagesEvent::ReplyTargetChanged) {
                        if store.read(cx).reply_target().is_some()
                            && let Some(input) = this.chat_area.mention_input.clone()
                        {
                            input.update(cx, |input, cx| input.focus_input(window, cx));
                        }
                        cx.notify();
                    }
                },
            );
            self._reply_sub = Some(reply_sub);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        locale: &str,
        channel_name: Option<&str>,
        is_dm: bool,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        show_results_panel: bool,
        message_search_panel: Option<Entity<MessageSearchPanel>>,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        let mention_input = match self.mention_input.clone() {
            Some(s) => s,
            None => {
                return div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .into_any_element();
            }
        };

        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name.map(SharedString::from),
                is_dm,
                show_members_button,
                show_member_panel,
                show_inbox,
                inbox_handle,
                clan_id,
                pin_handle,
                show_search_bar,
                search_expanded,
                show_search_options,
                search_input,
                Some(locale.to_string()),
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        let theme = cx.theme();

        let replying_to = MessagesStore::global(cx)
            .read(cx)
            .reply_target()
            .map(|draft| ReplyTarget {
                sender_name: draft.sender_name.clone(),
            });
        let input_bar = InputBar::new()
            .with_mention_input(mention_input.clone())
            .replying_to(replying_to);

        let header = AnyView::from(self.header.clone()).cached(
            StyleRefinement::default()
                .w_full()
                .h(px(crate::app::window_controls::APP_HEADER_HEIGHT))
                .flex_shrink_0(),
        );

        let drop_input = mention_input.clone();
        let drop_title = mezon_i18n::t(locale, "common.uploadToChannel")
            .replace("{{channelName}}", channel_name.unwrap_or_default());
        let drop_body = mezon_i18n::t(locale, "common.uploadInstructions");
        let drop_overlay = div()
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
                            .child(SharedString::from(drop_title)),
                    )
                    .child(
                        div()
                            .px(px(24.))
                            .text_center()
                            .text_size(px(14.))
                            .text_color(rgb(0xffffff))
                            .child(SharedString::from(drop_body)),
                    ),
            );

        let message_column = div()
            .relative()
            .group("chat-drop-zone")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .on_drop(
                move |paths: &ExternalPaths, window: &mut Window, cx: &mut App| {
                    let dropped: Vec<PathBuf> = paths.paths().to_vec();
                    drop_input.update(cx, |input, cx| input.add_dropped_paths(dropped, window, cx));
                },
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(
                AnyView::from(self.timeline.clone()).cached(StyleRefinement::default().size_full()),
            ))
            .child(input_bar.render(theme, locale))
            .child(self.typing.clone())
            .child(drop_overlay);

        let has_search_panel = show_results_panel && message_search_panel.is_some();
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
            .when(show_member_panel && !has_search_panel, |row| {
                match &self.member_panel {
                    Some(panel) => row.child(
                        AnyView::from(panel.clone()).cached(
                            StyleRefinement::default()
                                .w(px(245.))
                                .h_full()
                                .flex_shrink_0(),
                        ),
                    ),
                    None => row.child(div().w(px(245.)).h_full().flex_shrink_0()),
                }
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
