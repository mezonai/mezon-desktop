use gpui::{
    App, ClipboardItem, Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription,
    Window, deferred, div, img, prelude::*, px,
};
use mezon_store::{
    AppConfig, ChannelId, ChannelList, ChannelType, ChannelWebhook, ClanId, ClanMembersEvent,
    ClanMembersStore, PlatformStore, Settings, UserId, WEBHOOK_NAME_MAX_LENGTH, WebhookEvent,
    WebhookStore, webhook_name_is_valid,
};

use crate::app::shell::Shell;
use crate::chat::message::format_i18n_full_date_from_seconds;
use crate::clan::settings::{random_webhook_avatar, random_webhook_name, upload_webhook_avatar};
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Modal, Sizable,
    Size, Spinner, h_flex, v_flex,
};
use crate::image_cache::shared_avatar_cache;
use crate::theme::{ActiveTheme, Theme};

const LEARN_MORE_INTEGRATIONS: &str = "https://mezon.ai/docs/en/developer/webhooks/overview";
const LEARN_MORE_CHANNEL_WEBHOOK: &str =
    "https://mezon.ai/docs/en/developer/webhooks/channel-webhook";

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntegrationsView {
    Landing,
    Webhooks,
}

enum PendingDiscard {
    Collapse,
    Expand(String),
    OpenLanding,
}

#[derive(Clone)]
struct ChannelOption {
    id: ChannelId,
    label: SharedString,
}

#[derive(Clone)]
struct WebhookRow {
    id: String,
    name: String,
    avatar: String,
    created_label: String,
}

struct EditBaseline {
    name: String,
    avatar: String,
    channel_id: ChannelId,
}

pub struct IntegrationsTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    view: IntegrationsView,
    creating: bool,
    saving: bool,
    expanded_id: Option<String>,
    edit_name: Option<Entity<InputState>>,
    edit_avatar: Option<String>,
    edit_channel_id: Option<ChannelId>,
    edit_input_sub: Option<Subscription>,
    channel_menu_open: bool,
    avatar_uploading: bool,
    edit_baseline: Option<EditBaseline>,
    rows: Vec<WebhookRow>,
    channel_options: Vec<ChannelOption>,
    discard_confirm: Option<PendingDiscard>,
    _subs: Vec<Subscription>,
}

