use gpui::{
    App, Context, Entity, FontWeight, PathPromptOptions, Render, SharedString, Subscription,
    Window, div, prelude::*, px,
};

use mezon_store::{
    AppConfig, ClanId, ClanMembersStore, ClanWebhook, Settings, WEBHOOK_NAME_MAX_LENGTH,
    WebhookStore,
};

use super::integration_setting_page::{
    random_webhook_avatar, random_webhook_name, render_webhook_create_box,
    render_webhook_empty_box, render_webhook_url_field, upload_webhook_avatar,
};
use crate::app::shell::Shell;
use crate::chat::message::format_relative_time_from_seconds;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Sizable, Size,
    Spinner, h_flex, v_flex,
};
use crate::image_cache::shared_avatar_cache;
use crate::theme::ActiveTheme;

pub struct ClanWebhookTab {
    clan_id: ClanId,
    settings: Entity<Settings>,
    can_manage: bool,
    expanded_id: Option<String>,
    edit_names: std::collections::HashMap<String, Entity<InputState>>,
    edit_avatars: std::collections::HashMap<String, String>,
    edit_input_subs: std::collections::HashMap<String, Subscription>,
    avatar_uploading: std::collections::HashSet<String>,
    creating: bool,
    _webhook_sub: Subscription,
    _members_sub: Subscription,
}

