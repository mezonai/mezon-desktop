use std::collections::HashMap;

use gpui::{
    App, Context, Entity, FocusHandle, FontWeight, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use mezon_store::{BreakoutInvite, BreakoutRule, ClanId, UserId, VoiceStore};

use crate::Shell;
use crate::components::primitives::{Button, ButtonVariants, Radio, h_flex, v_flex};
use crate::theme::ActiveTheme;

const MAX_BREAKOUT_ROOMS: u32 = 50;

fn locale(cx: &App) -> String {
    mezon_store::Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default()
}

fn close(cx: &mut App) {
    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
}

fn room_label(locale: &str, index: usize) -> String {
    mezon_i18n::t(locale, "channelVoice.breakout.room")
        .replace("{{number}}", &(index + 1).to_string())
}

pub struct BreakoutRoomsModal {
    focus_handle: FocusHandle,
    voice: Entity<VoiceStore>,
    room_count: u32,
    rule: BreakoutRule,
    rooms_of: HashMap<UserId, u32>,
    _observe: Subscription,
}

impl BreakoutRoomsModal {
    pub fn show(voice: Entity<VoiceStore>, window: &mut Window, cx: &mut App) {
        let view = cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            _observe: cx.observe(&voice, |_, _, cx| cx.notify()),
            voice,
            room_count: 2,
            rule: BreakoutRule::Automatic,
            rooms_of: HashMap::new(),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        focus_handle.focus(window, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn participants(&self, cx: &App) -> Vec<(UserId, String)> {
        let store = self.voice.read(cx);
        let clan_id = store
            .connection()
            .connected_channel()
            .and_then(|(_, clan)| clan.parse::<i64>().ok())
            .map(ClanId);
        store
            .participants()
            .iter()
            .filter(|p| !p.is_local && !p.is_agent)
            .filter_map(|p| {
                let user_id = p.identity.parse::<UserId>().ok()?;
                let fallback = if p.metadata_name.is_empty() {
                    &p.name
                } else {
                    &p.metadata_name
                };
                let name = crate::util::voice_member::resolve_user_display(
                    cx, clan_id, user_id, fallback, "",
                )
                .name;
                Some((
                    user_id,
                    if name.is_empty() {
                        fallback.clone()
                    } else {
                        name
                    },
                ))
            })
            .collect()
    }

    fn room_of(&self, index: usize, user_id: UserId) -> u32 {
        self.rooms_of.get(&user_id).copied().unwrap_or(index as u32) % self.room_count
    }

    fn set_room_count(&mut self, room_count: u32, cx: &mut Context<Self>) {
        self.room_count = room_count.clamp(1, MAX_BREAKOUT_ROOMS);
        cx.notify();
    }

    fn step_room(&mut self, index: usize, user_id: UserId, cx: &mut Context<Self>) {
        let next = (self.room_of(index, user_id) + 1) % self.room_count;
        self.rooms_of.insert(user_id, next);
        cx.notify();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let mut assignments = vec![Vec::new(); self.room_count as usize];
        if self.rule == BreakoutRule::Manual {
            for (index, (user_id, _)) in self.participants(cx).into_iter().enumerate() {
                assignments[self.room_of(index, user_id) as usize].push(user_id.0);
            }
        }
        let (room_count, rule) = (self.room_count, self.rule);
        self.voice.update(cx, |store, cx| {
            store.open_breakout_rooms(room_count, rule, assignments, cx)
        });
        close(cx);
    }

    fn render_open(&self, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rule_row =
            |id: &'static str, rule: BreakoutRule, key: &'static str, cx: &mut Context<Self>| {
                Radio::new(id)
                    .selected(self.rule == rule)
                    .label(mezon_i18n::t(locale, key))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rule = rule;
                        cx.notify();
                    }))
            };
        let stepper_button = |id: &'static str, label: &'static str, enabled: bool| {
            Button::new(id).label(label).ghost().disabled(!enabled)
        };
        let manual_rows = (self.rule == BreakoutRule::Manual).then(|| {
            let participants = self.participants(cx);
            let rows = participants
                .into_iter()
                .enumerate()
                .map(|(index, (user_id, name))| {
                    h_flex()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(theme.tokens.text_theme_primary)
                                .child(name),
                        )
                        .child(
                            Button::new(("breakout-assign", index))
                                .label(room_label(locale, self.room_of(index, user_id) as usize))
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.step_room(index, user_id, cx)
                                })),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            if rows.is_empty() {
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelVoice.breakout.noParticipants",
                    ))
                    .into_any_element()
            } else {
                v_flex()
                    .id("breakout-assignments")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .gap_1()
                    .children(rows)
                    .into_any_element()
            }
        });
        let room_count = self.room_count;
        let pending = self.voice.read(cx).breakout_pending();

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(locale, "channelVoice.breakout.roomCount")),
                    )
                    .child(
                        stepper_button("breakout-count-down", "-", room_count > 1).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.set_room_count(room_count - 1, cx)
                            }),
                        ),
                    )
                    .child(
                        div()
                            .w(px(32.))
                            .text_center()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(room_count.to_string()),
                    )
                    .child(
                        stepper_button("breakout-count-up", "+", room_count < MAX_BREAKOUT_ROOMS)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_room_count(room_count + 1, cx)
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(rule_row(
                        "breakout-rule-auto",
                        BreakoutRule::Automatic,
                        "channelVoice.breakout.ruleAutomatic",
                        cx,
                    ))
                    .child(rule_row(
                        "breakout-rule-manual",
                        BreakoutRule::Manual,
                        "channelVoice.breakout.ruleManual",
                        cx,
                    ))
                    .child(rule_row(
                        "breakout-rule-self",
                        BreakoutRule::SelfSelect,
                        "channelVoice.breakout.ruleSelfSelect",
                        cx,
                    )),
            )
            .children(manual_rows)
            .child(
                h_flex()
                    .pt(px(8.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("breakout-cancel")
                            .label(mezon_i18n::t(locale, "common.cancel"))
                            .ghost()
                            .on_click(|_, _, cx| close(cx)),
                    )
                    .child(
                        Button::new("breakout-create")
                            .label(mezon_i18n::t(locale, "channelVoice.breakout.create"))
                            .primary()
                            .loading(pending)
                            .disabled(pending)
                            .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                    ),
            )
    }

    fn render_hosted(
        &self,
        locale: &str,
        rooms: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let pending = self.voice.read(cx).breakout_pending();
        let voice = self.voice.clone();
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(
                        mezon_i18n::t(locale, "channelVoice.breakout.openRooms")
                            .replace("{{count}}", &rooms.to_string()),
                    ),
            )
            .child(
                h_flex()
                    .pt(px(8.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("breakout-cancel")
                            .label(mezon_i18n::t(locale, "common.cancel"))
                            .ghost()
                            .on_click(|_, _, cx| close(cx)),
                    )
                    .child(
                        Button::new("breakout-close")
                            .label(mezon_i18n::t(locale, "channelVoice.breakout.close"))
                            .danger()
                            .loading(pending)
                            .disabled(pending)
                            .on_click(move |_, _, cx| {
                                voice.update(cx, |store, cx| store.close_breakout_rooms(cx));
                                close(cx);
                            }),
                    ),
            )
    }
}

