use crate::chat::mention_input::grammar_correction_bar as shared_grammar_correction_bar;
use crate::chat::message::invite_card::member_count_label;
use crate::chat::quick_replies::{
    last_text_message_for_quick_replies, message_bucket, quick_reply_source_text,
};
use crate::chat::{MentionInput, ReplyTarget};
use crate::components::primitives::{Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};
use gpui::{
    Context, Entity, FontWeight, ObjectFit, Render, SharedString, Subscription, Task, Window, div,
    img, prelude::*, px, radians, rgb,
};
use mezon_grammar::client::MezonGrammarClient;
use mezon_store::MessageId;
use mezon_store::{
    BadgeService, ChannelId, MessagesEvent, MessagesStore, OgpResult, Settings, TopicsStore,
};

const OGP_PREVIEW_MEMBER_DOT: u32 = 0x22_c5_5e;
const QUICK_REPLY_PILL_BLUE: u32 = 0x3b_82_f6;
const QUICK_REPLY_PILL_BLUE_BORDER: u32 = 0x60_66_f1;
const QUICK_REPLY_PILL_TEXT: u32 = 0x93_c5_fd;
const QUICK_REPLY_TOP_K: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplyClearSource {
    #[default]
    Messages,
    Topics,
}

pub struct InputBar {
    mention_input: Entity<MentionInput>,
    settings: Entity<Settings>,
    locale: SharedString,
    replying_to: Option<ReplyTarget>,
    reply_clear: ReplyClearSource,
    _settings_observe: Subscription,
    _mention_observe: Subscription,
    _messages_sub: Subscription,
    ogp_image_cache: Entity<LruImageCache>,
    for_topic: bool,
    quick_reply_bucket: Option<ChannelId>,
    quick_reply_source_id: Option<MessageId>,
    quick_replies: Vec<SharedString>,
    quick_reply_generation: u64,
    _quick_reply_task: Option<Task<()>>,
}

impl InputBar {
    pub fn new(
        mention_input: Entity<MentionInput>,
        locale: SharedString,
        settings: &Entity<Settings>,
        for_topic: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings_entity = settings.clone();
        let settings_observe = cx.observe(settings, |this, _, cx| {
            if this.sync_quick_replies(cx) {
                cx.notify();
            }
        });
        let mention_observe = cx.observe(&mention_input, |_, _, cx| cx.notify());
        let messages_sub = cx.subscribe(
            &MessagesStore::global(cx),
            |this, _, event: &MessagesEvent, cx| {
                if matches!(event, MessagesEvent::AnonymousModeChanged) {
                    cx.notify();
                    return;
                }
                if Self::quick_replies_need_refresh(event) && this.sync_quick_replies(cx) {
                    cx.notify();
                }
            },
        );
        let mut bar = Self {
            mention_input,
            settings: settings_entity,
            locale,
            replying_to: None,
            reply_clear: ReplyClearSource::Messages,
            _settings_observe: settings_observe,
            _mention_observe: mention_observe,
            _messages_sub: messages_sub,
            ogp_image_cache: crate::image_cache::ogp_aux_cache("composer-ogp", cx),
            for_topic,
            quick_reply_bucket: None,
            quick_reply_source_id: None,
            quick_replies: Vec::new(),
            quick_reply_generation: 0,
            _quick_reply_task: None,
        };
        bar.sync_quick_replies(cx);
        bar
    }

    fn quick_replies_need_refresh(event: &MessagesEvent) -> bool {
        matches!(
            event,
            MessagesEvent::Reset { .. }
                | MessagesEvent::Resized { .. }
                | MessagesEvent::Shifted { .. }
                | MessagesEvent::Updated { .. }
                | MessagesEvent::RemovedAt { .. }
                | MessagesEvent::TopicUpdated { .. }
        )
    }

    fn clear_quick_replies(&mut self) -> bool {
        let changed = self.quick_reply_bucket.is_some()
            || self.quick_reply_source_id.is_some()
            || !self.quick_replies.is_empty()
            || self._quick_reply_task.is_some();
        self.quick_reply_bucket = None;
        self.quick_reply_source_id = None;
        self.quick_replies.clear();
        self._quick_reply_task = None;
        changed
    }