impl IntegrationsTab {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        WebhookStore::global(cx).update(cx, |store, cx| {
            store.ensure_channel_webhooks_loaded(clan_id, cx);
        });
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });
        ChannelList::global(cx).update(cx, |store, cx| store.load_for_clan(clan_id, cx));

        let webhook_store = WebhookStore::global(cx);
        let members = ClanMembersStore::global(cx);
        let channel_list = ChannelList::global(cx);
        let subs = vec![
            cx.observe(&settings, |this, _, cx| {
                this.rebuild_rows(cx);
                cx.notify();
            }),
            cx.observe(&channel_list, |this, _, cx| {
                this.rebuild_channel_options(cx);
                cx.notify();
            }),
            cx.subscribe(&webhook_store, |this, _, event, cx| {
                if matches!(
                    event,
                    WebhookEvent::ChannelWebhooksChanged { clan_id } if *clan_id == this.clan_id
                ) {
                    this.cleanup_stale_edit_state(cx);
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&members, |this, _, event: &ClanMembersEvent, cx| {
                if event.clan_id() == this.clan_id {
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            }),
        ];

        let mut this = Self {
            clan_id,
            channel_id,
            settings,
            view: IntegrationsView::Landing,
            creating: false,
            saving: false,
            expanded_id: None,
            edit_name: None,
            edit_avatar: None,
            edit_channel_id: None,
            edit_input_sub: None,
            channel_menu_open: false,
            avatar_uploading: false,
            edit_baseline: None,
            rows: Vec::new(),
            channel_options: Vec::new(),
            discard_confirm: None,
            _subs: subs,
        };
        this.rebuild_rows(cx);
        this.rebuild_channel_options(cx);
        this
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn rebuild_rows(&mut self, cx: &App) {
        let locale = self.locale(cx);
        self.rows = WebhookStore::global(cx)
            .read(cx)
            .channel_webhooks_for_channel(self.clan_id, self.channel_id)
            .into_iter()
            .map(|webhook| {
                let created =
                    format_i18n_full_date_from_seconds(webhook.create_time_seconds, &locale);
                let creator = self.creator_name(webhook.creator_id, cx);
                let created_label =
                    mezon_i18n::t(&locale, "clanIntegrationsSetting.webhooksItem.createdBy")
                        .replace("{{webhookCreateTime}}", &created)
                        .replace("{{webhookUserOwnerName}}", &creator);
                WebhookRow {
                    id: webhook.id.clone(),
                    name: webhook.webhook_name.clone(),
                    avatar: webhook.avatar.clone(),
                    created_label,
                }
            })
            .collect();
    }

    fn webhook_by_id(&self, webhook_id: &str, cx: &App) -> Option<ChannelWebhook> {
        WebhookStore::global(cx)
            .read(cx)
            .channel_webhooks_for_clan(self.clan_id)
            .iter()
            .find(|webhook| webhook.id == webhook_id)
            .cloned()
    }

    fn creator_name(&self, creator_id: UserId, cx: &App) -> String {
        ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, creator_id)
            .map(|member| member.user.username.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    fn rebuild_channel_options(&mut self, cx: &App) {
        self.channel_options = ChannelList::global(cx)
            .read(cx)
            .categories_for_clan(self.clan_id)
            .iter()
            .flat_map(|category| category.channels.iter())
            .filter(|channel| {
                channel.channel_type == ChannelType::Text && channel.parent_id.is_none()
            })
            .map(|channel| ChannelOption {
                id: channel.id,
                label: channel.name.clone().into(),
            })
            .collect();
    }

    fn open_webhooks(&mut self, cx: &mut Context<Self>) {
        self.view = IntegrationsView::Webhooks;
        cx.notify();
    }

    fn open_landing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_discard(PendingDiscard::OpenLanding, window, cx);
    }

    fn discard_edit_state(&mut self) {
        self.expanded_id = None;
        self.edit_name = None;
        self.edit_avatar = None;
        self.edit_channel_id = None;
        self.edit_input_sub = None;
        self.channel_menu_open = false;
        self.avatar_uploading = false;
        self.edit_baseline = None;
        self.discard_confirm = None;
    }

    fn draft_matches_baseline(&self, cx: &App) -> bool {
        let Some(baseline) = &self.edit_baseline else {
            return false;
        };
        self.draft_name(cx).trim() == baseline.name.trim()
            && self.edit_avatar.as_deref() == Some(baseline.avatar.as_str())
            && self.edit_channel_id == Some(baseline.channel_id)
    }

    fn set_baseline_from_webhook(&mut self, webhook: &ChannelWebhook) {
        self.edit_baseline = Some(EditBaseline {
            name: webhook.webhook_name.clone(),
            avatar: webhook.avatar.clone(),
            channel_id: webhook.channel_id,
        });
    }

    fn sync_editor_if_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(expanded_id) = self.expanded_id.clone() else {
            return;
        };
        let Some(webhook) = self.webhook_by_id(&expanded_id, cx) else {
            self.discard_edit_state();
            return;
        };
        if !self.draft_matches_baseline(cx) {
            return;
        }
        let already_synced = self.edit_baseline.as_ref().is_some_and(|baseline| {
            baseline.name == webhook.webhook_name
                && baseline.avatar == webhook.avatar
                && baseline.channel_id == webhook.channel_id
        });
        if already_synced {
            return;
        }
        if let Some(input) = self.edit_name.clone() {
            input.update(cx, |state, cx| {
                state.set_value(&webhook.webhook_name, window, cx);
            });
        }
        self.edit_avatar = Some(webhook.avatar.clone());
        self.edit_channel_id = Some(webhook.channel_id);
        self.set_baseline_from_webhook(&webhook);
    }

    fn cleanup_stale_edit_state(&mut self, cx: &App) {
        let Some(expanded_id) = self.expanded_id.as_deref() else {
            return;
        };
        if self.webhook_by_id(expanded_id, cx).is_none() {
            self.discard_edit_state();
        }
    }

    fn toggle_expand(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let pending = if self.expanded_id.as_ref() == Some(&id) {
            PendingDiscard::Collapse
        } else {
            PendingDiscard::Expand(id)
        };
        self.request_discard(pending, window, cx);
    }

    fn request_discard(
        &mut self,
        pending: PendingDiscard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_dirty(cx) {
            self.discard_confirm = Some(pending);
            cx.notify();
            return;
        }
        self.apply_pending_discard(pending, window, cx);
    }

    fn apply_pending_discard(
        &mut self,
        pending: PendingDiscard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.discard_confirm = None;
        match pending {
            PendingDiscard::Collapse => self.discard_edit_state(),
            PendingDiscard::Expand(id) => {
                self.discard_edit_state();
                self.expanded_id = Some(id.clone());
                self.ensure_edit_state(&id, window, cx);
                if self.edit_name.is_none() {
                    self.discard_edit_state();
                }
            }
            PendingDiscard::OpenLanding => {
                self.discard_edit_state();
                self.view = IntegrationsView::Landing;
            }
        }
        cx.notify();
    }

    fn cancel_discard(&mut self, cx: &mut Context<Self>) {
        self.discard_confirm = None;
        cx.notify();
    }

    fn confirm_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.discard_confirm.take() else {
            return;
        };
        self.apply_pending_discard(pending, window, cx);
    }

    fn ensure_edit_state(&mut self, webhook_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(webhook) = self.webhook_by_id(webhook_id, cx) else {
            return;
        };
        let input_bg = cx.theme().tokens.theme_setting_primary;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .height(px(50.0))
                .text_size(px(14.0))
                .padding_x(px(10.0))
                .borderless()
                .bg(input_bg)
        });
        input.update(cx, |state, cx| {
            state.set_value(&webhook.webhook_name, window, cx);
        });
        self.edit_input_sub = Some(cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
            if *event == InputEvent::Change {
                cx.notify();
            }
        }));
        self.edit_name = Some(input);
        self.edit_avatar = Some(webhook.avatar.clone());
        self.edit_channel_id = Some(webhook.channel_id);
        self.set_baseline_from_webhook(&webhook);
    }

    fn draft_name(&self, cx: &App) -> String {
        self.edit_name
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default()
    }

    fn is_dirty(&self, cx: &App) -> bool {
        let Some(webhook_id) = self.expanded_id.as_deref() else {
            return false;
        };
        let Some(webhook) = self.webhook_by_id(webhook_id, cx) else {
            return false;
        };
        let name_changed = self.draft_name(cx).trim() != webhook.webhook_name.trim();
        let avatar_changed = self
            .edit_avatar
            .as_deref()
            .is_some_and(|avatar| avatar != webhook.avatar);
        let channel_changed = self
            .edit_channel_id
            .is_some_and(|channel_id| channel_id != webhook.channel_id);
        name_changed || avatar_changed || channel_changed
    }

    pub fn should_show_save_bar(&self, cx: &App) -> bool {
        self.view == IntegrationsView::Webhooks && self.is_dirty(cx)
    }

    pub fn can_save(&self, cx: &App) -> bool {
        self.should_show_save_bar(cx) && !self.saving && webhook_name_is_valid(&self.draft_name(cx))
    }

    pub fn reset_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(webhook_id) = self.expanded_id.clone() else {
            return;
        };
        let Some(webhook) = self.webhook_by_id(&webhook_id, cx) else {
            return;
        };
        if let Some(input) = self.edit_name.clone() {
            input.update(cx, |state, cx| {
                state.set_value(&webhook.webhook_name, window, cx);
            });
        }
        self.edit_avatar = Some(webhook.avatar.clone());
        self.edit_channel_id = Some(webhook.channel_id);
        self.channel_menu_open = false;
        self.set_baseline_from_webhook(&webhook);
        cx.notify();
    }

    pub fn save_draft(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let Some(webhook_id) = self.expanded_id.clone() else {
            return;
        };
        let Some(webhook) = self.webhook_by_id(&webhook_id, cx) else {
            return;
        };
        let name = self.draft_name(cx).trim().to_string();
        let avatar = self
            .edit_avatar
            .clone()
            .unwrap_or_else(|| webhook.avatar.clone());
        let channel_id_update = self.edit_channel_id;
        self.saving = true;
        cx.notify();
        let task = WebhookStore::global(cx).update(cx, |store, cx| {
            store.update_channel_webhook(&webhook, name, avatar, channel_id_update, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.edit_baseline = Some(EditBaseline {
                            name: this.draft_name(cx).trim().to_string(),
                            avatar: this.edit_avatar.clone().unwrap_or_default(),
                            channel_id: this.edit_channel_id.unwrap_or(this.channel_id),
                        });
                    }
                    Err(err) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.error(err, cx));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_delete_webhook(
        &self,
        webhook_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(webhook) = self.webhook_by_id(webhook_id, cx) else {
            return;
        };
        let locale = self.locale(cx);
        Shell::global(cx).update(cx, |shell, cx| {
            shell.confirm_delete_channel_webhook(webhook, &locale, window, cx);
        });
    }

    fn copy_webhook_url(&self, webhook_id: &str, cx: &mut App) {
        let Some(url) = self
            .webhook_by_id(webhook_id, cx)
            .map(|webhook| webhook.url)
        else {
            return;
        };
        let locale = self.locale(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        Shell::global(cx).update(cx, |shell, cx| {
            shell.success(
                mezon_i18n::t(&locale, "clanIntegrationsSetting.webhooksEdit.copied"),
                cx,
            );
        });
    }

    fn pick_webhook_avatar(&mut self, cx: &mut Context<Self>) {
        if self.avatar_uploading || self.expanded_id.is_none() {
            return;
        }
        self.avatar_uploading = true;
        cx.notify();
        let locale = self.locale(cx);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                mezon_i18n::t(&locale, "clanSettings.clanLogo.uploadImage")
                    .to_string()
                    .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let paths = match rx.await {
                Ok(Ok(Some(paths))) => paths,
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        this.avatar_uploading = false;
                        cx.notify();
                    });
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                let _ = this.update(cx, |this, cx| {
                    this.avatar_uploading = false;
                    cx.notify();
                });
                return;
            };
            match upload_webhook_avatar(path, &locale, cx).await {
                Ok(Some(url)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.edit_avatar = Some(url);
                        this.avatar_uploading = false;
                        cx.notify();
                    });
                }
                Ok(None) => {
                    let _ = this.update(cx, |this, cx| {
                        this.avatar_uploading = false;
                        cx.notify();
                    });
                }
                Err(message) => {
                    let _ = this.update(cx, |this, cx| {
                        this.avatar_uploading = false;
                        cx.notify();
                    });
                    cx.update(|cx| {
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                    });
                }
            }
        })
        .detach();
    }

    fn create_webhook(&mut self, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let base_img = AppConfig::try_global(cx)
            .map(|cfg| cfg.base_img_url.clone())
            .unwrap_or_else(|| AppConfig::dev_defaults().base_img_url);
        let name = random_webhook_name();
        let avatar = random_webhook_avatar(&base_img);
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        self.creating = true;
        cx.notify();
        let task = WebhookStore::global(cx).update(cx, |store, cx| {
            store.create_channel_webhook(clan_id, channel_id, name.clone(), avatar, cx)
        });
        let locale = self.locale(cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                match &result {
                    Ok(()) => {
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.success(
                                mezon_i18n::t(&locale, "integrations.toast.generateSuccess")
                                    .replace("{{name}}", &name),
                                cx,
                            );
                        });
                    }
                    Err(err) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.error(err.clone(), cx));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_new_webhook_button(
        &self,
        id: &'static str,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let creating = self.creating;
        h_flex()
            .id(id)
            .flex_none()
            .items_center()
            .justify_center()
            .px(px(16.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .gap_1()
            .bg(theme.brand)
            .text_color(theme.text_primary)
            .when(creating, |el| el.opacity(0.6))
            .when(!creating, |el| {
                el.cursor_pointer()
                    .hover(|s| s.bg(theme.brand_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.create_webhook(cx);
                    }))
            })
            .when(creating, |el| {
                el.child(Spinner::new().with_size(Size::Small))
            })
            .child(mezon_i18n::t(locale, "integrations.newWebhook"))
    }

    fn render_learn_more(
        id: &'static str,
        label: SharedString,
        url: &'static str,
        theme: &Theme,
    ) -> impl IntoElement {
        div()
            .id(id)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text_link)
            .cursor_pointer()
            .hover(|style| style.underline())
            .on_click(move |_, _, cx| {
                if let Some(store) = PlatformStore::try_global(cx) {
                    let _ = store.read(cx).open_url_external(url);
                }
            })
            .child(label)
    }

    fn render_discard_confirm(&self, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let title = mezon_i18n::t(locale, "integrations.discardChangesConfirm");
        let entity = cx.entity();
        Modal::new(title)
            .on_dismiss({
                let entity = entity.clone();
                move |_, cx| {
                    entity.update(cx, |this, cx| this.cancel_discard(cx));
                }
            })
            .action(
                Button::new("channel-integrations-discard-cancel")
                    .label(mezon_i18n::t(locale, "common.cancel"))
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel_discard(cx);
                    })),
            )
            .action(
                Button::new("channel-integrations-discard-confirm")
                    .label(title)
                    .danger()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.confirm_discard(window, cx);
                    })),
            )
    }

    fn render_description_block(
        description: SharedString,
        learn_more: SharedString,
        learn_more_id: &'static str,
        url: &'static str,
        theme: &Theme,
    ) -> impl IntoElement {
        h_flex()
            .pt(px(20.0))
            .flex_wrap()
            .items_start()
            .gap_1()
            .text_sm()
            .text_color(theme.tokens.text_theme_primary)
            .child(description)
            .child(Self::render_learn_more(
                learn_more_id,
                learn_more,
                url,
                theme,
            ))
    }

    fn render_divider(theme: &Theme) -> impl IntoElement {
        div()
            .my(px(32.0))
            .h(px(1.0))
            .w_full()
            .bg(theme.tokens.border_theme_primary)
    }

    fn render_title(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on_webhooks = self.view == IntegrationsView::Webhooks;
        h_flex()
            .mb(px(20.0))
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("channel-integrations-title")
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .when(on_webhooks, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_landing(window, cx);
                            }))
                    })
                    .child(mezon_i18n::t(locale, "integrations.title")),
            )
            .when(on_webhooks, |el| {
                el.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(20.0))
                        .text_color(theme.text_primary),
                )
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(mezon_i18n::t(locale, "integrations.webhooks")),
                )
            })
    }

    fn render_landing(
        &self,
        rows: &[WebhookRow],
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = rows.len();
        let empty = count == 0;
        v_flex()
            .w_full()
            .child(Self::render_description_block(
                mezon_i18n::t(locale, "integrations.description").into(),
                mezon_i18n::t(locale, "integrations.learnMore").into(),
                "channel-integrations-learn-more",
                LEARN_MORE_INTEGRATIONS,
                theme,
            ))
            .child(Self::render_divider(theme))
            .child(
                h_flex()
                    .id("channel-integrations-webhooks-card")
                    .w_full()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .py(px(20.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .bg(theme.tokens.theme_setting_nav)
                    .when(!empty, |el| {
                        el.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                            this.open_webhooks(cx);
                        }))
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .gap_4()
                            .min_w_0()
                            .child(
                                Icon::new(IconName::WebhooksIcon)
                                    .size(px(20.0))
                                    .flex_shrink_0()
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .pb(px(3.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.tokens.text_theme_primary)
                                            .child(mezon_i18n::t(locale, "integrations.webhooks")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(theme.tokens.text_theme_primary)
                                            .child(webhook_count_label(count, locale)),
                                    ),
                            ),
                    )
                    .child(if empty {
                        Button::new("channel-integrations-create-webhook")
                            .label(mezon_i18n::t(locale, "integrations.createWebhook"))
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_webhooks(cx);
                            }))
                            .into_any_element()
                    } else {
                        h_flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(14.0))
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(locale, "integrations.viewWebhook"))
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(15.0))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .into_any_element()
                    }),
            )
    }

    fn render_channel_picker(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut options = self.channel_options.clone();
        let selected_id = self.edit_channel_id;
        if let Some(selected_id) = selected_id {
            options.sort_by_key(|option| if option.id == selected_id { 0 } else { 1 });
        }
        let selected_label = selected_id
            .and_then(|id| {
                options
                    .iter()
                    .find(|option| option.id == id)
                    .map(|option| option.label.clone())
            })
            .or_else(|| {
                selected_id.and_then(|id| {
                    ChannelList::global(cx)
                        .read(cx)
                        .channel_display_name(self.clan_id, id)
                        .map(SharedString::from)
                })
            })
            .unwrap_or_default();
        let open = self.channel_menu_open;

        v_flex()
            .w_full()
            .relative()
            .child(
                h_flex()
                    .id("channel-integrations-channel-trigger")
                    .h(px(50.0))
                    .w_full()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .rounded_md()
                    .bg(theme.tokens.theme_setting_primary)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.channel_menu_open = !this.channel_menu_open;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(selected_label),
                    )
                    .child(
                        Icon::new(IconName::ArrowDownFill)
                            .size(px(16.0))
                            .flex_shrink_0()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .when(open, |el| {
                el.child(deferred(
                    div()
                        .absolute()
                        .top_full()
                        .left_0()
                        .right_0()
                        .mt(px(4.0))
                        .max_h(px(200.0))
                        .p(px(4.0))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg_floating)
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.channel_menu_open = false;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .id("channel-integrations-channel-menu")
                                .overflow_y_scroll()
                                .max_h(px(192.0))
                                .child(v_flex().children(options.into_iter().map(|option| {
                                    let selected = selected_id == Some(option.id);
                                    let option_id = option.id;
                                    h_flex()
                                        .id(SharedString::from(format!(
                                            "channel-integrations-channel-{}",
                                            option.id.get()
                                        )))
                                        .w_full()
                                        .items_center()
                                        .gap_2()
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .rounded(px(4.0))
                                        .text_sm()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(theme.bg_hover))
                                        .when(selected, |row| {
                                            row.border_1()
                                                .border_color(
                                                    theme.tokens.border_highlight_react_theme,
                                                )
                                                .font_weight(FontWeight::SEMIBOLD)
                                        })
                                        .child(
                                            Icon::new(IconName::Hashtag)
                                                .size(px(16.0))
                                                .flex_shrink_0()
                                                .text_color(theme.text_muted),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(theme.tokens.text_theme_primary)
                                                .child(option.label),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.edit_channel_id = Some(option_id);
                                            this.channel_menu_open = false;
                                            cx.notify();
                                        }))
                                }))),
                        ),
                ))
            })
    }

    fn render_edit_panel(
        &self,
        webhook_id: &str,
        locale: &str,
        theme: &Theme,
        avatar_cache: Entity<crate::image_cache::LruImageCache>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let draft_name = self.draft_name(cx);
        let name_too_long = draft_name.trim().chars().count() > WEBHOOK_NAME_MAX_LENGTH;
        let edit_avatar = self.edit_avatar.clone().unwrap_or_default();
        let copy_id = webhook_id.to_string();
        let delete_id = webhook_id.to_string();
        v_flex()
            .w_full()
            .pt(px(20.0))
            .mt(px(12.0))
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_2()
                    .child(
                        v_flex()
                            .w(gpui::relative(0.25))
                            .items_center()
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        Avatar::new()
                                            .src(edit_avatar)
                                            .name(draft_name.clone())
                                            .size_px(px(100.0))
                                            .image_cache(avatar_cache),
                                    )
                                    .child({
                                        let mut pick = div()
                                            .id("channel-integrations-avatar-pick")
                                            .absolute()
                                            .top(px(-4.0))
                                            .right(px(-4.0))
                                            .size(px(24.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_floating)
                                            .cursor_pointer()
                                            .occlude();
                                        pick.interactivity().on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.pick_webhook_avatar(cx);
                                            },
                                        ));
                                        pick.child(
                                            Icon::new(IconName::SelectFileIcon)
                                                .size(px(12.0))
                                                .text_color(theme.text_secondary),
                                        )
                                    })
                                    .when(self.avatar_uploading, |el| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_full()
                                                .bg(gpui::hsla(0.0, 0.0, 0.0, 0.45))
                                                .child(Spinner::new().with_size(Size::Small)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .mt(px(10.0))
                                    .text_size(px(10.0))
                                    .text_color(theme.text_muted)
                                    .child(mezon_i18n::t(
                                        locale,
                                        "clanIntegrationsSetting.webhooksEdit.recommendImage",
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_6()
                                    .items_start()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .mb(px(10.0))
                                                    .text_size(px(12.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(theme.text_muted)
                                                    .child(
                                                        mezon_i18n::t(
                                                            locale,
                                                            "clanIntegrationsSetting.webhooksEdit.nameLabel",
                                                        )
                                                        .to_uppercase(),
                                                    ),
                                            )
                                            .when_some(self.edit_name.clone(), |el, input| {
                                                el.child(
                                                    div()
                                                        .w_full()
                                                        .rounded_sm()
                                                        .when(name_too_long, |field| {
                                                            field
                                                                .border_1()
                                                                .border_color(theme.danger)
                                                        })
                                                        .child(Input::new(&input)),
                                                )
                                            })
                                            .when(name_too_long, |el| {
                                                el.child(
                                                    div()
                                                        .mt_1()
                                                        .text_xs()
                                                        .text_color(theme.danger)
                                                        .child(mezon_i18n::t(
                                                            locale,
                                                            "clanIntegrationsSetting.webhooksEdit.nameMaxLengthError",
                                                        )),
                                                )
                                            }),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .mb(px(10.0))
                                                    .text_size(px(12.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(theme.text_muted)
                                                    .child(
                                                        mezon_i18n::t(
                                                            locale,
                                                            "clanIntegrationsSetting.webhooksEdit.channel",
                                                        )
                                                        .to_uppercase(),
                                                    ),
                                            )
                                            .child(self.render_channel_picker(theme, cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .my(px(24.0))
                                    .h(px(1.0))
                                    .w_full()
                                    .bg(theme.border),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(20.0))
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "channel-integrations-copy-{copy_id}"
                                        )))
                                        .label(format!(
                                            "{} {}",
                                            mezon_i18n::t(
                                                locale,
                                                "clanIntegrationsSetting.webhooksEdit.copy"
                                            ),
                                            mezon_i18n::t(
                                                locale,
                                                "clanIntegrationsSetting.webhooksEdit.webhookURL"
                                            )
                                        ))
                                        .primary()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_webhook_url(&copy_id, cx);
                                        })),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "channel-integrations-delete-{delete_id}"
                                            )))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.danger)
                                            .cursor_pointer()
                                            .hover(|style| style.underline())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.confirm_delete_webhook(&delete_id, window, cx);
                                            }))
                                            .child(mezon_i18n::t(
                                                locale,
                                                "clanIntegrationsSetting.webhooksEdit.deleteWebhook",
                                            )),
                                    ),
                            ),
                    ),
            )
    }

    fn render_webhooks_list(
        &self,
        rows: &[WebhookRow],
        loading: bool,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let avatar_cache = shared_avatar_cache(cx);
        let entity = cx.entity();
        let mut cards = Vec::new();
        for row in rows {
            let created_label = row.created_label.clone();
            let expanded = self.expanded_id.as_deref() == Some(row.id.as_str());
            let header_avatar = if expanded {
                self.edit_avatar
                    .clone()
                    .unwrap_or_else(|| row.avatar.clone())
            } else {
                row.avatar.clone()
            };
            cards.push(
                render_webhook_row(
                    row,
                    created_label,
                    header_avatar,
                    expanded,
                    theme,
                    avatar_cache.clone(),
                    entity.clone(),
                    self,
                    locale,
                    cx,
                )
                .into_any_element(),
            );
        }
        v_flex()
            .w_full()
            .pb(px(20.0))
            .child(Self::render_description_block(
                mezon_i18n::t(locale, "integrations.webhookDescription").into(),
                mezon_i18n::t(locale, "integrations.learnMoreWebhook").into(),
                "channel-webhooks-learn-more",
                LEARN_MORE_CHANNEL_WEBHOOK,
                theme,
            ))
            .child(Self::render_divider(theme))
            .when(loading && rows.is_empty(), |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .py_4()
                        .child(mezon_i18n::t(locale, "root.loading")),
                )
            })
            .when(!loading && rows.is_empty(), |el| {
                el.child(
                    v_flex()
                        .w_full()
                        .items_center()
                        .gap_4()
                        .child(
                            img(crate::util::assets::EMPTY_WEBHOOK)
                                .w(px(272.0))
                                .h(px(145.0))
                                .flex_none(),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_theme_primary)
                                .child(mezon_i18n::t(locale, "integrations.noWebhooks")),
                        )
                        .child(h_flex().mb(px(24.0)).child(self.render_new_webhook_button(
                            "channel-integrations-new-webhook-empty",
                            locale,
                            theme,
                            cx,
                        ))),
                )
            })
            .when(!cards.is_empty(), |el| {
                el.child(h_flex().mb(px(24.0)).child(self.render_new_webhook_button(
                    "channel-integrations-new-webhook",
                    locale,
                    theme,
                    cx,
                )))
                .children(cards)
            })
    }
}

