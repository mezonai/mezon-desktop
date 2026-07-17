use gpui::{
    AnyView, ClickEvent, Context, Entity, FontWeight, KeyDownEvent, SharedString, StyleRefinement,
    Subscription, Window, div, prelude::*, px,
};
use mezon_store::{ChannelId, MessageId, MessagesStore, Settings, TopicsEvent, TopicsStore};

use crate::app::window_controls::APP_HEADER_HEIGHT;
use crate::chat::ReplyTarget;
use crate::chat::channel_typing::ChannelTyping;
use crate::chat::input_bar::{InputBar, ReplyClearSource};
use crate::chat::mention_input::{MentionInput, MentionInputEvent};
use crate::chat::message::ChannelMessages;
use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use crate::theme::ActiveTheme;

const PANEL_WIDTH: f32 = 510.;

pub struct TopicPanel {
    settings: Entity<Settings>,
    mention_input: Entity<MentionInput>,
    input_bar: Entity<InputBar>,
    typing: Entity<ChannelTyping>,
    topic_timeline: Entity<ChannelMessages>,
    reply_target_id: Option<MessageId>,
    _subs: Vec<Subscription>,
}

impl TopicPanel {
    pub fn new(
        settings: Entity<Settings>,
        align_timeline: Entity<ChannelMessages>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "messageBox.placeholder").to_string();
        let mention_input =
            cx.new(|cx| MentionInput::new(placeholder, settings.clone(), window, cx));
        let input_bar = cx.new(|cx| {
            InputBar::new(
                mention_input.clone(),
                SharedString::from(locale.clone()),
                &settings,
                cx,
            )
        });
        let typing = cx.new(|cx| ChannelTyping::new(&settings, cx));
        let topic_timeline =
            cx.new(|cx| ChannelMessages::new_topic_box(settings.clone(), align_timeline, cx));
        topic_timeline.update(cx, |timeline, cx| timeline.bind_window(window, cx));
        mention_input.update(cx, |input, cx| input.focus_input(window, cx));

        let mut subs = Vec::new();
        subs.push(cx.observe(&MessagesStore::global(cx), |_, _, cx| cx.notify()));
        subs.push(cx.observe(&TopicsStore::global(cx), |this, store, cx| {
            if store.read(cx).reply_target().is_none() {
                this.reply_target_id = None;
            }
            cx.notify();
        }));
        subs.push(cx.subscribe_in(
            &TopicsStore::global(cx),
            window,
            |this, store, event: &TopicsEvent, window, cx| {
                if !matches!(event, TopicsEvent::ReplyTargetChanged) {
                    return;
                }
                let reply_id = store.read(cx).reply_target().map(|d| d.message_ref_id);
                if reply_id.is_some() && reply_id != this.reply_target_id {
                    this.mention_input
                        .update(cx, |input, cx| input.focus_input(window, cx));
                }
                this.reply_target_id = reply_id;
            },
        ));
        subs.push(cx.subscribe_in(
            &mention_input,
            window,
            |this, _, event: &MentionInputEvent, window, cx| match event {
                MentionInputEvent::Submit => this.submit(window, cx),
                MentionInputEvent::SendSticker { url, filename } => {
                    this.send_sticker(url.clone(), filename.clone(), cx);
                }
                MentionInputEvent::SendGif { url, width, height } => {
                    this.send_gif(url.clone(), *width, *height, cx);
                }
                MentionInputEvent::SendSound { url, filename } => {
                    this.send_sound(url.clone(), filename.clone(), cx);
                }
                MentionInputEvent::Cancel => {
                    TopicsStore::global(cx).update(cx, |store, cx| store.close_panel(cx));
                }
            },
        ));

        Self {
            settings,
            mention_input,
            input_bar,
            typing,
            topic_timeline,
            reply_target_id: None,
            _subs: subs,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if TopicsStore::global(cx).read(cx).is_submitting() {
            return;
        }
        let Some((content, content_tokens, attachments)) = self
            .mention_input
            .update(cx, |input, cx| input.take_payload(window, cx))
        else {
            return;
        };
        TopicsStore::global(cx).update(cx, |store, cx| {
            store.submit_reply(content, content_tokens, attachments, cx);
        });
    }

    fn send_sticker(&mut self, url: String, filename: String, cx: &mut Context<Self>) {
        if TopicsStore::global(cx).read(cx).is_submitting() {
            return;
        }
        TopicsStore::global(cx).update(cx, |store, cx| {
            store.submit_reply_url_attachment(url, filename, "sticker".to_string(), 0, 0, cx);
        });
    }

    fn send_gif(&mut self, url: String, width: u32, height: u32, cx: &mut Context<Self>) {
        if TopicsStore::global(cx).read(cx).is_submitting() {
            return;
        }
        TopicsStore::global(cx).update(cx, |store, cx| {
            store.submit_reply_url_attachment(
                url,
                String::new(),
                "sticker".to_string(),
                width as i32,
                height as i32,
                cx,
            );
        });
    }

    fn send_sound(&mut self, url: String, filename: String, cx: &mut Context<Self>) {
        if TopicsStore::global(cx).read(cx).is_submitting() {
            return;
        }
        TopicsStore::global(cx).update(cx, |store, cx| {
            store.submit_reply_url_attachment(url, filename, "audio/mpeg".to_string(), 0, 0, cx);
        });
    }
}

impl Render for TopicPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (topic_id, error) = {
            let topics = TopicsStore::global(cx).read(cx);
            (
                topics.active_topic_id(),
                topics.create_error().map(str::to_string),
            )
        };

        self.typing.update(cx, |typing, cx| {
            typing.sync(topic_id.map(ChannelId), cx);
        });

        let locale = self.settings.read(cx).language.clone();
        let replying_to = TopicsStore::global(cx)
            .read(cx)
            .reply_target()
            .map(|draft| ReplyTarget {
                sender_name: SharedString::from(draft.sender_name.clone()),
            });
        self.input_bar.update(cx, |bar, cx| {
            bar.sync(&locale, replying_to, ReplyClearSource::Topics, cx);
        });

        let theme = cx.theme();
        let tokens = &theme.tokens;

        let header = h_flex()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .w_full()
            .h(px(APP_HEADER_HEIGHT))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::TopicIcon)
                            .size(px(20.))
                            .text_color(tokens.text_theme_primary),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tokens.text_theme_primary)
                            .child(mezon_i18n::t(&locale, "channelTopbar.topic")),
                    ),
            )
            .child(
                div()
                    .id("topic-panel-close")
                    .cursor_pointer()
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(20.))
                            .text_color(tokens.text_theme_primary_hover),
                    )
                    .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                        TopicsStore::global(cx).update(cx, |store, cx| store.close_panel(cx));
                    })),
            );

        let composer = v_flex()
            .flex_shrink_0()
            .when_some(error, |this, err| {
                this.px_3().pt_2().child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.status_dnd)
                        .child(err),
                )
            })
            .child(self.input_bar.clone())
            .child(self.typing.clone());

        v_flex()
            .w(px(PANEL_WIDTH))
            .min_w(px(PANEL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .border_l_1()
            .border_color(tokens.border_primary)
            .bg(theme.bg_primary)
            .text_color(tokens.text_theme_message)
            .on_key_down(cx.listener(|_this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "escape" {
                    TopicsStore::global(cx).update(cx, |store, cx| store.close_panel(cx));
                }
            }))
            .child(header)
            .child(
                div().flex_1().min_h_0().w_full().overflow_hidden().child(
                    AnyView::from(self.topic_timeline.clone())
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .child(composer)
    }
}
