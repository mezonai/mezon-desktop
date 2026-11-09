use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    App, ClickEvent, ClipboardItem, Context, Entity, FontWeight, MouseDownEvent, Subscription,
    Task, Window, deferred, div, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, ClanList, STREAM_QUALITIES, Settings,
};

use crate::components::primitives::{
    Icon, IconName, Label, Sizable, Size, Spinner, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const OBS_STEP_KEYS: [&str; 5] = [
    "setting.streaming.obsStepService",
    "setting.streaming.obsStepPaste",
    "setting.streaming.obsStepEncoder",
    "setting.streaming.obsStepSimulcast",
    "setting.streaming.obsStepStart",
];

#[derive(Clone)]
struct StreamChannelOption {
    id: ChannelId,
    name: String,
    clan_id: ClanId,
    clan_name: String,
}

#[derive(Clone)]
struct StreamKeyData {
    whip_url: String,
    token: String,
    expires_at: i64,
}

#[derive(Clone)]
struct KeyResult {
    channel_id: ChannelId,
    key: Option<StreamKeyData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyTarget {
    Server,
    Token,
}

pub struct StreamingPage {
    settings: Entity<Settings>,
    channels: Vec<StreamChannelOption>,
    active_clan_id: Option<ClanId>,
    picked_channel_id: Option<ChannelId>,
    dropdown_open: bool,
    dropdown_closed_at: Option<Instant>,
    result: Option<KeyResult>,
    loading_channel_id: Option<ChannelId>,
    revealed: bool,
    request_counter: u64,
    copied_target: Option<CopyTarget>,
    _subs: Vec<Subscription>,
    _key_task: Option<Task<()>>,
    _countdown_task: Option<Task<()>>,
    _copy_reset_task: Option<Task<()>>,
}

impl StreamingPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let mut subs = Vec::new();
        subs.push(cx.observe(&settings, |_, _, cx| cx.notify()));
        if let Some(channel_list) = ChannelList::try_global(cx) {
            subs.push(cx.observe(&channel_list, |this, _, cx| {
                this.refresh_channels(cx);
                cx.notify();
            }));
        }
        if let Some(clan_list) = ClanList::try_global(cx) {
            subs.push(cx.observe(&clan_list, |this, _, cx| {
                this.refresh_channels(cx);
                cx.notify();
            }));
        }
        Self::request_channel_loads(cx);
        let (channels, active_clan_id) = Self::collect_stream_channels(cx);
        Self {
            settings,
            channels,
            active_clan_id,
            picked_channel_id: None,
            dropdown_open: false,
            dropdown_closed_at: None,
            result: None,
            loading_channel_id: None,
            revealed: false,
            request_counter: 0,
            copied_target: None,
            _subs: subs,
            _key_task: None,
            _countdown_task: None,
            _copy_reset_task: None,
        }
    }

    pub fn ensure_channels_loaded(&mut self, cx: &mut Context<Self>) {
        Self::request_channel_loads(cx);
    }

    fn request_channel_loads(cx: &mut App) {
        if let Some(channel_list) = ChannelList::try_global(cx) {
            let clan_ids: Vec<ClanId> = ClanList::try_global(cx)
                .map(|cl| {
                    cl.read(cx)
                        .clans
                        .iter()
                        .map(|c| c.id)
                        .filter(|id| !id.is_zero())
                        .collect()
                })
                .unwrap_or_default();
            channel_list.update(cx, |cl, cx| {
                cl.ensure_user_channels_loaded(cx);
                for clan_id in clan_ids {
                    cl.load_structure_for_clan(clan_id, cx);
                }
            });
        }
    }

    fn refresh_channels(&mut self, cx: &App) {
        let prev_selected = self.selected_channel().map(|c| c.id);
        let (channels, active_clan_id) = Self::collect_stream_channels(cx);
        self.channels = channels;
        self.active_clan_id = active_clan_id;
        let next_selected = self.selected_channel().map(|c| c.id);
        if prev_selected != next_selected {
            self.clear_key_state();
        }
    }

    fn selected_channel(&self) -> Option<&StreamChannelOption> {
        Self::resolve_selected(&self.channels, self.picked_channel_id, self.active_clan_id)
    }

    fn clear_key_state(&mut self) {
        self.request_counter = self.request_counter.wrapping_add(1);
        self.result = None;
        self.loading_channel_id = None;
        self._key_task = None;
        self._countdown_task = None;
    }

    fn collect_stream_channels(cx: &App) -> (Vec<StreamChannelOption>, Option<ClanId>) {
        let (clan_names, active_clan_id) = ClanList::try_global(cx)
            .map(|cl| {
                let cl = cl.read(cx);
                let names: Vec<(ClanId, String)> =
                    cl.clans.iter().map(|c| (c.id, c.name.clone())).collect();
                (names, cl.active_clan_id)
            })
            .unwrap_or_default();

        let Some(channel_list) = ChannelList::try_global(cx) else {
            return (Vec::new(), active_clan_id);
        };
        let channels = channel_list
            .read(cx)
            .user_channels()
            .filter(|ch| ch.channel_type == ChannelType::Stream)
            .map(|ch| {
                let clan_name = clan_names
                    .iter()
                    .find(|(id, _)| *id == ch.clan_id)
                    .map(|(_, name)| name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| ch.clan_name.clone());
                StreamChannelOption {
                    id: ch.id,
                    name: ch.name.clone(),
                    clan_id: ch.clan_id,
                    clan_name,
                }
            })
            .collect();
        (channels, active_clan_id)
    }

    fn resolve_selected(
        channels: &[StreamChannelOption],
        picked_id: Option<ChannelId>,
        active_clan_id: Option<ClanId>,
    ) -> Option<&StreamChannelOption> {
        picked_id
            .and_then(|id| channels.iter().find(|c| c.id == id))
            .or_else(|| active_clan_id.and_then(|cid| channels.iter().find(|c| c.clan_id == cid)))
            .or_else(|| channels.first())
    }

    fn select_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        let current_selected = self.selected_channel().map(|c| c.id);
        self.picked_channel_id = Some(channel_id);
        self.dropdown_open = false;
        self.revealed = false;
        if current_selected != Some(channel_id) {
            self.clear_key_state();
        }
        cx.notify();
    }

    fn fetch_key(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if channel_id.is_zero() || self.loading_channel_id == Some(channel_id) {
            return;
        }
        self.request_counter = self.request_counter.wrapping_add(1);
        let req_id = self.request_counter;
        self.picked_channel_id = Some(channel_id);
        self.loading_channel_id = Some(channel_id);
        self.revealed = false;
        self._countdown_task = None;
        cx.notify();

        let Some(api) = crate::chat::channel_settings::channel_acl::api(cx) else {
            self.loading_channel_id = None;
            self.result = Some(KeyResult {
                channel_id,
                key: None,
            });
            cx.notify();
            return;
        };

        self._key_task = Some(cx.spawn(async move |this, cx| {
            let resp = api.get_stream_playback(channel_id.get(), true).await;
            this.update(cx, |this, cx| {
                if this.request_counter != req_id {
                    return;
                }
                this.loading_channel_id = None;
                let key = match resp {
                    Ok(pb) if !pb.token.is_empty() && !pb.whip_url.is_empty() => {
                        Some(StreamKeyData {
                            whip_url: pb.whip_url,
                            token: pb.token,
                            expires_at: pb.expires_at,
                        })
                    }
                    _ => None,
                };
                let expires_at = key.as_ref().map(|k| k.expires_at);
                this.result = Some(KeyResult { channel_id, key });
                if let Some(exp) = expires_at {
                    this.start_countdown(channel_id, exp, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn start_countdown(&mut self, channel_id: ChannelId, expires_at: i64, cx: &mut Context<Self>) {
        let expires_at_ms = expires_at.saturating_mul(1000);
        self._countdown_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let active = this
                            .result
                            .as_ref()
                            .and_then(|r| (r.channel_id == channel_id).then_some(r.key.as_ref()))
                            .flatten()
                            .is_some_and(|k| k.expires_at == expires_at);
                        if !active {
                            return false;
                        }
                        cx.notify();
                        now_unix_ms() < expires_at_ms
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn copy_text(&mut self, text: String, target: CopyTarget, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied_target = Some(target);
        cx.notify();
        self._copy_reset_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                if this.copied_target == Some(target) {
                    this.copied_target = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn render_channel_picker(
        channels: &[StreamChannelOption],
        selected: Option<&StreamChannelOption>,
        is_open: bool,
        locale: &str,
        theme: &Theme,
        entity: Entity<StreamingPage>,
    ) -> impl IntoElement {
        let Some(selected) = selected else {
            return div()
                .h(px(40.0))
                .w_full()
                .flex()
                .items_center()
                .px_3()
                .rounded_md()
                .bg(theme.tokens.theme_setting_primary)
                .border_1()
                .border_color(theme.border)
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(locale, "setting.streaming.noChannels"))
                .into_any_element();
        };

        let selected_id = selected.id;
        let selected_name = selected.name.clone();
        let selected_clan = selected.clan_name.clone();

        div()
            .relative()
            .child(
                div()
                    .id("stream-channel-picker")
                    .h(px(40.0))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .rounded_md()
                    .bg(theme.tokens.theme_setting_primary)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Stream)
                                    .size(px(16.0))
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .child(selected_name),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(selected_clan),
                            ),
                    )
                    .child(div().text_color(theme.text_muted).child(if is_open {
                        "▲"
                    } else {
                        "▼"
                    }))
                    .on_click({
                        let entity = entity.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            entity.update(cx, |this, cx| {
                                let just_closed = this
                                    .dropdown_closed_at
                                    .is_some_and(|at| at.elapsed() < Duration::from_millis(400));
                                this.dropdown_open = !this.dropdown_open && !just_closed;
                                this.dropdown_closed_at = None;
                                cx.notify();
                            });
                        }
                    }),
            )
            .when(is_open && !channels.is_empty(), |el| {
                let options = channels.to_vec();
                let close_entity = entity.clone();
                el.child(deferred(
                    div()
                        .id("stream-channel-dropdown")
                        .absolute()
                        .top(px(44.0))
                        .left_0()
                        .right_0()
                        .max_h(px(240.0))
                        .overflow_y_scroll()
                        .rounded_md()
                        .bg(theme.tokens.theme_setting_primary)
                        .border_1()
                        .border_color(theme.border)
                        .occlude()
                        .on_mouse_down_out(
                            move |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                                close_entity.update(cx, |this, cx| {
                                    if this.dropdown_open {
                                        this.dropdown_open = false;
                                        this.dropdown_closed_at = Some(Instant::now());
                                        cx.notify();
                                    }
                                });
                            },
                        )
                        .children(options.into_iter().map(move |opt| {
                            let is_selected = opt.id == selected_id;
                            let channel_id = opt.id;
                            let entity = entity.clone();
                            div()
                                .id(("stream-channel-opt", channel_id.get() as u64))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .cursor_pointer()
                                .when(is_selected, |el| el.bg(theme.bg_hover))
                                .when(!is_selected, |el| el.hover(|s| s.bg(theme.bg_hover)))
                                .child(
                                    Icon::new(IconName::Stream)
                                        .size(px(16.0))
                                        .text_color(theme.text_primary),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .text_color(theme.text_primary)
                                        .child(opt.name),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .pl_4()
                                        .text_xs()
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(opt.clan_name),
                                )
                                .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                    entity.update(cx, |this, cx| {
                                        this.select_channel(channel_id, cx);
                                    });
                                })
                        })),
                ))
            })
            .into_any_element()
    }

    fn render_copy_button(
        id: &'static str,
        locale: &str,
        theme: &Theme,
        copied: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .child(
                Icon::new(if copied {
                    IconName::Check
                } else {
                    IconName::CopyIcon
                })
                .size(px(16.0))
                .text_color(if copied {
                    theme.status_online
                } else {
                    theme.text_primary
                }),
            )
            .child(
                Label::new(mezon_i18n::t(locale, "setting.streaming.copy"))
                    .text_sm()
                    .text_color(theme.text_primary),
            )
            .on_click(on_click)
    }
}

impl Render for StreamingPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let default_quality = self.settings.read(cx).stream_default_quality;
        let entity = cx.entity();

        let selected = self.selected_channel();
        let selected_channel_id = selected.map(|c| c.id);

        let shown_result = selected_channel_id
            .and_then(|cid| self.result.as_ref().filter(|r| r.channel_id == cid));
        let stream_key = shown_result.and_then(|r| r.key.as_ref()).cloned();
        let is_error = shown_result.is_some_and(|r| r.key.is_none());
        let loading =
            selected_channel_id.is_some() && self.loading_channel_id == selected_channel_id;
        let revealed = self.revealed;
        let server_copied = self.copied_target == Some(CopyTarget::Server);
        let token_copied = self.copied_target == Some(CopyTarget::Token);

        let button_disabled = selected_channel_id.is_none() || loading;
        let button_label = mezon_i18n::t(
            &locale,
            if stream_key.is_some() {
                "setting.streaming.regenerate"
            } else {
                "setting.streaming.getKey"
            },
        );

        let mut key_section = v_flex()
            .w_full()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.streaming.keyTitle"))
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.streaming.keyDescription"))
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Label::new(
                            mezon_i18n::t(&locale, "setting.streaming.channel").to_uppercase(),
                        )
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_secondary),
                    )
                    .child(Self::render_channel_picker(
                        &self.channels,
                        selected,
                        self.dropdown_open,
                        &locale,
                        &theme,
                        entity.clone(),
                    )),
            );

        if let Some(key) = &stream_key {
            let whip_url = key.whip_url.clone();
            let whip_for_copy = key.whip_url.clone();
            let token_for_copy = key.token.clone();
            let displayed_token = if revealed {
                key.token.clone()
            } else {
                "•".repeat(32)
            };
            let remaining_ms = key.expires_at.saturating_mul(1000) - now_unix_ms();
            let expiry_el = if remaining_ms > 0 {
                let text = mezon_i18n::t(&locale, "setting.streaming.expiresIn")
                    .replace("{{time}}", &format_remaining_ms(remaining_ms));
                Label::new(text)
                    .text_xs()
                    .text_color(theme.tokens.text_theme_primary)
                    .into_any_element()
            } else {
                Label::new(mezon_i18n::t(&locale, "setting.streaming.expired"))
                    .text_xs()
                    .text_color(theme.danger_text)
                    .into_any_element()
            };

            key_section = key_section.child(
                v_flex()
                    .gap_4()
                    .rounded_md()
                    .bg(theme.tokens.theme_setting_primary)
                    .border_1()
                    .border_color(theme.border)
                    .p_4()
                    .child(
                        h_flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Label::new(
                                            mezon_i18n::t(&locale, "setting.streaming.server")
                                                .to_uppercase(),
                                        )
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text_secondary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_primary)
                                            .whitespace_normal()
                                            .child(whip_url),
                                    ),
                            )
                            .child(Self::render_copy_button(
                                "stream-key-copy-server",
                                &locale,
                                &theme,
                                server_copied,
                                {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        let text = whip_for_copy.clone();
                                        entity.update(cx, |this, cx| {
                                            this.copy_text(text, CopyTarget::Server, cx);
                                        });
                                    }
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Label::new(
                                            mezon_i18n::t(&locale, "setting.streaming.token")
                                                .to_uppercase(),
                                        )
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text_secondary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_primary)
                                            .whitespace_normal()
                                            .child(displayed_token),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("stream-key-reveal")
                                            .px_3()
                                            .py_1()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_secondary)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover))
                                            .child(
                                                Label::new(mezon_i18n::t(
                                                    &locale,
                                                    if revealed {
                                                        "setting.streaming.hide"
                                                    } else {
                                                        "setting.streaming.show"
                                                    },
                                                ))
                                                .text_sm()
                                                .text_color(theme.text_primary),
                                            )
                                            .on_click({
                                                let entity = entity.clone();
                                                move |_, _, cx| {
                                                    entity.update(cx, |this, cx| {
                                                        this.revealed = !this.revealed;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    )
                                    .child(Self::render_copy_button(
                                        "stream-key-copy-token",
                                        &locale,
                                        &theme,
                                        token_copied,
                                        {
                                            let entity = entity.clone();
                                            move |_, _, cx| {
                                                let text = token_for_copy.clone();
                                                entity.update(cx, |this, cx| {
                                                    this.copy_text(text, CopyTarget::Token, cx);
                                                });
                                            }
                                        },
                                    )),
                            ),
                    )
                    .child(expiry_el),
            );
        }

        if is_error {
            key_section = key_section.child(
                Label::new(mezon_i18n::t(&locale, "setting.streaming.error"))
                    .text_xs()
                    .text_color(theme.danger_text),
            );
        }

        let mut get_key_btn = div()
            .id("stream-get-key-btn")
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .rounded_lg()
            .bg(theme.brand)
            .when(button_disabled, |el| el.opacity(0.5))
            .when(!button_disabled, |el| el.cursor_pointer())
            .when(loading, |el| {
                el.child(Spinner::new().with_size(Size::Small).color(gpui::white()))
            })
            .child(
                Label::new(button_label)
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(gpui::white()),
            );

        if let Some(cid) = selected_channel_id
            && !loading
        {
            let entity = entity.clone();
            get_key_btn = get_key_btn.on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| {
                    this.fetch_key(cid, cx);
                });
            });
        }

        key_section = key_section.child(h_flex().child(get_key_btn));

        let obs_section = v_flex()
            .w_full()
            .gap_3()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_6()
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.streaming.obsTitle"))
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary),
            )
            .child(
                v_flex()
                    .gap_2()
                    .children(OBS_STEP_KEYS.iter().enumerate().map(|(idx, key)| {
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_sm()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(format!("{}.", idx + 1)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .whitespace_normal()
                                    .child(mezon_i18n::t(&locale, key)),
                            )
                    })),
            );

        let quality_section = v_flex()
            .w_full()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.streaming.qualityTitle"))
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.streaming.qualityDescription",
                        ))
                        .text_xs()
                        .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .children(STREAM_QUALITIES.iter().enumerate().map(|(idx, &quality)| {
                        let is_selected = default_quality == quality;
                        let label_key = quality.i18n_key();
                        h_flex()
                            .id(("stream-default-quality", idx))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(if is_selected {
                                theme.brand
                            } else {
                                theme.border
                            })
                            .when(is_selected, |el| el.bg(theme.bg_hover))
                            .when(!is_selected, |el| el.hover(|s| s.bg(theme.bg_hover)))
                            .cursor_pointer()
                            .child(
                                Label::new(mezon_i18n::t(&locale, label_key))
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                div()
                                    .size(px(16.0))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if is_selected {
                                        theme.brand
                                    } else {
                                        theme.text_muted
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(is_selected, |el| {
                                        el.child(div().size(px(8.0)).rounded_full().bg(theme.brand))
                                    }),
                            )
                            .on_click(move |_, _, cx| {
                                mezon_store::set_stream_default_quality(quality, cx);
                            })
                    })),
            );

        v_flex()
            .w_full()
            .gap_6()
            .child(key_section)
            .child(obs_section)
            .child(quality_section)
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn format_remaining_ms(remaining_ms: i64) -> String {
    if remaining_ms <= 0 {
        return "0:00".to_string();
    }
    let seconds = (remaining_ms + 999) / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
