use gpui::{
    App, Context, Entity, FontWeight, PathPromptOptions, Render, SharedString, Subscription,
    Window, deferred, div, prelude::*, px,
};

use mezon_store::{
    AppConfig, ChannelId, ChannelList, ChannelType, ChannelWebhook, ClanId, ClanMembersStore,
    Settings, WEBHOOK_NAME_MAX_LENGTH, WebhookStore,
};

use super::integration_setting_page::{
    random_webhook_avatar, random_webhook_name, render_webhook_create_box,
    render_webhook_url_field, upload_webhook_avatar,
};
use crate::app::shell::Shell;
use crate::chat::message::format_relative_time_from_seconds;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Sizable, Size,
    Spinner, h_flex, v_flex,
};
use crate::image_cache::shared_avatar_cache;
use crate::theme::ActiveTheme;

#[derive(Clone, PartialEq)]
struct ChannelOption {
    id: ChannelId,
    label: SharedString,
    category_name: SharedString,
}

pub struct ChannelWebhookTab {
    clan_id: ClanId,
    channel_list: Entity<ChannelList>,
    settings: Entity<Settings>,
    channel_options: Vec<ChannelOption>,
    selected_channel_index: Option<usize>,
    channel_menu_open: bool,
    expanded_id: Option<String>,
    edit_names: std::collections::HashMap<String, Entity<InputState>>,
    edit_avatars: std::collections::HashMap<String, String>,
    edit_input_subs: std::collections::HashMap<String, Subscription>,
    avatar_uploading: std::collections::HashSet<String>,
    creating: bool,
    _channel_sub: Subscription,
    _webhook_sub: Subscription,
    _members_sub: Subscription,
}

