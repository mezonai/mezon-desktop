use std::time::Duration;

use gpui::{
    App, Context, Entity, FontWeight, ObjectFit, SharedString, Subscription, Task, Window, div,
    img, prelude::*, px, rgb,
};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelType, ClanId, MAX_CHANNEL_TOPIC_CHARS,
    PERMISSION_ADMINISTRATOR, PERMISSION_CLAN_OWNER, PERMISSION_MANAGE_CHANNEL,
    PERMISSION_MANAGE_CLAN, PermissionStore, Settings, UpdateChannelOverviewError,
    overview_duplicate_thread_parent_id, truncate_chars, validate_channel_name,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Switch, TextArea,
    TextAreaEvent, TextAreaField, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::assets::{CHANNEL_SETTING_LOGO_DARK, CHANNEL_SETTING_LOGO_LIGHT};
use crate::util::theme::theme_is_light;

const DUPLICATE_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameValidation {
    None,
    Invalid,
    Duplicate,
    Valid,
}

pub struct OverviewTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    is_thread: bool,
    channel_type: ChannelType,
    name_input: Entity<InputState>,
    topic_input: Entity<TextArea>,
    saved_label: String,
    saved_topic: String,
    saved_age_restricted: i32,
    draft_age_restricted: i32,
    validation: NameValidation,
    duplicate_checking: bool,
    detail_loading: bool,
    saving: bool,
    pending_store_sync: bool,
    _name_sub: Subscription,
    _topic_sub: Subscription,
    _channel_sync_sub: Subscription,
    _save_task: Option<Task<()>>,
    _duplicate_task: Task<()>,
    _fetch_task: Task<()>,
    _subs: Vec<Subscription>,
}

impl OverviewTab {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        is_thread: bool,
        channel_type: ChannelType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .cloned();
        let saved_label = channel
            .as_ref()
            .map(|channel| channel.name.clone())
            .unwrap_or_default();
        let saved_topic = channel
            .as_ref()
            .map(|channel| channel.topic.clone())
            .unwrap_or_default();
        let saved_age_restricted = channel
            .as_ref()
            .map(|channel| channel.age_restricted)
            .unwrap_or(0);
        let draft_age_restricted = saved_age_restricted;

        let locale = settings.read(cx).language.clone();
        let theme = cx.theme().clone();