impl Render for BreakoutRoomsModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = locale(cx);
        let hosted_rooms = {
            let store = self.voice.read(cx);
            store
                .connection()
                .connected_channel()
                .and_then(|(channel_id, _)| store.hosted_breakout(channel_id))
                .map(|hosted| hosted.rooms.len())
        };
        let body = match hosted_rooms {
            Some(rooms) => self.render_hosted(&locale, rooms, cx).into_any_element(),
            None => self.render_open(&locale, cx).into_any_element(),
        };
        let theme = cx.theme();
        modal_card(theme, self.focus_handle.clone())
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(&locale, "channelVoice.breakout.title")),
            )
            .child(body)
    }
}

pub struct BreakoutInviteModal {
    focus_handle: FocusHandle,
    voice: Entity<VoiceStore>,
    _observe: Subscription,
}

impl BreakoutInviteModal {
    pub fn show(voice: Entity<VoiceStore>, cx: &mut App) {
        let view = cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            _observe: cx.observe(&voice, |_: &mut Self, voice, cx| {
                if voice.read(cx).breakout_invite().is_some() {
                    cx.notify();
                    return;
                }
                let owner = cx.entity_id();
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal_if_current(owner, cx));
            }),
            voice,
        });
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn accept(&self, room_id: Option<String>, window: &mut Window, cx: &mut App) {
        let target = self.voice.update(cx, |store, cx| {
            store.accept_breakout_invite(room_id, window, cx)
        });
        open_voice_channel(target, cx);
    }
}

