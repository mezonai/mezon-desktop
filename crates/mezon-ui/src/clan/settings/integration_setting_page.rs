use std::path::PathBuf;

use gpui::{
    AsyncApp, ClipboardItem, Context, Entity, FontWeight, Pixels, Render, SharedString,
    Subscription, Window, div, prelude::*, px,
};

use mezon_store::{
    ChannelList, ClanId, ClanImageMimeType, MAX_WEBHOOK_AVATAR_BYTES, Settings, WebhookStore,
};

use super::channel_webhook_tab::ChannelWebhookTab;
use super::clan_webhook_tab::ClanWebhookTab;
use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, TabBar, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};

const WEBHOOK_NAMES: [&str; 3] = ["Captain hook", "Spidey bot", "Komu Knight"];
const WEBHOOK_AVATAR_PATHS: [&str; 3] = [
    "/images/webhook-avatar-1.png",
    "/images/webhook-avatar-2.png",
    "/images/webhook-avatar-3.png",
];

const CLAN_SETTINGS_TITLE_AREA_PX: f32 = 112.0;
const CLAN_SETTINGS_CONTENT_BOTTOM_PADDING_PX: f32 = 28.0;
const INTEGRATION_PAGE_MIN_HEIGHT_PX: f32 = 240.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IntegrationTab {
    ChannelWebhooks,
    ClanWebhooks,
}

pub fn random_webhook_name() -> String {
    let idx = (rand_webhook_seed() as usize) % WEBHOOK_NAMES.len();
    WEBHOOK_NAMES[idx].to_string()
}

pub fn random_webhook_avatar(base_img_url: &str) -> String {
    let base = base_img_url.trim_end_matches('/');
    let idx = (rand_webhook_seed() as usize) % WEBHOOK_AVATAR_PATHS.len();
    format!("{base}{}", WEBHOOK_AVATAR_PATHS[idx])
}

fn rand_webhook_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        % 3
}

pub fn render_webhook_url_field(
    id: impl Into<SharedString>,
    url: String,
    locale: &str,
    theme: &Theme,
) -> impl IntoElement {
    let id = id.into();
    let locale = locale.to_string();
    let copy_id = SharedString::from(format!("{id}-copy"));
    h_flex()
        .id(id)
        .w_full()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.tokens.bg_tertiary)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_xs()
                .text_color(theme.text_muted)
                .child(url.clone()),
        )
        .child(
            div()
                .id(copy_id)
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.0))
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(
                    Icon::new(IconName::CopyIcon)
                        .size(px(16.0))
                        .text_color(theme.text_secondary),
                )
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.success(
                            mezon_i18n::t(&locale, "clanIntegrationsSetting.webhooksEdit.copied"),
                            cx,
                        );
                    });
                }),
        )
}

pub fn render_webhook_create_box(
    id: impl Into<SharedString>,
    label: SharedString,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    render_webhook_dashed_box(id, label, theme, true, Some(on_click))
}

pub fn render_webhook_empty_box(
    id: impl Into<SharedString>,
    label: SharedString,
    theme: &Theme,
) -> impl IntoElement {
    render_webhook_dashed_box(
        id,
        label,
        theme,
        false,
        None::<fn(&gpui::ClickEvent, &mut Window, &mut gpui::App)>,
    )
}

fn render_webhook_dashed_box(
    id: impl Into<SharedString>,
    label: SharedString,
    theme: &Theme,
    interactive: bool,
    on_click: Option<impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static>,
) -> impl IntoElement {
    v_flex().self_stretch().child({
        let mut box_el = h_flex()
            .id(id.into())
            .justify_center()
            .items_center()
            .py_6()
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(theme.tokens.border_primary)
            .opacity(0.5)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        Icon::new(IconName::WebhooksIcon)
                            .size(px(20.0))
                            .text_color(theme.tokens.text_theme_primary),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(label),
                    ),
            );

        if interactive {
            box_el = box_el
                .cursor_pointer()
                .hover(|s| s.opacity(1.0).bg(theme.bg_hover));
            if let Some(on_click) = on_click {
                box_el = box_el.on_click(on_click);
            }
        }

        box_el
    })
}

pub async fn upload_webhook_avatar(
    path: PathBuf,
    locale: &str,
    cx: &mut AsyncApp,
) -> Result<Option<String>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ClanImageMimeType::is_allowed_extension(&ext) {
        return Err(mezon_i18n::t(locale, "clanSettings.clanLogo.modal.content").to_string());
    }
    let path_buf = path.clone();
    let file_size = match cx
        .background_spawn(async move { std::fs::metadata(&path_buf).ok().map(|m| m.len()) })
        .await
    {
        Some(size) => size,
        None => return Ok(None),
    };
    if file_size > MAX_WEBHOOK_AVATAR_BYTES {
        return Err(mezon_i18n::t(locale, "clanSoundSetting.toast.errorSizeLimit").to_string());
    }
    let task = cx.update(|cx| {
        WebhookStore::global(cx).update(cx, |store, cx| store.upload_webhook_avatar(&path, cx))
    });
    task.await.map(Some).map_err(|err| {
        tracing::error!("webhook avatar upload failed: {err}");
        mezon_i18n::t(locale, "streamThumbnail.errors.uploadFailed").to_string()
    })
}