        let name_placeholder: SharedString = if is_thread {
            mezon_i18n::t(&locale, "channelSetting.fields.threadName.placeholder").into()
        } else {
            mezon_i18n::t(&locale, "channelSetting.fields.channelName.placeholder").into()
        };

        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(name_placeholder)
                .height(px(40.))
                .embedded(true)
                .borderless()
                .bg(gpui::transparent_black())
                .text_color(theme.text_primary)
                .text_size(px(15.))
        });
        name_input.update(cx, |input, cx| {
            input.set_value(saved_label.clone(), window, cx);
        });

        let topic_placeholder: SharedString = if is_thread {
            mezon_i18n::t(
                &locale,
                "channelSetting.fields.threadDescription.placeholder",
            )
            .into()
        } else {
            mezon_i18n::t(
                &locale,
                "channelSetting.fields.channelDescription.placeholder",
            )
            .into()
        };

        let topic_input = cx.new(|cx| {
            TextArea::new(window, cx)
                .placeholder(topic_placeholder)
                .min_height(px(87.))
                .max_visible_lines(8)
                .bg(gpui::transparent_black())
                .text_color(theme.text_primary)
                .text_size(px(15.))
                .padding_x(px(12.))
                .radius(px(0.))
        });
        topic_input.update(cx, |state, cx| {
            state.set_value(saved_topic.clone(), cx);
        });

        let name_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_name_changed(cx);
            }
        });
        let topic_sub = cx.subscribe(&topic_input, |this, _, event: &TextAreaEvent, cx| {
            if *event == TextAreaEvent::Change {
                this.on_topic_changed(cx);
            }
        });

        let channel_list = ChannelList::global(cx);
        let permission_store = PermissionStore::global(cx);
        let channel_sync_sub = cx.observe(&channel_list, |this, _, cx| {
            this.pending_store_sync = true;
            cx.notify();
        });
        let subs = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&permission_store, |_, _, cx| cx.notify()),
        ];

        let api = super::channel_acl::api(cx);
        let fetch_task = cx.spawn(async move |this, cx| {
            let Some(api) = api else {
                let _ = this.update(cx, |this, cx| {
                    this.detail_loading = false;
                    cx.notify();
                });
                return;
            };
            let result = api.list_channel_detail(channel_id.get()).await;
            let _ = this.update(cx, |this, cx| {
                this.detail_loading = false;
                if let Ok(detail) = result {
                    let topic_dirty = this.is_topic_dirty(cx);
                    if !topic_dirty {
                        this.saved_topic = detail.topic.clone();
                        this.topic_input.update(cx, |state, cx| {
                            state.set_value(detail.topic.clone(), cx);
                        });
                    }
                    ChannelList::global(cx).update(cx, |store, cx| {
                        store.patch_channel_overview_detail(
                            clan_id,
                            channel_id,
                            detail.topic,
                            None,
                            detail.e2ee,
                            detail.app_id,
                            cx,
                        );
                    });
                }
                cx.notify();
            });
        });

        Self {
            clan_id,
            channel_id,
            settings,
            is_thread,
            channel_type,
            name_input,
            topic_input,
            saved_label,
            saved_topic,
            saved_age_restricted,
            draft_age_restricted,
            validation: NameValidation::None,
            duplicate_checking: false,
            detail_loading: true,
            saving: false,
            pending_store_sync: false,
            _name_sub: name_sub,
            _topic_sub: topic_sub,
            _channel_sync_sub: channel_sync_sub,
            _save_task: None,
            _duplicate_task: Task::ready(()),
            _fetch_task: fetch_task,
            _subs: subs,
        }
    }

    fn draft_label(&self, cx: &App) -> String {
        self.name_input.read(cx).value().to_string()
    }

    fn draft_topic(&self, cx: &App) -> String {
        self.topic_input.read(cx).value().to_string()
    }

    fn can_edit(&self, cx: &App) -> bool {
        let store = PermissionStore::global(cx);
        let store = store.read(cx);
        let has_manage_channel_or_clan =
            store.check(self.clan_id, None, PERMISSION_MANAGE_CHANNEL, cx)
                || store.check(self.clan_id, None, PERMISSION_MANAGE_CLAN, cx);
        let is_clan_owner = store.check(self.clan_id, None, PERMISSION_CLAN_OWNER, cx);
        let is_administrator = store.check(self.clan_id, None, PERMISSION_ADMINISTRATOR, cx);
        let is_creator = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .is_some_and(|channel| {
                BadgeService::try_global(cx)
                    .and_then(|badges| badges.read(cx).current_user_id(cx))
                    .is_some_and(|me| me == channel.creator_id)
            });
        can_edit_overview(
            self.is_thread,
            has_manage_channel_or_clan,
            is_clan_owner,
            is_administrator,
            is_creator,
        )
    }

    fn sync_from_store(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .cloned()
        else {
            return;
        };

        if !self.is_name_dirty(cx) && channel.name != self.saved_label {
            self.saved_label = channel.name.clone();
            self.name_input.update(cx, |input, cx| {
                input.set_value(self.saved_label.clone(), window, cx);
            });
            self.validation = NameValidation::None;
        }

        if !self.is_topic_dirty(cx) && channel.topic != self.saved_topic {
            self.saved_topic = channel.topic.clone();
            self.topic_input.update(cx, |state, cx| {
                state.set_value(self.saved_topic.clone(), cx);
            });
        }

        if !self.is_age_restricted_dirty(cx) && channel.age_restricted != self.saved_age_restricted
        {
            self.saved_age_restricted = channel.age_restricted;
            self.draft_age_restricted = channel.age_restricted;
        }
    }

    fn is_name_dirty(&self, cx: &App) -> bool {
        self.draft_label(cx).trim() != self.saved_label
    }

    fn is_topic_dirty(&self, cx: &App) -> bool {
        self.draft_topic(cx) != self.saved_topic
    }

    fn is_age_restricted_dirty(&self, _cx: &App) -> bool {
        self.draft_age_restricted != self.saved_age_restricted
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.is_name_dirty(cx) || self.is_topic_dirty(cx) || self.is_age_restricted_dirty(cx)
    }

    pub fn should_show_save_bar(&self, cx: &App) -> bool {
        if !self.can_edit(cx) || !self.is_dirty(cx) {
            return false;
        }
        if self.is_name_dirty(cx) && self.validation != NameValidation::Valid {
            return false;
        }
        true
    }

    pub fn reset_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset(window, cx);
    }

    pub fn save_draft(&mut self, cx: &mut Context<Self>) {
        self.save(cx);
    }

    fn can_save(&self, cx: &App) -> bool {
        if !self.can_edit(cx) || self.saving || self.duplicate_checking || self.detail_loading {
            return false;
        }
        if !self.is_dirty(cx) {
            return false;
        }
        if self.is_name_dirty(cx) {
            if self.draft_label(cx).trim().is_empty() {
                return false;
            }
            if self.validation != NameValidation::Valid {
                return false;
            }
        }
        true
    }

    fn on_name_changed(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit(cx) {
            return;
        }
        let value = self.draft_label(cx);
        if value.trim().is_empty() {
            self.validation = NameValidation::Invalid;
            self.duplicate_checking = false;
            cx.notify();
            return;
        }
        match validate_channel_name(&value) {
            Ok(trimmed) if trimmed == self.saved_label => {
                self.validation = NameValidation::Valid;
                self.duplicate_checking = false;
            }
            Ok(_) => {
                self.validation = NameValidation::Valid;
                self.schedule_duplicate_check(cx);
            }
            Err(_) => {
                self.validation = NameValidation::Invalid;
                self.duplicate_checking = false;
            }
        }
        cx.notify();
    }

    fn on_topic_changed(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit(cx) {
            return;
        }
        let raw = self.draft_topic(cx);
        let value = truncate_chars(&raw, MAX_CHANNEL_TOPIC_CHARS);
        if value != raw {
            self.topic_input.update(cx, |state, cx| {
                state.set_value(value, cx);
            });
        }
        cx.notify();
    }

    fn schedule_duplicate_check(&mut self, cx: &mut Context<Self>) {
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let saved_label = self.saved_label.clone();
        let label = self.draft_label(cx);
        let channel = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .cloned();
        let api = super::channel_acl::api(cx);
        self.duplicate_checking = true;
        self._duplicate_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DUPLICATE_DEBOUNCE).await;
            let Some(channel) = channel else {
                let _ = this.update(cx, |this, cx| {
                    this.duplicate_checking = false;
                    cx.notify();
                });
                return;
            };
            let validated = match validate_channel_name(&label) {
                Ok(name) => name,
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.duplicate_checking = false;
                        this.validation = NameValidation::Invalid;
                        cx.notify();
                    });
                    return;
                }
            };
            if validated == saved_label {
                let _ = this.update(cx, |this, cx| {
                    this.duplicate_checking = false;
                    this.validation = NameValidation::Valid;
                    cx.notify();
                });
                return;
            }
            let Some(api) = api else {
                let _ = this.update(cx, |this, cx| {
                    this.duplicate_checking = false;
                    cx.notify();
                });
                return;
            };
            let duplicate = if let Some(parent) = overview_duplicate_thread_parent_id(&channel) {
                api.check_duplicate_thread_name(&validated, &parent)
                    .await
                    .unwrap_or(false)
            } else {
                let category_id = channel.category_id.clone().unwrap_or_default();
                api.check_duplicate_channel_name(&validated, &category_id)
                    .await
                    .unwrap_or(false)
            };
            let _ = this.update(cx, |this, cx| {
                if this.draft_label(cx) != label {
                    return;
                }
                this.duplicate_checking = false;
                this.validation = if duplicate {
                    NameValidation::Duplicate
                } else {
                    NameValidation::Valid
                };
                cx.notify();
            });
        });
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.name_input.update(cx, |input, cx| {
            input.set_value(self.saved_label.clone(), window, cx);
        });
        self.topic_input.update(cx, |state, cx| {
            state.set_value(self.saved_topic.clone(), cx);
        });
        self.draft_age_restricted = self.saved_age_restricted;
        self.validation = NameValidation::None;
        self.duplicate_checking = false;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let sent_label = match validate_channel_name(&self.draft_label(cx)) {
            Ok(label) => label,
            Err(_) => return,
        };
        let sent_topic = truncate_chars(&self.draft_topic(cx), MAX_CHANNEL_TOPIC_CHARS);
        let sent_age_restricted = self.draft_age_restricted;
        self.saving = true;
        cx.notify();

        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let channel_list = ChannelList::global(cx);
        let task = channel_list.update(cx, |store, cx| {
            store.update_channel_overview(
                clan_id,
                channel_id,
                sent_label.clone(),
                sent_topic.clone(),
                sent_age_restricted,
                cx,
            )
        });
        self._save_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.saved_label = sent_label;
                        this.saved_topic = sent_topic;
                        this.saved_age_restricted = sent_age_restricted;
                        this.validation = NameValidation::None;
                    }
                    Err(UpdateChannelOverviewError::InvalidName) => {
                        this.validation = NameValidation::Invalid;
                    }
                    Err(UpdateChannelOverviewError::DuplicateName) => {
                        this.validation = NameValidation::Duplicate;
                    }
                    Err(UpdateChannelOverviewError::Other(msg)) => {
                        tracing::error!("update_channel_overview failed: {msg}");
                        Shell::global(cx).update(cx, |shell, cx| shell.error(msg, cx));
                    }
                }
                cx.notify();
            });
        }));
    }

    fn validation_message(&self, locale: &str, draft: &str) -> Option<SharedString> {
        overview_name_validation_key(self.is_thread, self.validation, draft.trim().is_empty())
            .map(|key| mezon_i18n::t(locale, key).into())
    }

    fn render_hr(theme: &Theme) -> impl IntoElement {
        div().h(px(1.0)).w_full().bg(theme.tokens.border_primary)
    }

    fn render_section_divider(theme: &Theme) -> impl IntoElement {
        div().my(px(40.0)).child(Self::render_hr(theme))
    }

    fn render_bottom_logo(theme: &Theme) -> impl IntoElement {
        let src = if theme_is_light(theme) {
            CHANNEL_SETTING_LOGO_LIGHT
        } else {
            CHANNEL_SETTING_LOGO_DARK
        };
        div()
            .flex()
            .justify_center()
            .pb(px(40.0))
            .child(img(src).w(px(280.0)).object_fit(ObjectFit::Cover))
    }

    fn render_name_field(
        &self,
        name_label: SharedString,
        theme: &Theme,
        can_edit: bool,
        cx: &App,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .mb_2()
                    .text_color(theme.text_primary)
                    .child(name_label),
            )
            .child(if can_edit {
                div()
                    .id("channel-overview-name")
                    .w_full()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .pl(px(12.))
                    .pr(px(12.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .bg(theme.tokens.bg_input_secondary)
                    .child(Input::new(&self.name_input).w_full())
                    .into_any_element()
            } else {
                div()
                    .id("channel-overview-name")
                    .w_full()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .pl(px(12.))
                    .pr(px(12.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .bg(theme.tokens.bg_input_secondary)
                    .opacity(0.6)
                    .text_size(px(15.))
                    .text_color(theme.text_primary)
                    .child(self.draft_label(cx))
                    .into_any_element()
            })
    }

    fn render_topic_field(
        &self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        cx: &App,
    ) -> impl IntoElement {
        let topic_label = if self.is_thread {
            mezon_i18n::t(locale, "channelSetting.fields.threadDescription.title")
        } else {
            mezon_i18n::t(locale, "channelSetting.fields.channelDescription.title")
        };
        let topic_len = self.draft_topic(cx).chars().count();
        let remaining = MAX_CHANNEL_TOPIC_CHARS.saturating_sub(topic_len);

        v_flex()
            .w_full()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .mb_2()
                    .text_color(theme.text_primary)
                    .child(topic_label),
            )
            .child(if can_edit {
                div()
                    .id("channel-overview-topic")
                    .relative()
                    .w_full()
                    .min_h(px(87.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .bg(theme.tokens.bg_input_secondary)
                    .overflow_hidden()
                    .child(TextAreaField::new(&self.topic_input).w_full())
                    .child(
                        div()
                            .absolute()
                            .bottom(px(8.0))
                            .right(px(8.0))
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(remaining.to_string()),
                    )
                    .into_any_element()
            } else {
                div()
                    .id("channel-overview-topic")
                    .relative()
                    .w_full()
                    .min_h(px(87.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .bg(theme.tokens.bg_input_secondary)
                    .opacity(0.6)
                    .p_3()
                    .text_size(px(15.))
                    .text_color(theme.text_primary)
                    .child(self.draft_topic(cx))
                    .into_any_element()
            })
    }

    fn render_age_restricted(
        &self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let checked = self.draft_age_restricted == 1;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .when(!can_edit, |el| el.opacity(0.6))
                    .child(
                        div()
                            .flex_1()
                            .pr_4()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "channelSetting.overview.ageRestricted.title",
                            )),
                    )
                    .child({
                        let mut switch = Switch::new("channel-overview-age-restricted")
                            .checked(checked)
                            .disabled(!can_edit);
                        if can_edit {
                            switch = switch.on_click(cx.listener(|this, checked, _, cx| {
                                this.draft_age_restricted = i32::from(*checked);
                                cx.notify();
                            }));
                        }
                        switch
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.overview.ageRestricted.description",
                    )),
            )
    }

    fn render_hide_inactivity(&self, locale: &str, theme: &Theme) -> impl IntoElement {
        let value = mezon_i18n::t(locale, "channelSetting.fields.channelHideInactivity._1Week");
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.fields.channelHideInactivity.title",
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(50.0))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.bg_input_secondary)
                    .opacity(0.5)
                    .cursor_default()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(value),
                    )
                    .child(
                        Icon::new(IconName::ArrowDownFill)
                            .size(px(16.0))
                            .flex_shrink_0()
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.fields.channelHideInactivity.description",
                    )),
            )
    }

    fn render_bottom_block(
        &self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .mt(px(40.0))
            .gap(px(40.0))
            .text_sm()
            .text_color(theme.text_primary)
            .child(Self::render_hr(theme))
            .when(self.show_age_restricted(), |section| {
                section
                    .child(self.render_age_restricted(locale, theme, can_edit, cx))
                    .child(Self::render_hr(theme))
            })
            .when(self.show_hide_inactivity(), |section| {
                section.child(self.render_hide_inactivity(locale, theme))
            })
            .child(Self::render_bottom_logo(theme))
    }

    fn show_bottom_block(&self) -> bool {
        !matches!(self.channel_type, ChannelType::Voice | ChannelType::Stream)
    }

    fn show_age_restricted(&self) -> bool {
        !self.is_thread && self.show_bottom_block()
    }

    fn show_hide_inactivity(&self) -> bool {
        self.show_bottom_block()
    }
}