impl ChannelWebhookTab {
    pub fn new(
        clan_id: ClanId,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        channel_list.update(cx, |store, cx| store.load_for_clan(clan_id, cx));
        WebhookStore::global(cx).update(cx, |store, cx| {
            store.ensure_channel_webhooks_loaded(clan_id, cx);
        });
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });

        let channel_options = Self::build_channel_options(&channel_list, clan_id, cx);
        let selected_channel_index = channel_options.first().map(|_| 0);

        let channel_list_for_sub = channel_list.clone();
        Self {
            clan_id,
            channel_list,
            settings,
            channel_options,
            selected_channel_index,
            channel_menu_open: false,
            expanded_id: None,
            edit_names: std::collections::HashMap::new(),
            edit_avatars: std::collections::HashMap::new(),
            edit_input_subs: std::collections::HashMap::new(),
            avatar_uploading: std::collections::HashSet::new(),
            creating: false,
            _channel_sub: cx.subscribe(&channel_list_for_sub, |this, _, _, cx| {
                this.channel_options =
                    Self::build_channel_options(&this.channel_list, this.clan_id, cx);
                if this.selected_channel_index.is_none() && !this.channel_options.is_empty() {
                    this.selected_channel_index = Some(0);
                }
                cx.notify();
            }),
            _webhook_sub: cx.subscribe(&WebhookStore::global(cx), |this, _, event, cx| {
                if matches!(
                    event,
                    mezon_store::WebhookEvent::ChannelWebhooksChanged { clan_id } if *clan_id == this.clan_id
                ) {
                    this.cleanup_stale_edit_state(cx);
                    cx.notify();
                }
            }),
            _members_sub: cx.subscribe(&ClanMembersStore::global(cx), |_, _, _, cx| {
                cx.notify();
            }),
        }
    }

    fn build_channel_options(
        channel_list: &Entity<ChannelList>,
        clan_id: ClanId,
        cx: &App,
    ) -> Vec<ChannelOption> {
        channel_list
            .read(cx)
            .categories_for_clan(clan_id)
            .iter()
            .flat_map(|category| {
                category.channels.iter().filter_map(|channel| {
                    if channel.channel_type != ChannelType::Text || channel.private {
                        return None;
                    }
                    Some(ChannelOption {
                        id: channel.id,
                        label: channel.name.clone().into(),
                        category_name: category.name.to_uppercase().into(),
                    })
                })
            })
            .collect()
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn selected_channel(&self) -> Option<&ChannelOption> {
        self.selected_channel_index
            .and_then(|index| self.channel_options.get(index))
    }

    fn create_webhook(&mut self, cx: &mut Context<Self>) {
        let Some(channel) = self.selected_channel() else {
            return;
        };
        if self.creating {
            return;
        }
        let base_img = AppConfig::try_global(cx)
            .map(|cfg| cfg.base_img_url.clone())
            .unwrap_or_else(|| AppConfig::dev_defaults().base_img_url);
        let name = random_webhook_name();
        let avatar = random_webhook_avatar(&base_img);
        let channel_id = channel.id;
        let clan_id = self.clan_id;
        self.creating = true;
        cx.notify();
        let task = WebhookStore::global(cx).update(cx, |store, cx| {
            store.create_channel_webhook(clan_id, channel_id, name, avatar, cx)
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
        if self.expanded_id.as_ref() == Some(&id) {
            self.discard_edit_state(&id);
            self.expanded_id = None;
        } else {
            if let Some(prev) = self.expanded_id.take() {
                self.discard_edit_state(&prev);
            }
            self.expanded_id = Some(id.clone());
            self.ensure_edit_state(&id, window, cx);
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
            let finish = |this: &mut ChannelWebhookTab| {
                this.avatar_uploading.remove(&webhook_id);
            };
            let paths = match rx.await {
                Ok(Ok(Some(p))) => p,
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    return;
                }
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

    fn ensure_edit_state(&mut self, webhook_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_names.contains_key(webhook_id) {
            return;
        }
        let webhook = WebhookStore::global(cx)
            .read(cx)
            .channel_webhooks_for_clan(self.clan_id)
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

    fn webhook_edit_can_save(&self, webhook: &ChannelWebhook, cx: &App) -> bool {
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

    fn save_webhook(&mut self, webhook: &ChannelWebhook, cx: &mut Context<Self>) {
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
            store.update_channel_webhook(webhook, name, avatar, None, cx)
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

    fn creator_name(&self, creator_id: mezon_store::UserId, cx: &App) -> String {
        ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, creator_id)
            .map(|m| m.user.username.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    fn cleanup_stale_edit_state(&mut self, cx: &App) {
        let ids = WebhookStore::global(cx)
            .read(cx)
            .channel_webhooks_for_clan(self.clan_id)
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

    fn render_filter_channel_picker(
        &mut self,
        theme: &crate::theme::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self
            .selected_channel_index
            .and_then(|index| self.channel_options.get(index));
        let open = self.channel_menu_open;
        let options = self.channel_options.clone();
        let entity = cx.entity();
        let locale = self.locale(cx);
        let selected_channel_index = self.selected_channel_index;

        let mut trigger = h_flex()
            .id("channel-webhook-filter-trigger")
            .h(px(40.0))
            .items_center()
            .gap_2()
            .px(px(12.0))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.bg_tertiary)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .when_some(selected.cloned(), |el, option| {
                        el.child(render_channel_label(&option, theme))
                    })
                    .when(selected.is_none(), |el| {
                        el.text_sm()
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(
                                &locale,
                                "clanIntegrationsSetting.webhookChannelSelect.placeholder",
                            ))
                    }),
            )
            .child(
                Icon::new(IconName::ArrowDownFill)
                    .size(px(16.0))
                    .flex_shrink_0()
                    .text_color(theme.text_muted),
            );
        trigger
            .interactivity()
            .on_click(cx.listener(|this, _, _, cx| {
                this.channel_menu_open = !this.channel_menu_open;
                cx.notify();
            }));

        v_flex()
            .self_stretch()
            .relative()
            .items_stretch()
            .child(trigger)
            .when(open, |menu| {
                menu.child(deferred(
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
                        .bg(theme.surfaces.input_primary)
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            if this.channel_menu_open {
                                this.channel_menu_open = false;
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .id("channel-webhook-filter-menu")
                                .overflow_y_scroll()
                                .max_h(px(192.0))
                                .child(v_flex().children(options.into_iter().enumerate().map(
                                    |(index, option)| {
                                        let is_selected = selected_channel_index == Some(index);
                                        h_flex()
                                            .id(("channel-webhook-filter-item", index))
                                            .w_full()
                                            .items_center()
                                            .px(px(16.0))
                                            .py(px(8.0))
                                            .rounded(px(4.0))
                                            .text_sm()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover))
                                            .when(is_selected, |row| row.bg(theme.bg_hover))
                                            .child(render_channel_label(&option, theme))
                                            .on_click({
                                                let entity = entity.clone();
                                                move |_, _, cx| {
                                                    entity.update(cx, |this, cx| {
                                                        this.selected_channel_index = Some(index);
                                                        this.channel_menu_open = false;
                                                        cx.notify();
                                                    });
                                                }
                                            })
                                    },
                                ))),
                        ),
                ))
            })
    }
}

fn render_channel_label(option: &ChannelOption, theme: &crate::theme::Theme) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .overflow_hidden()
        .child(
            Icon::new(IconName::Hashtag)
                .size(px(16.0))
                .flex_shrink_0()
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(option.label.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(theme.text_muted)
                .child(option.category_name.clone()),
        )
}

fn render_channel_webhook_edit(
    webhook: &ChannelWebhook,
    channel_name: SharedString,
    name_input: &Entity<InputState>,
    can_save: bool,
    locale: &str,
    theme: &crate::theme::Theme,
    entity: Entity<ChannelWebhookTab>,
) -> impl IntoElement {
    let url = webhook.url.clone();
    let webhook_save = webhook.clone();
    let webhook_delete = webhook.clone();
    let entity_save = entity.clone();
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
                            "clanIntegrationsSetting.webhooksEdit.channel",
                        )),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.text_primary)
                        .child(format!("#{channel_name}")),
                ),
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
                    SharedString::from(format!("channel-webhook-url-{}", webhook.id)),
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
                        "delete-channel-webhook-{}",
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
                                shell.confirm_delete_channel_webhook(
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
                        "save-channel-webhook-{}",
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
}

fn render_channel_webhook_item(
    webhook: &ChannelWebhook,
    expanded: bool,
    channel_name: SharedString,
    created_label: SharedString,
    edit_avatar: String,
    avatar_uploading: bool,
    can_save: bool,
    edit_names: &std::collections::HashMap<String, Entity<InputState>>,
    locale: &str,
    theme: &crate::theme::Theme,
    avatar_cache: Entity<crate::image_cache::LruImageCache>,
    entity: Entity<ChannelWebhookTab>,
) -> impl IntoElement {
    let id = webhook.id.clone();
    let expand_id = id.clone();
    let pick_id = id.clone();
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
        .child(
            h_flex()
                .w_full()
                .id(SharedString::from(format!("channel-webhook-expand-{id}")))
                .gap_4()
                .items_center()
                .cursor_pointer()
                .on_click({
                    let ent = entity.clone();
                    move |_, window, cx| {
                        ent.update(cx, |this, cx| {
                            this.toggle_expand(expand_id.clone(), window, cx);
                        });
                    }
                })
                .child(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .child(
                            Avatar::new()
                                .src(header_avatar)
                                .name(webhook.webhook_name.clone())
                                .size_px(px(50.0))
                                .image_cache(avatar_cache),
                        )
                        .when(expanded, |el| {
                            el.child({
                                let mut pick_btn = div()
                                    .id(SharedString::from(format!(
                                        "channel-webhook-avatar-pick-{pick_id}"
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
                                    let entity_pick = entity.clone();
                                    let webhook_id = pick_id.clone();
                                    move |_, _, cx| {
                                        entity_pick.update(cx, |this, cx| {
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
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(format!("#{channel_name} · {created_label}")),
                        ),
                )
                .child(if expanded {
                    Icon::new(IconName::ChevronDown)
                        .size(px(20.0))
                        .text_color(theme.text_secondary)
                } else {
                    Icon::new(IconName::ChevronRight)
                        .size(px(20.0))
                        .text_color(theme.text_secondary)
                }),
        )
        .when(expanded, |card| {
            card.when_some(edit_names.get(&webhook.id), |card, name_input| {
                card.child(render_channel_webhook_edit(
                    webhook,
                    channel_name,
                    name_input,
                    can_save,
                    locale,
                    theme,
                    entity,
                ))
            })
        })
}

impl Render for ChannelWebhookTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        let entity = cx.entity();
        let filter_channel = self.selected_channel().map(|c| c.id);
        let (loading, has_webhooks, filtered_webhook_ids) = {
            let store = WebhookStore::global(cx).read(cx);
            let webhooks = store.channel_webhooks_for_clan(self.clan_id);
            let filtered_webhook_ids = webhooks
                .iter()
                .filter(|webhook| filter_channel.is_none_or(|id| webhook.channel_id == id))
                .map(|webhook| webhook.id.clone())
                .collect::<Vec<_>>();
            let has_webhooks = !filtered_webhook_ids.is_empty();
            let loading = store.channel_webhooks_loading(self.clan_id);
            (loading, has_webhooks, filtered_webhook_ids)
        };
        let avatar_cache = shared_avatar_cache(cx);
        let now = chrono::Local::now();
        let has_channels = !self.channel_options.is_empty();

        let filter_picker = self.render_filter_channel_picker(&theme, cx);

        v_flex()
            .relative()
            .w_full()
            .self_stretch()
            .items_stretch()
            .gap_4()
            .child(
                div()
                    .pt_2()
                    .px_2()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(
                        &locale,
                        "clanIntegrationsSetting.webhooks.description",
                    )),
            )
            .when(has_channels, |el| {
                el.child(
                    h_flex()
                        .self_stretch()
                        .child(div().flex_1().min_w(px(0.0)).child(filter_picker)),
                )
            })
            .when(!has_channels, |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(
                            &locale,
                            "clanIntegrationsSetting.webhookChannelSelect.description",
                        )),
                )
            })
            .when(loading && !has_webhooks, |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .py_4()
                        .child(mezon_i18n::t(&locale, "common.loading")),
                )
            })
            .when(!has_webhooks && !loading && has_channels, |el| {
                el.child(render_webhook_create_box(
                    "new-channel-webhook-empty",
                    mezon_i18n::t(&locale, "integrations.noWebhooks").into(),
                    &theme,
                    cx.listener(|this, _, _, cx| this.create_webhook(cx)),
                ))
            })
            .when(has_webhooks, |el| {
                el.children(filtered_webhook_ids.iter().filter_map(|webhook_id| {
                    let webhook = WebhookStore::global(cx)
                        .read(cx)
                        .channel_webhooks_for_clan(self.clan_id)
                        .iter()
                        .find(|webhook| webhook.id == *webhook_id)?;
                    let id = webhook.id.clone();
                    let expanded = self.expanded_id.as_ref() == Some(&id);
                    let creator = self.creator_name(webhook.creator_id, cx);
                    let created = format_relative_time_from_seconds(
                        webhook.create_time_seconds,
                        &locale,
                        now,
                    );
                    let channel_name = self
                        .channel_list
                        .read(cx)
                        .channel_display_name(self.clan_id, webhook.channel_id)
                        .unwrap_or_else(|| "unknown".to_string());
                    let created_label =
                        mezon_i18n::t(&locale, "clanIntegrationsSetting.webhooksItem.createdBy")
                            .replace("{{webhookCreateTime}}", &created)
                            .replace("{{webhookUserOwnerName}}", &creator);
                    let edit_avatar = self
                        .edit_avatars
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| webhook.avatar.clone());
                    let avatar_uploading = self.avatar_uploading.contains(&id);

                    Some(render_channel_webhook_item(
                        webhook,
                        expanded,
                        channel_name.into(),
                        created_label.into(),
                        edit_avatar,
                        avatar_uploading,
                        self.webhook_edit_can_save(webhook, cx),
                        &self.edit_names,
                        &locale,
                        &theme,
                        avatar_cache.clone(),
                        entity.clone(),
                    ))
                }))
            })
            .when(has_channels && has_webhooks, |el| {
                el.child(render_webhook_create_box(
                    "new-channel-webhook",
                    mezon_i18n::t(&locale, "integrations.newWebhook").into(),
                    &theme,
                    cx.listener(|this, _, _, cx| this.create_webhook(cx)),
                ))
            })
    }
}