pub struct IntegrationSettingPage {
    clan_id: ClanId,
    settings: Entity<Settings>,
    channel_list: Entity<ChannelList>,
    can_manage_clan_webhooks: bool,
    active_tab: IntegrationTab,
    channel_tab: Option<Entity<ChannelWebhookTab>>,
    clan_tab: Option<Entity<ClanWebhookTab>>,
    _subs: Vec<Subscription>,
}

impl IntegrationSettingPage {
    pub fn new(
        clan_id: ClanId,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        can_manage_clan_webhooks: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel_list_for_sub = channel_list.clone();
        let mut subs = Vec::new();
        subs.push(cx.observe(&settings, |_, _, cx| cx.notify()));
        subs.push(cx.observe(&channel_list_for_sub, |this, _, cx| {
            this.ensure_active_tab(cx);
            cx.notify();
        }));

        let mut this = Self {
            clan_id,
            settings,
            channel_list,
            can_manage_clan_webhooks,
            active_tab: IntegrationTab::ChannelWebhooks,
            channel_tab: None,
            clan_tab: None,
            _subs: subs,
        };
        this.ensure_active_tab(cx);
        this
    }

    pub fn release(&mut self) {
        self.channel_tab = None;
        self.clan_tab = None;
    }

    fn ensure_active_tab(&mut self, cx: &mut Context<Self>) {
        match self.active_tab {
            IntegrationTab::ChannelWebhooks => {
                if self.channel_tab.is_none() {
                    let tab = cx.new(|cx| {
                        ChannelWebhookTab::new(
                            self.clan_id,
                            self.channel_list.clone(),
                            self.settings.clone(),
                            cx,
                        )
                    });
                    self._subs.push(cx.observe(&tab, |_, _, cx| cx.notify()));
                    self.channel_tab = Some(tab);
                }
            }
            IntegrationTab::ClanWebhooks => {
                if self.clan_tab.is_none() {
                    let tab = cx.new(|cx| {
                        ClanWebhookTab::new(
                            self.clan_id,
                            self.settings.clone(),
                            self.can_manage_clan_webhooks,
                            cx,
                        )
                    });
                    self._subs.push(cx.observe(&tab, |_, _, cx| cx.notify()));
                    self.clan_tab = Some(tab);
                }
            }
        }
    }

    fn set_tab(&mut self, tab: IntegrationTab, cx: &mut Context<Self>) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        self.ensure_active_tab(cx);
        cx.notify();
    }
}

fn calc_page_height(window: &Window) -> Pixels {
    let viewport_h = f32::from(window.viewport_size().height);
    let height = viewport_h - CLAN_SETTINGS_TITLE_AREA_PX - CLAN_SETTINGS_CONTENT_BOTTOM_PADDING_PX;
    px(height.max(INTEGRATION_PAGE_MIN_HEIGHT_PX))
}

impl Render for IntegrationSettingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let entity = cx.entity();
        let active = self.active_tab;

        let tabs = vec![
            mezon_i18n::t(
                &locale,
                "clanIntegrationsSetting.integration.channelWebhooks",
            )
            .into(),
            mezon_i18n::t(&locale, "clanIntegrationsSetting.integration.clanWebhooks").into(),
        ];
        let selected = match active {
            IntegrationTab::ChannelWebhooks => 0,
            IntegrationTab::ClanWebhooks => 1,
        };

        let mut content_panel = div().w_full().flex().flex_col().items_stretch();
        match active {
            IntegrationTab::ChannelWebhooks => {
                if let Some(tab) = &self.channel_tab {
                    content_panel = content_panel.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_stretch()
                            .child(tab.clone()),
                    );
                }
            }
            IntegrationTab::ClanWebhooks => {
                if let Some(tab) = &self.clan_tab {
                    content_panel = content_panel.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_stretch()
                            .child(tab.clone()),
                    );
                }
            }
        }

        v_flex()
            .h(calc_page_height(window))
            .min_h_0()
            .w_full()
            .items_stretch()
            .child(
                div()
                    .flex_shrink_0()
                    .w_full()
                    .bg(theme.tokens.theme_setting_primary)
                    .child(TabBar::new(tabs).selected(selected).on_select(
                        move |index, _window, cx| {
                            let tab = if index == 0 {
                                IntegrationTab::ChannelWebhooks
                            } else {
                                IntegrationTab::ClanWebhooks
                            };
                            entity.update(cx, |this, cx| this.set_tab(tab, cx));
                        },
                    )),
            )
            .child(
                div()
                    .id("integration-tab-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content_panel),
            )
    }
}
