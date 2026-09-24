use crate::chat::message::invite_card::member_count_label;
use crate::chat::{MentionInput, ReplyTarget};
use crate::components::primitives::{Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};
use gpui::{
    Context, Entity, FontWeight, ObjectFit, Render, SharedString, Subscription, Window, div, img,
    prelude::*, px, radians, rgb,
};
use mezon_store::{MessagesEvent, MessagesStore, OgpResult, Settings, TopicsStore};

const OGP_PREVIEW_MEMBER_DOT: u32 = 0x22_c5_5e;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplyClearSource {
    #[default]
    Messages,
    Topics,
}

pub struct InputBar {
    mention_input: Entity<MentionInput>,
    locale: SharedString,
    replying_to: Option<ReplyTarget>,
    reply_clear: ReplyClearSource,
    _settings_observe: Subscription,
    _mention_observe: Subscription,
    _messages_sub: Subscription,
    ogp_image_cache: Entity<LruImageCache>,
    for_topic: bool,
}

impl InputBar {
    pub fn new(
        mention_input: Entity<MentionInput>,
        locale: SharedString,
        settings: &Entity<Settings>,
        for_topic: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings_observe = cx.observe(settings, |_, _, cx| cx.notify());
        let mention_observe = cx.observe(&mention_input, |_, _, cx| cx.notify());
        let messages_sub = cx.subscribe(
            &MessagesStore::global(cx),
            |_, _, event: &MessagesEvent, cx| {
                if matches!(event, MessagesEvent::AnonymousModeChanged) {
                    cx.notify();
                }
            },
        );
        Self {
            mention_input,
            locale,
            replying_to: None,
            reply_clear: ReplyClearSource::Messages,
            _settings_observe: settings_observe,
            _mention_observe: mention_observe,
            _messages_sub: messages_sub,
            ogp_image_cache: crate::image_cache::ogp_aux_cache("composer-ogp", cx),
            for_topic,
        }
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

    fn render_bar(&self, theme: std::sync::Arc<Theme>, cx: &mut Context<Self>) -> impl IntoElement {
        let replying = self.replying_to.is_some();
        let reply_clear = self.reply_clear;
        let ogp_preview = self.mention_input.read(cx).ogp_preview().cloned();
        let store = MessagesStore::global(cx);
        let anonymous = if self.for_topic {
            store.read(cx).topic_anonymous_mode()
        } else {
            store.read(cx).is_anonymous_mode()
        };
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
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(replying, |d| d.rounded_bl_lg().rounded_br_lg())
                    .when(!replying, |d| d.rounded_lg())
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