    fn sync_quick_replies(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.read(cx).quick_replies_enabled {
            return self.clear_quick_replies();
        }
        let store = MessagesStore::global(cx).read(cx);
        let bucket = message_bucket(store, self.for_topic);
        let Some(bucket) = bucket else {
            return self.clear_quick_replies();
        };
        let viewer_id = BadgeService::try_global(cx)
            .and_then(|badge| badge.read(cx).current_user_id(cx))
            .map(|id| id.0.to_string());
        let Some(message) =
            last_text_message_for_quick_replies(store, bucket, viewer_id.as_deref())
        else {
            return self.clear_quick_replies();
        };
        let Some(source_text) = quick_reply_source_text(message) else {
            return self.clear_quick_replies();
        };
        if self.quick_reply_bucket == Some(bucket) && self.quick_reply_source_id == Some(message.id)
        {
            return false;
        }
        self.quick_reply_bucket = Some(bucket);
        self.quick_reply_source_id = Some(message.id);
        self.quick_replies.clear();
        self.quick_reply_generation += 1;
        let generation = self.quick_reply_generation;
        let previous_message = source_text.to_string();
        self._quick_reply_task = Some(cx.spawn(async move |this, cx| {
            let replies = cx
                .background_executor()
                .spawn(async move {
                    let client = MezonGrammarClient::shared();
                    client.suggest_quick_replies_with_model(
                        &previous_message,
                        None,
                        QUICK_REPLY_TOP_K,
                    )
                })
                .await;
            this.update(cx, |bar, cx| {
                if bar.quick_reply_generation != generation {
                    return;
                }
                bar._quick_reply_task = None;
                bar.quick_replies = replies.into_iter().map(SharedString::from).collect();
                cx.notify();
            })
            .ok();
        }));
        true
    }

    pub fn sync(
        &mut self,
        locale: &str,
        replying_to: Option<ReplyTarget>,
        reply_clear: ReplyClearSource,
        cx: &mut Context<Self>,
    ) {
        if self.locale.as_ref() == locale
            && self.replying_to == replying_to
            && self.reply_clear == reply_clear
        {
            return;
        }
        self.locale = SharedString::from(locale.to_string());
        self.replying_to = replying_to;
        self.reply_clear = reply_clear;
        cx.notify();
    }