impl ClanWebhookTab {
    pub fn new(
        clan_id: ClanId,
        settings: Entity<Settings>,
        can_manage: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        WebhookStore::global(cx).update(cx, |store, cx| {
            store.ensure_clan_webhooks_loaded(clan_id, cx);
        });
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });

        let webhook_sub = cx.subscribe(&WebhookStore::global(cx), |this, _, event, cx| {
            if matches!(
                event,
                mezon_store::WebhookEvent::ClanWebhooksChanged { clan_id } if *clan_id == this.clan_id
            ) {
                this.cleanup_stale_edit_state(cx);
                cx.notify();
            }
        });
        let members_sub = cx.subscribe(&ClanMembersStore::global(cx), |_, _, _, cx| {
            cx.notify();
        });

        Self {
            clan_id,
            settings,
            can_manage,
            expanded_id: None,
            edit_names: std::collections::HashMap::new(),
            edit_avatars: std::collections::HashMap::new(),
            edit_input_subs: std::collections::HashMap::new(),
            avatar_uploading: std::collections::HashSet::new(),
            creating: false,
            _webhook_sub: webhook_sub,
            _members_sub: members_sub,
        }
    }

    fn locale(&self, cx: &gpui::App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn create_webhook(&mut self, cx: &mut Context<Self>) {
        if !self.can_manage || self.creating {
            return;
        }
        let base_img = AppConfig::try_global(cx)
            .map(|cfg| cfg.base_img_url.clone())
            .unwrap_or_else(|| AppConfig::dev_defaults().base_img_url);
        let name = random_webhook_name();
        let avatar = random_webhook_avatar(&base_img);
        let clan_id = self.clan_id;
        self.creating = true;
        cx.notify();
        let task = WebhookStore::global(cx).update(cx, |store, cx| {
            store.create_clan_webhook(clan_id, name, avatar, cx)
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
                                mezon_i18n::t(&locale, "clanIntegrationsSetting.toast.addSuccess"),
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

    fn toggle_expand(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_manage {
            return;
        }
        if self.expanded_id.as_ref() == Some(&id) {
            self.discard_edit_state(&id);
            self.expanded_id = None;
        } else {
            if let Some(prev) = self.expanded_id.take() {
                self.discard_edit_state(&prev);
            }
            self.expanded_id = Some(id.clone());
            self.ensure_name_input(&id, window, cx);
        }
        cx.notify();
    }

    fn discard_edit_state(&mut self, webhook_id: &str) {
        self.edit_names.remove(webhook_id);
        self.edit_avatars.remove(webhook_id);
        self.edit_input_subs.remove(webhook_id);
        self.avatar_uploading.remove(webhook_id);
    }

    fn pick_webhook_avatar(&mut self, webhook_id: &str, cx: &mut Context<Self>) {
        if self.avatar_uploading.contains(webhook_id) {
            return;
        }
        self.avatar_uploading.insert(webhook_id.to_string());
        cx.notify();

        let locale = self.locale(cx);
        let webhook_id = webhook_id.to_string();
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
            let finish = |this: &mut ClanWebhookTab| {
                this.avatar_uploading.remove(&webhook_id);
            };
            let Some(paths) = crate::util::file_dialog::resolve(rx, cx).await else {
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    cx.notify();
                });
                return;
            };
            let path = match paths.into_iter().next() {
                Some(p) => p,
                None => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    return;
                }
            };
            match upload_webhook_avatar(path, &locale, cx).await {
                Ok(Some(url)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.edit_avatars.insert(webhook_id.clone(), url);
                        finish(this);
                        cx.notify();
                    });
                }
                Ok(None) => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                }
                Err(message) => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
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

    fn ensure_name_input(&mut self, webhook_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_names.contains_key(webhook_id) {
            return;
        }
        let webhook = WebhookStore::global(cx)
            .read(cx)
            .clan_webhooks_for_clan(self.clan_id)
            .iter()
            .find(|w| w.id == webhook_id)
            .cloned();
        let Some(webhook) = webhook else {
            return;
        };
        let initial = webhook.webhook_name.clone();
        let input_bg = cx.theme().tokens.bg_tertiary;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .height(px(40.0))
                .text_size(px(14.0))
                .borderless()
                .bg(input_bg)
        });
        input.update(cx, |state, cx| state.set_value(&initial, window, cx));
        let sub = cx.subscribe(&input, |_, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                cx.notify();
            }
        });
        let webhook_id = webhook_id.to_string();
        self.edit_names.insert(webhook_id.clone(), input);
        self.edit_avatars
            .insert(webhook_id.clone(), webhook.avatar.clone());
        self.edit_input_subs.insert(webhook_id, sub);
    }

    fn webhook_edit_can_save(&self, webhook: &ClanWebhook, cx: &App) -> bool {
        let Some(input) = self.edit_names.get(&webhook.id) else {
            return false;
        };
        let name = input.read(cx).value().trim();
        if name.is_empty() || name.len() > WEBHOOK_NAME_MAX_LENGTH {
            return false;
        }
        let name_changed = name != webhook.webhook_name.trim();
        let avatar = self
            .edit_avatars
            .get(&webhook.id)
            .map(String::as_str)
            .unwrap_or(webhook.avatar.as_str());
        let avatar_changed = avatar != webhook.avatar;
        name_changed || avatar_changed
    }

    fn save_webhook(&mut self, webhook: &ClanWebhook, cx: &mut Context<Self>) {
        let Some(input) = self.edit_names.get(&webhook.id) else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() || name.len() > WEBHOOK_NAME_MAX_LENGTH {
            return;
        }
        let avatar = self
            .edit_avatars
            .get(&webhook.id)
            .cloned()
            .unwrap_or_else(|| webhook.avatar.clone());
        let task = WebhookStore::global(cx).update(cx, |store, cx| {
            store.update_clan_webhook(webhook, name, avatar, false, cx)
        });
        let locale = self.locale(cx);
        cx.spawn(async move |_, cx| match task.await {
            Ok(()) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.success(
                            mezon_i18n::t(&locale, "clanIntegrationsSetting.toast.saveSuccess"),
                            cx,
                        );
                    });
                });
            }
            Err(err) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(err, cx));
                });
            }
        })
        .detach();
    }

    fn reset_token(&mut self, webhook: &ClanWebhook, cx: &mut Context<Self>) {
        let name = self
            .edit_names
            .get(&webhook.id)
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_else(|| webhook.webhook_name.clone());
        let avatar = self
            .edit_avatars
            .get(&webhook.id)
            .cloned()
            .unwrap_or_else(|| webhook.avatar.clone());
        let task = WebhookStore::global(cx).update(cx, |store, cx| {
            store.update_clan_webhook(webhook, name, avatar, true, cx)
        });
        let locale = self.locale(cx);
        cx.spawn(async move |_, cx| match task.await {
            Ok(()) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.success(
                            mezon_i18n::t(
                                &locale,
                                "clanIntegrationsSetting.toast.resetTokenSuccess",
                            ),
                            cx,
                        );
                    });
                });
            }
            Err(err) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(err, cx));
                });
            }
        })
        .detach();
    }

    fn creator_name(&self, creator_id: mezon_store::UserId, cx: &gpui::App) -> String {
        ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, creator_id)
            .map(|m| m.user.username.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    fn cleanup_stale_edit_state(&mut self, cx: &gpui::App) {
        let ids = WebhookStore::global(cx)
            .read(cx)
            .clan_webhooks_for_clan(self.clan_id)
            .iter()
            .map(|webhook| webhook.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if self
            .expanded_id
            .as_ref()
            .is_some_and(|id| !ids.contains(id))
        {
            self.expanded_id = None;
        }
        self.edit_names.retain(|id, _| ids.contains(id));
        self.edit_avatars.retain(|id, _| ids.contains(id));
        self.edit_input_subs.retain(|id, _| ids.contains(id));
        self.avatar_uploading.retain(|id| ids.contains(id));
    }
}

impl Render for ClanWebhookTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        let entity = cx.entity();
        let (webhooks, loading) = {
            let store = WebhookStore::global(cx).read(cx);
            (
                store.clan_webhooks_for_clan(self.clan_id).to_vec(),
                store.clan_webhooks_loading(self.clan_id),
            )
        };
        let avatar_cache = shared_avatar_cache(cx);
        let now = chrono::Local::now();
        let can_manage = self.can_manage;

        v_flex()
            .w_full()
            .self_stretch()
            .items_stretch()
            .gap_4()
            .child(
                v_flex()
                    .w_full()
                    .pt_2()
                    .px_2()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(
                                &locale,
                                "clanIntegrationsSetting.clanWebhooks.description",
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.brand)
                            .child(mezon_i18n::t(
                                &locale,
                                "clanIntegrationsSetting.clanWebhooks.tips",
                            )),
                    ),
            )
            .child(div().border_b_1().border_color(theme.border))
            .when(loading && webhooks.is_empty(), |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .py_4()
                        .child(mezon_i18n::t(&locale, "common.loading")),
                )
            })
            .when(webhooks.is_empty() && !loading && self.can_manage, |el| {
                el.child(render_webhook_create_box(
                    "new-clan-webhook-empty",
                    mezon_i18n::t(&locale, "integrations.noWebhooks").into(),
                    &theme,
                    cx.listener(|this, _, _, cx| this.create_webhook(cx)),
                ))
            })
            .when(webhooks.is_empty() && !loading && !self.can_manage, |el| {
                el.child(render_webhook_empty_box(
                    "clan-webhooks-empty",
                    mezon_i18n::t(&locale, "integrations.noWebhooks").into(),
                    &theme,
                ))
            })
            .children(webhooks.iter().map(|webhook| {
                let id = webhook.id.clone();
                let expanded = can_manage && self.expanded_id.as_ref() == Some(&id);
                let creator = self.creator_name(webhook.creator_id, cx);
                let created =
                    format_relative_time_from_seconds(webhook.create_time_seconds, &locale, now);
                let created_label =
                    mezon_i18n::t(&locale, "clanIntegrationsSetting.webhooksItem.createdBy")
                        .replace("{{webhookCreateTime}}", &created)
                        .replace("{{webhookUserOwnerName}}", &creator);
                let webhook_for_expand = webhook.clone();
                let ent = entity.clone();
                let locale_for_edit = locale.clone();
                let edit_avatar = self
                    .edit_avatars
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| webhook.avatar.clone());
                let avatar_uploading = self.avatar_uploading.contains(&id);
                let header_avatar = if expanded {
                    edit_avatar.clone()
                } else {
                    webhook.avatar.clone()
                };

                v_flex()
                    .w_full()
                    .self_stretch()
                    .mb_1()
                    .p_4()
                    .rounded_md()
                    .bg(theme.tokens.theme_setting_nav)
                    .border_1()
                    .border_color(theme.border)
                    .child({
                        let mut header = h_flex()
                            .w_full()
                            .id(SharedString::from(format!("clan-webhook-expand-{id}")))
                            .gap_4()
                            .items_center();
                        if can_manage {
                            header = header.cursor_pointer().on_click({
                                let id = id.clone();
                                let ent = ent.clone();
                                move |_, window, cx| {
                                    let expand_id = id.clone();
                                    ent.update(cx, |this, cx| {
                                        this.toggle_expand(expand_id, window, cx);
                                    });
                                }
                            });
                        }
                        header
                            .child(
                                div()
                                    .relative()
                                    .flex_shrink_0()
                                    .child(
                                        Avatar::new()
                                            .src(header_avatar)
                                            .name(webhook.webhook_name.clone())
                                            .size_px(px(50.0))
                                            .image_cache(avatar_cache.clone()),
                                    )
                                    .when(expanded, |el| {
                                        el.child({
                                            let mut pick_btn = div()
                                                .id(SharedString::from(format!(
                                                    "clan-webhook-avatar-pick-{id}"
                                                )))
                                                .absolute()
                                                .top(px(-8.0))
                                                .right(px(-8.0))
                                                .size(px(20.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_full()
                                                .border_1()
                                                .border_color(theme.border)
                                                .bg(theme.bg_floating)
                                                .cursor_pointer()
                                                .occlude();
                                            pick_btn.interactivity().on_click({
                                                let ent = ent.clone();
                                                let webhook_id = id.clone();
                                                move |_, _, cx| {
                                                    ent.update(cx, |this, cx| {
                                                        this.pick_webhook_avatar(&webhook_id, cx);
                                                    });
                                                }
                                            });
                                            pick_btn.child(
                                                Icon::new(IconName::SelectFileIcon)
                                                    .size(px(12.0))
                                                    .text_color(theme.text_secondary),
                                            )
                                        })
                                    })
                                    .when(expanded && avatar_uploading, |el| {
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
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(webhook.webhook_name.clone()),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::ClockIcon)
                                                    .size(px(14.0))
                                                    .text_color(theme.text_muted),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.text_muted)
                                                    .child(created_label),
                                            ),
                                    ),
                            )
                            .when(can_manage, |row| {
                                row.child(if expanded {
                                    Icon::new(IconName::ChevronDown)
                                        .size(px(20.0))
                                        .text_color(theme.text_secondary)
                                } else {
                                    Icon::new(IconName::ChevronRight)
                                        .size(px(20.0))
                                        .text_color(theme.text_secondary)
                                })
                            })
                    })
                    .when(expanded, |card| {
                        card.child(render_clan_webhook_edit(
                            &webhook_for_expand,
                            &self.edit_names,
                            self.webhook_edit_can_save(&webhook_for_expand, cx),
                            &locale_for_edit,
                            &theme,
                            entity.clone(),
                        ))
                    })
            }))
            .when(!webhooks.is_empty() && self.can_manage, |el| {
                el.child(render_webhook_create_box(
                    "new-clan-webhook",
                    mezon_i18n::t(&locale, "integrations.newClanWebhook").into(),
                    &theme,
                    cx.listener(|this, _, _, cx| this.create_webhook(cx)),
                ))
            })
    }
}

