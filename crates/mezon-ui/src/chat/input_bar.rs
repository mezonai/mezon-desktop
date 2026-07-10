use crate::chat::{MentionInput, ReplyTarget};
use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;
use gpui::{div, prelude::*, px};
use mezon_store::MessagesStore;

pub struct InputBar {
    mention_input: Option<gpui::Entity<MentionInput>>,
    replying_to: Option<ReplyTarget>,
}

impl Default for InputBar {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBar {
    pub fn new() -> Self {
        Self {
            mention_input: None,
            replying_to: None,
        }
    }

    pub fn with_mention_input(mut self, mention_input: gpui::Entity<MentionInput>) -> Self {
        self.mention_input = Some(mention_input);
        self
    }

    pub fn replying_to(mut self, target: Option<ReplyTarget>) -> Self {
        self.replying_to = target;
        self
    }

    fn reply_preview_bar(theme: &Theme, locale: &str, target: &ReplyTarget) -> impl IntoElement {
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
                    .on_click(|_, _window, cx| {
                        MessagesStore::global(cx).update(cx, |store, cx| store.clear_reply(cx));
                    })
                    .child(
                        Icon::new(IconName::Close)
                            .size_4()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
    }

    pub fn render(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        let replying = self.replying_to.is_some();
        div()
            .flex()
            .flex_col()
            .px_3()
            .pb_1()
            .when_some(self.replying_to.as_ref(), |d, target| {
                d.child(Self::reply_preview_bar(theme, locale, target))
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
                    .when_some(self.mention_input.clone(), |d, mention_input| {
                        d.child(div().flex_1().child(mention_input))
                    }),
            )
    }
}