    fn reply_preview_bar(
        theme: &Theme,
        locale: &str,
        target: &ReplyTarget,
        reply_clear: ReplyClearSource,
    ) -> impl IntoElement {
        div()
            .id("reply-preview-bar")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .p_2()
            .rounded_tl_lg()
            .rounded_tr_lg()
            .bg(theme.tokens.theme_setting_nav)
            .text_size(px(14.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .whitespace_nowrap()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(locale, "chat.replyingTo").to_string())
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(target.sender_name.clone()),
                    ),
            )
            .child(
                div()
                    .id("reply-cancel")
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .size_5()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.7))
                    .on_click(move |_, _window, cx| match reply_clear {
                        ReplyClearSource::Messages => {
                            MessagesStore::global(cx).update(cx, |store, cx| store.clear_reply(cx));
                        }
                        ReplyClearSource::Topics => {
                            TopicsStore::global(cx).update(cx, |store, cx| store.clear_reply(cx));
                        }
                    })
                    .child(
                        Icon::new(IconName::Close)
                            .size_4()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
    }

    fn quick_replies_row(
        &self,
        mention_input: Entity<MentionInput>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("quick-replies-row")
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap_2()
            .w_full()
            .mb_1()
            .children(self.quick_replies.iter().enumerate().map(|(index, reply)| {
                let text = reply.clone();
                let mention_input = mention_input.clone();
                div()
                    .id(("quick-reply-pill", index))
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(QUICK_REPLY_PILL_BLUE_BORDER))
                    .bg(gpui::rgba(0x3b_82_f6_33))
                    .text_size(px(13.))
                    .text_color(rgb(QUICK_REPLY_PILL_TEXT))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(QUICK_REPLY_PILL_BLUE)))
                    .child(text.clone())
                    .on_click(cx.listener(move |_, _, window, cx| {
                        mention_input.update(cx, |input, cx| {
                            input.fill_quick_reply(text.as_ref(), window, cx);
                        });
                    }))
            }))
    }

    fn ogp_preview_bar(
        &self,
        preview: &OgpResult,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ogp_image_cache = self.ogp_image_cache.clone();
        let mut text_col = div()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(1.))
            .flex_1()
            .min_w_0();
        if !preview.title.is_empty() {
            text_col = text_col.child(
                div()
                    .w_full()
                    .truncate()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(SharedString::from(preview.title.clone())),
            );
        }
        if !preview.description.is_empty() {
            text_col = text_col.child(
                div()
                    .w_full()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(preview.description.clone())),
            );
        }
        if let Some(count) = preview.member_count {
            text_col = text_col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(13.))
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(rgb(OGP_PREVIEW_MEMBER_DOT)),
                    )
                    .child(member_count_label(&self.locale, count)),
            );
        }
        div()
            .id("ogp-preview-bar")
            .flex()
            .items_center()
            .gap(px(12.))
            .w_full()
            .p_2()
            .mb_1()
            .rounded_lg()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.tokens.theme_setting_nav)
            .when(!preview.image.is_empty(), |row| {
                row.child(
                    div()
                        .size(px(48.))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(px(4.))
                        .bg(theme.tokens.theme_setting_primary)
                        .image_cache(ogp_image_cache)
                        .child(
                            img(SharedString::from(preview.image.clone()))
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        ),
                )
            })
            .child(text_col)
            .child(
                div()
                    .id("ogp-preview-close")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.mention_input
                            .update(cx, |input, cx| input.clear_ogp_preview(cx));
                    })),
            )
    }

    fn grammar_correction_bar(
        &self,
        theme: &Theme,
        text: SharedString,
        cap_top: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        shared_grammar_correction_bar(
            theme,
            text,
            cap_top,
            cx.listener(|this, _, window, cx| {
                this.mention_input.update(cx, |input, cx| {
                    input.apply_grammar_correction(window, cx);
                });
            }),
            cx.listener(|this, _, _, cx| {
                this.mention_input
                    .update(cx, |input, cx| input.dismiss_grammar_popup(cx));
            }),
        )
    }

    fn render_bar(&self, theme: std::sync::Arc<Theme>, cx: &mut Context<Self>) -> impl IntoElement {
        let replying = self.replying_to.is_some();
        let reply_clear = self.reply_clear;
        let grammar_correction = self.mention_input.read(cx).grammar_correction_text();
        let composer_capped = replying || grammar_correction.is_some();
        let ogp_preview = self.mention_input.read(cx).ogp_preview().cloned();
        let store = MessagesStore::global(cx);
        let anonymous = if self.for_topic {
            store.read(cx).topic_anonymous_mode()
        } else {
            store.read(cx).is_anonymous_mode()
        };
        let show_quick_replies = grammar_correction.is_none()
            && self.settings.read(cx).quick_replies_enabled
            && !self.quick_replies.is_empty();
        let mention_input = self.mention_input.clone();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .px_3()
            .pb_1()
            .when_some(ogp_preview, |d, preview| {
                d.child(self.ogp_preview_bar(&preview, &theme, cx))
            })
            .when_some(self.replying_to.as_ref(), |d, target| {
                d.child(Self::reply_preview_bar(
                    &theme,
                    &self.locale,
                    target,
                    reply_clear,
                ))
            })
            .when(show_quick_replies, |d| {
                d.child(self.quick_replies_row(mention_input, cx))
            })
            .when_some(grammar_correction, |d, text| {
                d.child(self.grammar_correction_bar(&theme, text, !replying, cx))
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(composer_capped, |d| d.rounded_bl_lg().rounded_br_lg())
                    .when(!composer_capped, |d| d.rounded_lg())
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .bg(theme.surfaces.surface.fill())
                    .shadow_md()
                    .child(div().flex_1().child(self.mention_input.clone()))
                    .when(anonymous, |composer| {
                        composer.child(
                            div().absolute().top(px(-12.)).right(px(-12.)).child(
                                Icon::new(IconName::HatIcon)
                                    .size(px(28.))
                                    .text_color(theme.tokens.text_theme_primary)
                                    .with_transformation(gpui::Transformation::rotate(radians(
                                        std::f32::consts::FRAC_PI_4,
                                    ))),
                            ),
                        )
                    }),
            )
    }
}

impl Render for InputBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ogp_image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        self.render_bar(cx.theme().clone(), cx)
    }
}