pub fn render_channel_overview_save_bar(
    overview: Entity<OverviewTab>,
    locale: &str,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let can_save = overview.read(cx).can_save(cx);
    div()
        .absolute()
        .bottom(px(20.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .occlude()
        .child(
            div()
                .w(px(700.0))
                .max_w(gpui::relative(0.9))
                .py(px(10.0))
                .pl_4()
                .pr(px(10.0))
                .rounded(px(5.0))
                .bg(theme.bg_floating)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "clanSettings.modalSaveChanges.title",
                                )),
                        )
                        .child(
                            h_flex()
                                .gap(px(20.0))
                                .items_center()
                                .child(
                                    Button::new("channel-overview-reset")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "clanSettings.modalSaveChanges.reset",
                                        ))
                                        .ghost()
                                        .on_click({
                                            let overview = overview.clone();
                                            move |_, window, cx| {
                                                overview.update(cx, |tab, cx| {
                                                    tab.reset_draft(window, cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    Button::new("channel-overview-save")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "clanSettings.modalSaveChanges.saveChanges",
                                        ))
                                        .primary()
                                        .disabled(!can_save)
                                        .on_click({
                                            let overview = overview.clone();
                                            move |_, _, cx| {
                                                overview.update(cx, |tab, cx| {
                                                    tab.save_draft(cx);
                                                });
                                            }
                                        }),
                                ),
                        ),
                ),
        )
}