impl Render for BreakoutInviteModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = locale(cx);
        let invite = self.voice.read(cx).breakout_invite().cloned();
        let pending = self.voice.read(cx).breakout_pending();
        let theme = cx.theme();
        let (message_key, join_key) = match &invite {
            Some(BreakoutInvite::Assigned { .. }) => (
                "channelVoice.breakout.inviteAssigned",
                Some("channelVoice.breakout.join"),
            ),
            Some(BreakoutInvite::Choose { .. }) => ("channelVoice.breakout.inviteChoose", None),
            Some(BreakoutInvite::Return) | None => (
                "channelVoice.breakout.inviteReturn",
                Some("channelVoice.breakout.returnToMain"),
            ),
        };
        let rooms = match &invite {
            Some(BreakoutInvite::Choose { rooms }) => rooms.clone(),
            _ => Vec::new(),
        };
        let room_names: Vec<String> = rooms
            .iter()
            .enumerate()
            .map(|(index, room_id)| {
                voice_channel_name(room_id, cx).unwrap_or_else(|| room_label(&locale, index))
            })
            .collect();
        let voice = self.voice.clone();

        modal_card(theme, self.focus_handle.clone())
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(&locale, "channelVoice.breakout.title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(mezon_i18n::t(&locale, message_key)),
            )
            .when(!rooms.is_empty(), |el| {
                el.child(
                    h_flex()
                        .id("breakout-rooms")
                        .flex_wrap()
                        .gap_2()
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .children(rooms.into_iter().zip(room_names).enumerate().map(
                            |(index, (room_id, name))| {
                                Button::new(("breakout-room", index))
                                    .label(name)
                                    .disabled(pending)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.accept(Some(room_id.clone()), window, cx)
                                    }))
                            },
                        )),
                )
            })
            .child(
                h_flex()
                    .pt(px(8.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("breakout-later")
                            .label(mezon_i18n::t(&locale, "channelVoice.breakout.later"))
                            .ghost()
                            .on_click(move |_, _, cx| {
                                voice.update(cx, |store, cx| store.dismiss_breakout_invite(cx))
                            }),
                    )
                    .children(join_key.map(|key| {
                        Button::new("breakout-join")
                            .label(mezon_i18n::t(&locale, key))
                            .primary()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.accept(None, window, cx)),
                            )
                    })),
            )
    }
}

fn modal_card(theme: &crate::theme::Theme, focus_handle: FocusHandle) -> gpui::Div {
    v_flex()
        .track_focus(&focus_handle)
        .key_context("menu")
        .on_action(|_: &::menu::Cancel, _, cx| close(cx))
        .w(px(440.))
        .gap_3()
        .p(px(24.))
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg_floating)
        .shadow_lg()
}

fn voice_channel_name(channel_id: &str, cx: &App) -> Option<String> {
    let channel_id = mezon_store::ChannelId(channel_id.parse::<i64>().ok()?);
    let channels = mezon_store::ChannelList::try_global(cx)?;
    let name = channels
        .read(cx)
        .find_channel_in_active_clan(channel_id)?
        .name
        .clone();
    (!name.is_empty()).then_some(name)
}

pub(crate) fn open_voice_channel(target: Option<(String, String)>, cx: &mut App) {
    let Some((clan_id, channel_id)) = target else {
        return;
    };
    let (Ok(clan_id), Ok(channel_id)) = (clan_id.parse::<i64>(), channel_id.parse::<i64>()) else {
        return;
    };
    let (clan_id, channel_id) = (ClanId(clan_id), mezon_store::ChannelId(channel_id));
    mezon_store::ChannelList::global(cx).update(cx, |list, cx| list.select_channel(channel_id, cx));
    mezon_store::MessagesStore::global(cx).update(cx, |store, cx| {
        store.open_channel_in_clan(clan_id, channel_id, cx)
    });
    crate::router::navigate(
        cx,
        crate::router::Route::Channel {
            clan_id,
            channel_id,
        },
    );
}

pub(crate) fn can_open_breakout_rooms(store: &VoiceStore, cx: &App) -> bool {
    if store.external_meeting().is_some() || store.breakout_room_id().is_some() {
        return false;
    }
    let Some(clan_id) = store
        .connection()
        .connected_channel()
        .and_then(|(_, clan)| clan.parse::<i64>().ok())
        .filter(|clan| *clan != 0)
        .map(ClanId)
    else {
        return false;
    };
    mezon_store::PermissionStore::try_global(cx).is_some_and(|permissions| {
        permissions
            .read(cx)
            .check_permission(clan_id, mezon_store::PERMISSION_MANAGE_CLAN, cx)
    })
}

pub(crate) fn notice_text(
    locale: &str,
    notice: &mezon_store::BreakoutNotice,
) -> Option<SharedString> {
    Some(match notice {
        mezon_store::BreakoutNotice::Invited => return None,
        mezon_store::BreakoutNotice::Opened(_) => {
            mezon_i18n::t(locale, "channelVoice.breakout.opened").into()
        }
        mezon_store::BreakoutNotice::Closed => {
            mezon_i18n::t(locale, "channelVoice.breakout.closed").into()
        }
        mezon_store::BreakoutNotice::Failed(message) => message.clone().into(),
    })
}
