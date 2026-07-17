use crate::chat::{MentionInput, ReplyTarget};
use crate::components::primitives::{Icon, IconName};
use crate::theme::{ActiveTheme, Theme};
use gpui::{Context, Entity, Render, SharedString, Subscription, Window, div, prelude::*, px};
use mezon_store::{MessagesStore, Settings, TopicsStore};

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
}

impl InputBar {
    pub fn new(
        mention_input: Entity<MentionInput>,
        locale: SharedString,
        settings: &Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings_observe = cx.observe(settings, |_, _, cx| cx.notify());
        Self {
            mention_input,
            locale,
            replying_to: None,
            reply_clear: ReplyClearSource::Messages,
            _settings_observe: settings_observe,
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

    fn render_bar(&self, theme: &Theme) -> impl IntoElement {
        let replying = self.replying_to.is_some();
        let reply_clear = self.reply_clear;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .px_3()
            .pb_1()
            .when_some(self.replying_to.as_ref(), |d, target| {
                d.child(Self::reply_preview_bar(
                    theme,
                    &self.locale,
                    target,
                    reply_clear,
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(replying, |d| d.rounded_bl_lg().rounded_br_lg())
                    .when(!replying, |d| d.rounded_lg())
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .bg(theme.tokens.bg_surface)
                    .shadow_md()
                    .child(div().flex_1().child(self.mention_input.clone())),
            )
    }
}

impl Render for InputBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_bar(cx.theme())
    }
}