fn render_webhook_row(
    row: &WebhookRow,
    created_label: String,
    header_avatar: String,
    expanded: bool,
    theme: &Theme,
    avatar_cache: Entity<crate::image_cache::LruImageCache>,
    entity: Entity<IntegrationsTab>,
    tab: &IntegrationsTab,
    locale: &str,
    cx: &mut Context<IntegrationsTab>,
) -> impl IntoElement {
    let id = row.id.clone();
    v_flex()
        .w_full()
        .mb(px(20.0))
        .p(px(20.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme.tokens.border_theme_primary)
        .bg(theme.tokens.theme_setting_nav)
        .child(
            h_flex()
                .id(SharedString::from(format!(
                    "channel-integrations-webhook-{id}"
                )))
                .w_full()
                .gap(px(20.0))
                .items_center()
                .cursor_pointer()
                .on_click({
                    let entity = entity.clone();
                    let id = id.clone();
                    move |_, window, cx| {
                        entity.update(cx, |this, cx| {
                            this.toggle_expand(id.clone(), window, cx);
                        });
                    }
                })
                .child(
                    div().flex_shrink_0().child(
                        Avatar::new()
                            .src(header_avatar)
                            .name(row.name.clone())
                            .size_px(px(50.0))
                            .image_cache(avatar_cache.clone()),
                    ),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_color(theme.text_primary)
                                        .child(row.name.clone()),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            Icon::new(IconName::ClockIcon)
                                                .size(px(16.0))
                                                .flex_shrink_0()
                                                .text_color(theme.tokens.text_theme_primary),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(13.0))
                                                .text_color(theme.tokens.text_theme_primary)
                                                .child(created_label),
                                        ),
                                ),
                        )
                        .child(if expanded {
                            Icon::new(IconName::ChevronDown)
                                .size(px(30.0))
                                .flex_shrink_0()
                                .text_color(theme.text_secondary)
                        } else {
                            Icon::new(IconName::ChevronRight)
                                .size(px(30.0))
                                .flex_shrink_0()
                                .text_color(theme.text_secondary)
                        }),
                ),
        )
        .when(expanded, |card| {
            card.child(tab.render_edit_panel(&id, locale, theme, avatar_cache, cx))
        })
}

