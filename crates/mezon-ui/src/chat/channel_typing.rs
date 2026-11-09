use crate::components::primitives::{Icon, IconName};

use gpui::{
    Context, Entity, FontWeight, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use mezon_store::{ChannelId, ChannelTypingState, PresenceEvent, PresenceStore, Settings};

use crate::theme::ActiveTheme;

pub struct ChannelTyping {
    channel_id: Option<ChannelId>,
    is_typing_suffix: SharedString,
    several_people: SharedString,
    _presence_sub: Subscription,
    _settings_sub: Subscription,
}

enum TypingContent {
    One {
        name: SharedString,
        suffix: SharedString,
    },
    Several(SharedString),
}

impl ChannelTyping {
    pub fn new(settings: &Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let (is_typing_suffix, several_people) = Self::i18n_labels(&settings.read(cx).language);
        let _presence_sub = cx.subscribe(&PresenceStore::global(cx), |this, _, event, cx| {
            if let PresenceEvent::TypingChanged { channel_id } = event
                && this.channel_id == Some(*channel_id)
            {
                cx.notify();
            }
        });
        let _settings_sub = cx.observe(settings, |this, settings, cx| {
            let (is_typing_suffix, several_people) = Self::i18n_labels(&settings.read(cx).language);
            this.is_typing_suffix = is_typing_suffix;
            this.several_people = several_people;
            cx.notify();
        });
        Self {
            channel_id: None,
            is_typing_suffix,
            several_people,
            _presence_sub,
            _settings_sub,
        }
    }

    fn i18n_labels(locale: &str) -> (SharedString, SharedString) {
        let suffix = mezon_i18n::t(locale, "common.isTyping")
            .trim()
            .trim_end_matches(['.', '…'])
            .to_owned();
        (
            SharedString::from(suffix),
            SharedString::from(mezon_i18n::t(locale, "common.severalPeopleTyping")),
        )
    }

    pub fn sync(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        if self.channel_id == channel_id {
            return;
        }
        self.channel_id = channel_id;
        cx.notify();
    }

    fn content(&self, cx: &Context<Self>) -> Option<TypingContent> {
        let channel_id = self.channel_id?;
        match PresenceStore::global(cx)
            .read(cx)
            .channel_typing_state(channel_id)
        {
            ChannelTypingState::Idle => None,
            ChannelTypingState::One(name) => Some(TypingContent::One {
                name,
                suffix: self.is_typing_suffix.clone(),
            }),
            ChannelTypingState::Several => {
                Some(TypingContent::Several(self.several_people.clone()))
            }
        }
    }
}

impl Render for ChannelTyping {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (primary, active) = {
            let theme = cx.theme();
            (theme.tokens.text_theme_primary, theme.tokens.text_secondary)
        };
        let bar = div()
            .pl_3()
            .pr_1()
            .h(px(16.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1p5()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_xs()
            .line_height(px(16.))
            .text_color(primary);
        match self.content(cx) {
            Some(TypingContent::One { name, suffix }) => bar
                .child(
                    Icon::new(IconName::TypingIndicator)
                        .size(px(16.))
                        .flex_none()
                        .text_color(primary),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .min_w_0()
                        .child(
                            div()
                                .min_w_0()
                                .mr(px(2.))
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(active)
                                .child(name),
                        )
                        .child(div().flex_none().text_color(primary).child(suffix)),
                ),
            Some(TypingContent::Several(text)) => bar.child(text),
            None => bar,
        }
    }
}
