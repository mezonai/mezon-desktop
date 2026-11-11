use crate::components::primitives::{Label, Switch, h_flex, v_flex};
use crate::theme::ActiveTheme;
use gpui::{Context, Entity, FontWeight, Window, prelude::*};
use mezon_store::{DmSettingStore, Settings};

pub struct ChatPage {
    settings: Entity<Settings>,
}

impl ChatPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        if let Some(dm_setting) = DmSettingStore::try_global(cx) {
            cx.observe(&dm_setting, |_, _, cx| cx.notify()).detach();
        }
        Self { settings }
    }

    fn toggle_row(
        &self,
        id: &'static str,
        title_key: &'static str,
        description_key: &'static str,
        checked: bool,
        toggle: impl Fn(&mut Settings) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let settings = self.settings.clone();
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_4()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(&locale, title_key))
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(&locale, description_key))
                            .text_sm()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                Switch::new(id)
                    .checked(checked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.update(cx, |s, cx| {
                            toggle(s);
                            cx.notify();
                        });
                        mezon_store::schedule_settings_save(&settings, cx);
                        cx.notify();
                    })),
            )
    }

    fn only_friends_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let checked =
            DmSettingStore::try_global(cx).is_some_and(|store| store.read(cx).only_friends_dm());
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_4()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.chat.onlyFriendsDm.title"))
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.chat.onlyFriendsDm.description",
                        ))
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                Switch::new("only-friends-dm")
                    .checked(checked)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let Some(store) = DmSettingStore::try_global(cx) else {
                            return;
                        };
                        let enabled = !store.read(cx).only_friends_dm();
                        store.update(cx, |store, cx| store.set_only_friends(enabled, cx));
                    })),
            )
    }
}

impl Render for ChatPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quick = self.settings.read(cx).quick_replies_enabled;
        let grammar = self.settings.read(cx).grammar_correction_enabled;
        v_flex()
            .w_full()
            .gap_3()
            .child(self.toggle_row(
                "quick-replies-enabled",
                "setting.chat.quickReplies.title",
                "setting.chat.quickReplies.description",
                quick,
                |s| s.quick_replies_enabled = !s.quick_replies_enabled,
                cx,
            ))
            .child(self.toggle_row(
                "grammar-correction-enabled",
                "setting.chat.grammarCorrection.title",
                "setting.chat.grammarCorrection.description",
                grammar,
                |s| {
                    s.grammar_correction_enabled = !s.grammar_correction_enabled;
                    mezon_grammar::set_download_enabled(s.grammar_correction_enabled);
                    if s.grammar_correction_enabled {
                        mezon_grammar::prefetch_all_assets();
                    }
                },
                cx,
            ))
            .child(self.only_friends_row(cx))
    }
}