fn webhook_count_label(count: usize, locale: &str) -> String {
    let key = if count <= 1 {
        "integrations.webhookCount"
    } else {
        "integrations.webhook_other"
    };
    mezon_i18n::t(locale, key).replace("{{count}}", &count.to_string())
}

pub fn render_channel_integrations_save_bar(
    tab: Entity<IntegrationsTab>,
    locale: &str,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let can_save = tab.read(cx).can_save(cx);
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
                                    Button::new("channel-integrations-reset")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "clanSettings.modalSaveChanges.reset",
                                        ))
                                        .ghost()
                                        .on_click({
                                            let tab = tab.clone();
                                            move |_, window, cx| {
                                                tab.update(cx, |tab, cx| {
                                                    tab.reset_draft(window, cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    Button::new("channel-integrations-save")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "clanSettings.modalSaveChanges.saveChanges",
                                        ))
                                        .primary()
                                        .disabled(!can_save)
                                        .on_click({
                                            let tab = tab.clone();
                                            move |_, _, cx| {
                                                tab.update(cx, |tab, cx| {
                                                    tab.save_draft(cx);
                                                });
                                            }
                                        }),
                                ),
                        ),
                ),
        )
}

impl gpui::Render for IntegrationsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_editor_if_clean(window, cx);
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        let rows = self.rows.clone();
        let loading = WebhookStore::global(cx)
            .read(cx)
            .channel_webhooks_loading(self.clan_id);

        v_flex()
            .id("channel-integrations-tab")
            .relative()
            .w_full()
            .when(self.should_show_save_bar(cx), |el| el.pb(px(72.0)))
            .child(self.render_title(&locale, &theme, cx))
            .map(|el| match self.view {
                IntegrationsView::Landing => {
                    el.child(self.render_landing(&rows, &locale, &theme, cx))
                }
                IntegrationsView::Webhooks => {
                    el.child(self.render_webhooks_list(&rows, loading, &locale, &theme, cx))
                }
            })
            .when(self.discard_confirm.is_some(), |el| {
                el.child(self.render_discard_confirm(&locale, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{LEARN_MORE_CHANNEL_WEBHOOK, LEARN_MORE_INTEGRATIONS, webhook_count_label};

    #[test]
    fn webhook_count_uses_singular_for_zero_and_one() {
        assert_eq!(webhook_count_label(0, "en"), "0 webhook");
        assert_eq!(webhook_count_label(1, "en"), "1 webhook");
    }

    #[test]
    fn webhook_count_uses_plural_from_two() {
        assert_eq!(webhook_count_label(2, "en"), "2 webhooks");
        assert_eq!(webhook_count_label(12, "en"), "12 webhooks");
    }

    #[test]
    fn delete_webhook_uses_single_i18n_key() {
        assert_eq!(
            mezon_i18n::t("en", "clanIntegrationsSetting.webhooksEdit.deleteWebhook"),
            "Delete Webhook"
        );
        assert_eq!(
            mezon_i18n::t("vi", "clanIntegrationsSetting.webhooksEdit.deleteWebhook"),
            "Xóa Webhook"
        );
    }

    #[test]
    fn no_webhooks_matches_electron_integrations_corpus() {
        assert_eq!(
            mezon_i18n::t("en", "integrations.noWebhooks"),
            "You have no webhooks!"
        );
        assert_eq!(
            mezon_i18n::t("vi", "integrations.noWebhooks"),
            "Bạn chưa có webhook nào!"
        );
        for locale in [
            "en", "vi", "ru", "ukr", "es", "tt", "de", "it", "pt", "jpn", "pl", "kr", "swe", "blr",
            "fr", "nl",
        ] {
            let label = mezon_i18n::t(locale, "integrations.noWebhooks");
            assert_ne!(label, "integrations.noWebhooks");
            assert!(
                !label.to_ascii_lowercase().contains("click here"),
                "{locale} still has a create CTA: {label}"
            );
        }
    }

    #[test]
    fn loading_uses_existing_root_key() {
        assert_eq!(mezon_i18n::t("en", "root.loading"), "Loading...");
        assert_ne!(mezon_i18n::t("vi", "root.loading"), "root.loading");
    }

    #[test]
    fn discard_confirm_copy() {
        assert_eq!(
            mezon_i18n::t("vi", "integrations.discardChangesConfirm"),
            "Bỏ thay đổi?"
        );
        assert_eq!(
            mezon_i18n::t("en", "integrations.discardChangesConfirm"),
            "Discard changes?"
        );
    }

    #[test]
    fn learn_more_urls_match_electron() {
        assert_eq!(
            LEARN_MORE_INTEGRATIONS,
            "https://mezon.ai/docs/en/developer/webhooks/overview"
        );
        assert_eq!(
            LEARN_MORE_CHANNEL_WEBHOOK,
            "https://mezon.ai/docs/en/developer/webhooks/channel-webhook"
        );
    }
}