impl Render for OverviewTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_store_sync {
            self.pending_store_sync = false;
            self.sync_from_store(window, cx);
        }

        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let can_edit = self.can_edit(cx);
        let title = mezon_i18n::t(&locale, "channelSetting.overview.title");
        let name_label: SharedString = if self.is_thread {
            mezon_i18n::t(&locale, "channelSetting.fields.threadName.title").into()
        } else {
            mezon_i18n::t(&locale, "channelSetting.fields.channelName.title").into()
        };
        let draft = self.draft_label(cx);
        let validation_message = self.validation_message(&locale, &draft);

        v_flex()
            .id("channel-overview-tab")
            .w_full()
            .text_size(px(15.0))
            .child(
                div()
                    .mb_4()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(title),
            )
            .child(self.render_name_field(name_label, &theme, can_edit, cx))
            .when_some(validation_message, |el, message| {
                el.child(
                    div()
                        .mt_1()
                        .text_xs()
                        .italic()
                        .text_color(rgb(0xe4_41_41))
                        .child(message),
                )
            })
            .child(Self::render_section_divider(&theme))
            .child(self.render_topic_field(&locale, &theme, can_edit, cx))
            .when(self.show_bottom_block(), |el| {
                el.child(self.render_bottom_block(&locale, &theme, can_edit, cx))
            })
            .child(div().h(px(80.0)))
    }
}