fn render_clan_webhook_edit(
    webhook: &ClanWebhook,
    edit_names: &std::collections::HashMap<String, Entity<InputState>>,
    can_save: bool,
    locale: &str,
    theme: &crate::theme::Theme,
    entity: Entity<ClanWebhookTab>,
) -> impl IntoElement {
    let Some(name_input) = edit_names.get(&webhook.id) else {
        return div().into_any_element();
    };
    let url = webhook.url.clone();
    let webhook_save = webhook.clone();
    let webhook_reset = webhook.clone();
    let webhook_delete = webhook.clone();
    let entity_save = entity.clone();
    let entity_reset = entity.clone();
    let locale_delete = locale.to_string();

    v_flex()
        .mt_4()
        .pt_4()
        .border_t_1()
        .border_color(theme.border)
        .gap_4()
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(
                            locale,
                            "clanIntegrationsSetting.webhooksEdit.nameLabel",
                        )),
                )
                .child(Input::new(name_input)),
        )
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(
                            locale,
                            "clanIntegrationsSetting.webhooksEdit.webhookURL",
                        )),
                )
                .child(render_webhook_url_field(
                    SharedString::from(format!("clan-webhook-url-{}", webhook.id)),
                    url,
                    locale,
                    theme,
                )),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new(SharedString::from(format!(
                        "reset-clan-token-{}",
                        webhook.id
                    )))
                    .label(mezon_i18n::t(
                        locale,
                        "clanIntegrationsSetting.webhooksEdit.resetToken",
                    ))
                    .ghost()
                    .with_size(Size::Large)
                    .on_click({
                        move |_, _, cx| {
                            entity_reset.update(cx, |this, cx| {
                                this.reset_token(&webhook_reset, cx);
                            });
                        }
                    }),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "delete-clan-webhook-{}",
                        webhook.id
                    )))
                    .label(mezon_i18n::t(
                        locale,
                        "clanIntegrationsSetting.webhooksEdit.delete",
                    ))
                    .danger()
                    .with_size(Size::Large)
                    .w(px(100.0))
                    .on_click({
                        move |_, window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| {
                                shell.confirm_delete_clan_webhook(
                                    webhook_delete.clone(),
                                    &locale_delete,
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "save-clan-webhook-{}",
                        webhook.id
                    )))
                    .label(mezon_i18n::t(
                        locale,
                        "clanIntegrationsSetting.webhooksEdit.save",
                    ))
                    .primary()
                    .with_size(Size::Large)
                    .w(px(100.0))
                    .disabled(!can_save)
                    .on_click({
                        move |_, _, cx| {
                            entity_save.update(cx, |this, cx| {
                                this.save_webhook(&webhook_save, cx);
                            });
                        }
                    }),
                ),
        )
        .into_any_element()
}