fn can_edit_overview(
    is_thread: bool,
    has_manage_channel_or_clan: bool,
    is_clan_owner: bool,
    is_administrator: bool,
    is_creator: bool,
) -> bool {
    has_manage_channel_or_clan || (is_thread && (is_clan_owner || is_administrator || is_creator))
}

fn overview_name_validation_key(
    is_thread: bool,
    validation: NameValidation,
    draft_empty: bool,
) -> Option<&'static str> {
    match validation {
        NameValidation::Invalid if draft_empty => Some(if is_thread {
            "channelSetting.fields.threadName.emptyError"
        } else {
            "channelSetting.fields.channelName.emptyError"
        }),
        NameValidation::Invalid => Some(if is_thread {
            "channelSetting.fields.threadName.errorMessage"
        } else {
            "channelSetting.fields.channelName.errorMessage"
        }),
        NameValidation::Duplicate => Some(if is_thread {
            "channelSetting.fields.threadName.duplicateError"
        } else {
            "channelSetting.fields.channelName.duplicateError"
        }),
        NameValidation::None | NameValidation::Valid => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_creator_owner_or_admin_can_edit_overview() {
        assert!(can_edit_overview(true, false, false, false, true));
        assert!(can_edit_overview(true, false, true, false, false));
        assert!(can_edit_overview(true, false, false, true, false));
        assert!(can_edit_overview(true, true, false, false, false));
    }

    #[test]
    fn stranger_cannot_edit_thread_overview_without_manage() {
        assert!(!can_edit_overview(true, false, false, false, false));
    }

    #[test]
    fn regular_channel_overview_does_not_unlock_for_creator_only() {
        assert!(!can_edit_overview(false, false, false, false, true));
        assert!(!can_edit_overview(false, false, true, false, false));
        assert!(can_edit_overview(false, true, false, false, false));
    }

    #[test]
    fn thread_validation_uses_thread_name_keys() {
        assert_eq!(
            overview_name_validation_key(true, NameValidation::Invalid, true),
            Some("channelSetting.fields.threadName.emptyError")
        );
        assert_eq!(
            overview_name_validation_key(true, NameValidation::Invalid, false),
            Some("channelSetting.fields.threadName.errorMessage")
        );
        assert_eq!(
            overview_name_validation_key(true, NameValidation::Duplicate, false),
            Some("channelSetting.fields.threadName.duplicateError")
        );
    }

    #[test]
    fn channel_validation_keeps_channel_name_keys() {
        assert_eq!(
            overview_name_validation_key(false, NameValidation::Invalid, true),
            Some("channelSetting.fields.channelName.emptyError")
        );
        assert_eq!(
            overview_name_validation_key(false, NameValidation::Duplicate, false),
            Some("channelSetting.fields.channelName.duplicateError")
        );
    }
}
