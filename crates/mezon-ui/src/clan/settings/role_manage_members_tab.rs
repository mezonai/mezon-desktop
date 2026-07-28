use std::collections::HashSet;

use gpui::{
    Context, FontWeight, ListSizingBehavior, MouseDownEvent, SharedString, Window, canvas, div,
    prelude::*, px, size, uniform_list,
};

use mezon_store::{ClanMembersStore, RoleUser, RolesStore, UserId};

use super::role_setting_page::RoleSettingPage;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Checkbox, Icon, IconName, Input, InputEvent, InputState,
    Sizable, Size, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const MEMBER_CONTROL_HEIGHT: f32 = 34.0;
const MEMBER_REMOVE_ICON_SIZE: f32 = 14.0;
const MEMBER_ROW_HEIGHT: f32 = 44.0;
const ADD_MEMBER_ROW_HEIGHT: f32 = 40.0;

#[derive(Clone)]
struct MemberDisplay {
    user_id: UserId,
    name: SharedString,
    username: SharedString,
    avatar: SharedString,
}

fn member_matches_query(member: &MemberDisplay, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    member.name.to_ascii_lowercase().contains(query)
        || member.username.to_ascii_lowercase().contains(query)
}

fn role_user_display(
    user: &RoleUser,
    clan_id: mezon_store::ClanId,
    cx: &gpui::App,
) -> MemberDisplay {
    if let Some(member) = ClanMembersStore::global(cx)
        .read(cx)
        .member(clan_id, user.id)
    {
        return MemberDisplay {
            user_id: user.id,
            name: member.name().into(),
            username: member.user.username.clone().into(),
            avatar: member.avatar().into(),
        };
    }
    let name = if !user.display_name.is_empty() {
        user.display_name.clone()
    } else {
        user.username.clone()
    };
    MemberDisplay {
        user_id: user.id,
        name: name.into(),
        username: user.username.clone().into(),
        avatar: user.avatar_url.clone().into(),
    }
}

fn clan_member_display(member: &mezon_store::ClanMember) -> MemberDisplay {
    MemberDisplay {
        user_id: member.id(),
        name: member.name().into(),
        username: member.user.username.clone().into(),
        avatar: member.avatar().into(),
    }
}

fn member_name_column(theme: &Theme, member: &MemberDisplay) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(member.name.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .line_height(px(14.0))
                .font_weight(FontWeight::LIGHT)
                .text_color(theme.text_secondary)
                .child(member.username.clone()),
        )
}

impl RoleSettingPage {
    pub(super) fn render_manage_members_tab(
        &mut self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_member_search_input(window, cx);
        if let Some(role_id) = self.selected_role_id {
            RolesStore::global(cx).update(cx, |store, cx| {
                store.ensure_role_users_loaded(role_id, cx);
            });
        }
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(self.clan_id, cx);
        });

        let query = self.member_search_query.to_ascii_lowercase();
        let members = self.filtered_role_members(query, cx);
        let can_add = can_edit && self.selected_role_id.is_some() && !self.is_saving(cx);
        let members_saving = self.is_saving(cx);
        let popover_open = self.add_members_popover_open;
        let entity = cx.entity().clone();
        let bounds_page = entity.clone();
        let locale = locale.to_string();
        let member_count = members.len();

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .h(px(MEMBER_CONTROL_HEIGHT))
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().when_some(
                        self.member_search_input.clone(),
                        |row, input| {
                            row.child(
                                div()
                                    .h(px(MEMBER_CONTROL_HEIGHT))
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.tokens.bg_tertiary)
                                    .child(Input::new(&input)),
                            )
                        },
                    ))
                    .when(can_add, |row| {
                        row.child(
                            div()
                                .relative()
                                .child(
                                    Button::new("role-add-members")
                                        .label(mezon_i18n::t(
                                            &locale,
                                            "clanRoles.setupMember.addMember",
                                        ))
                                        .primary()
                                        .with_size(Size::Medium)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.toggle_add_members_popover(window, cx);
                                        })),
                                )
                                .child(
                                    canvas(
                                        move |bounds, _, cx| {
                                            bounds_page.update(cx, |this, _| {
                                                this.add_members_button_bounds = bounds;
                                            });
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .size_full(),
                                ),
                        )
                    }),
            )
            .when(popover_open && can_add, |col| {
                col.child(
                    div()
                        .w_full()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if this.add_members_button_bounds.contains(&event.position) {
                                return;
                            }
                            this.close_add_members_popover(cx);
                        }))
                        .child(self.render_add_members_popover(&locale, theme, window, cx)),
                )
            })
            .child(if members.is_empty() {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_8()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(
                        &locale,
                        "clanRoles.setupMember.noMembersFound",
                    ))
                    .into_any_element()
            } else {
                uniform_list(
                    "role-member-list",
                    member_count,
                    move |range, _window, cx| {
                        let theme = cx.theme().clone();
                        let page = entity.read(cx);
                        let query = page.member_search_query.to_ascii_lowercase();
                        let members = page.filtered_role_members(query, cx);
                        range
                            .map(|ix| match members.get(ix) {
                                Some(member) => page
                                    .render_role_member_row(
                                        &theme,
                                        member.clone(),
                                        can_edit && !members_saving,
                                        entity.clone(),
                                    )
                                    .into_any_element(),
                                None => div().h(px(MEMBER_ROW_HEIGHT)).into_any_element(),
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .with_item_size(size(px(0.0), px(MEMBER_ROW_HEIGHT)))
                .with_sizing_behavior(ListSizingBehavior::Infer)
                .track_scroll(&self.member_list_scroll)
                .w_full()
                .into_any_element()
            })
    }

    fn render_add_members_popover(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_add_member_search_input(window, cx);
        let query = self.add_member_search_query.to_ascii_lowercase();
        let candidates = self.add_member_candidates(query, cx);
        let role_name = self.draft_name.clone();
        let selected_count = self.add_member_selection.len();
        let title = mezon_i18n::t(locale, "clanRoles.setupMember.addMember");
        let members_saving = self.is_saving(cx);
        let page = cx.entity().clone();
        let entity = page.clone();
        let candidate_count = candidates.len();

        v_flex()
            .w_full()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .shadow_lg()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(title),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::RoleIcon)
                            .size(px(20.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_secondary)
                            .child(role_name),
                    ),
            )
            .child(
                div()
                    .h(px(MEMBER_CONTROL_HEIGHT))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.bg_tertiary)
                    .when_some(self.add_member_search_input.clone(), |col, input| {
                        col.child(Input::new(&input))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(locale, "clanRoles.members").to_uppercase()),
            )
            .child(
                div()
                    .id("role-add-members-list")
                    .max_h(px(200.0))
                    .overflow_y_scroll()
                    .child(if candidate_count == 0 {
                        div()
                            .py_4()
                            .text_sm()
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(
                                locale,
                                "clanRoles.setupMember.noMembersFound",
                            ))
                            .into_any_element()
                    } else {
                        uniform_list(
                            "role-add-member-candidates",
                            candidate_count,
                            move |range, _window, cx| {
                                let theme = cx.theme().clone();
                                let page = page.read(cx);
                                let query = page.add_member_search_query.to_ascii_lowercase();
                                let candidates = page.add_member_candidates(query, cx);
                                range
                                    .map(|ix| match candidates.get(ix) {
                                        Some(member) => page
                                            .render_add_member_candidate_row(
                                                &theme,
                                                member.clone(),
                                                members_saving,
                                                entity.clone(),
                                            )
                                            .into_any_element(),
                                        None => {
                                            div().h(px(ADD_MEMBER_ROW_HEIGHT)).into_any_element()
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .with_item_size(size(px(0.0), px(ADD_MEMBER_ROW_HEIGHT)))
                        .with_sizing_behavior(ListSizingBehavior::Infer)
                        .track_scroll(&self.add_member_list_scroll)
                        .w_full()
                        .into_any_element()
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("role-add-members-cancel")
                            .label(mezon_i18n::t(locale, "common.cancel"))
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_add_members_popover(cx);
                            })),
                    )
                    .child(
                        Button::new("role-add-members-confirm")
                            .label(mezon_i18n::t(locale, "clanRoles.setupMember.add"))
                            .primary()
                            .disabled(selected_count == 0 || members_saving)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_add_members(cx);
                            })),
                    ),
            )
    }

    fn filtered_role_members(&self, query: String, cx: &gpui::App) -> Vec<MemberDisplay> {
        let Some(role_id) = self.selected_role_id else {
            return Vec::new();
        };
        RolesStore::global(cx)
            .read(cx)
            .role_users(role_id)
            .iter()
            .map(|user| role_user_display(user, self.clan_id, cx))
            .filter(|member| member_matches_query(member, &query))
            .collect()
    }

    fn add_member_candidates(&self, query: String, cx: &gpui::App) -> Vec<MemberDisplay> {
        let Some(role_id) = self.selected_role_id else {
            return Vec::new();
        };
        let assigned: HashSet<UserId> = RolesStore::global(cx)
            .read(cx)
            .role_users(role_id)
            .iter()
            .map(|user| user.id)
            .collect();
        ClanMembersStore::global(cx)
            .read(cx)
            .members(self.clan_id)
            .into_iter()
            .filter(|member| !assigned.contains(&member.id()))
            .map(clan_member_display)
            .filter(|member| member_matches_query(member, &query))
            .collect()
    }

    fn render_role_member_row(
        &self,
        theme: &Theme,
        member: MemberDisplay,
        can_edit: bool,
        page: gpui::Entity<Self>,
    ) -> impl IntoElement {
        let user_id = member.user_id;
        let row_group = SharedString::from(format!("role-member-row-{}", user_id.get()));
        let mut avatar = Avatar::new().name(member.name.clone()).size_px(px(24.0));
        if !member.avatar.is_empty() {
            avatar = avatar.src(member.avatar.clone());
        }

        h_flex()
            .id(("role-member-row", user_id.get() as u64))
            .group(row_group.clone())
            .w_full()
            .w_full()
            .p_2()
            .rounded(px(6.0))
            .items_center()
            .justify_between()
            .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .ml_2()
                    .items_center()
                    .child(avatar)
                    .child(member_name_column(theme, &member)),
            )
            .when(can_edit, |row| {
                row.child(
                    div()
                        .id(("role-remove-member", user_id.get() as u64))
                        .flex_shrink_0()
                        .mr_2()
                        .p(px(5.0))
                        .cursor_pointer()
                        .rounded_full()
                        .hover(|s| s.bg(theme.bg_hover))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(MEMBER_REMOVE_ICON_SIZE))
                                .text_color(theme.text_secondary),
                        )
                        .on_click(move |_, _, cx| {
                            page.update(cx, |this, cx| {
                                this.remove_role_member(user_id, cx);
                            });
                        }),
                )
            })
    }

    fn render_add_member_candidate_row(
        &self,
        theme: &Theme,
        member: MemberDisplay,
        disabled: bool,
        page: gpui::Entity<Self>,
    ) -> impl IntoElement {
        let user_id = member.user_id;
        let checked = self.add_member_selection.contains(&user_id);
        let mut avatar = Avatar::new().name(member.name.clone()).size_px(px(20.0));
        if !member.avatar.is_empty() {
            avatar = avatar.src(member.avatar.clone());
        }

        div()
            .id(("role-add-member-candidate", user_id.get() as u64))
            .w_full()
            .w_full()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .when(!disabled, |el| el.cursor_pointer())
            .when(disabled, |el| el.opacity(0.5))
            .when(!disabled, |el| el.hover(|s| s.bg(theme.bg_hover)))
            .when(!disabled, |el| {
                el.on_click({
                    let page = page.clone();
                    move |_, _, cx| {
                        page.update(cx, |this, cx| {
                            this.toggle_add_member_selection(user_id, cx);
                        });
                    }
                })
            })
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(avatar)
                            .child(member_name_column(theme, &member)),
                    )
                    .child(
                        Checkbox::new(("role-add-member-check", user_id.get() as u64))
                            .checked(checked)
                            .disabled(disabled),
                    ),
            )
    }

    pub(super) fn toggle_add_members_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.add_members_popover_open {
            self.close_add_members_popover(cx);
            return;
        }
        self.add_members_popover_open = true;
        self.add_member_selection.clear();
        self.add_member_search_query.clear();
        if let Some(input) = &self.add_member_search_input {
            input.update(cx, |state, cx| {
                state.set_value("", window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn close_add_members_popover(&mut self, cx: &mut Context<Self>) {
        self.add_members_popover_open = false;
        self.add_member_selection.clear();
        self.add_member_search_query.clear();
        cx.notify();
    }

    fn toggle_add_member_selection(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        if self.add_member_selection.contains(&user_id) {
            self.add_member_selection.remove(&user_id);
        } else {
            self.add_member_selection.insert(user_id);
        }
        cx.notify();
    }

    pub(super) fn remove_role_member(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        let Some(role_id) = self.selected_role_id else {
            return;
        };
        if self.is_saving(cx) {
            return;
        }
        RolesStore::global(cx).update(cx, |store, cx| {
            store.mutate_role_members(self.clan_id, role_id, Vec::new(), vec![user_id.get()], cx);
        });
    }

    pub(super) fn confirm_add_members(&mut self, cx: &mut Context<Self>) {
        let Some(role_id) = self.selected_role_id else {
            return;
        };
        if self.is_saving(cx) {
            return;
        }
        let user_ids: Vec<i64> = self
            .add_member_selection
            .iter()
            .map(|id| id.get())
            .collect();
        if user_ids.is_empty() {
            return;
        }
        let accepted = RolesStore::global(cx).update(cx, |store, cx| {
            store.mutate_role_members(self.clan_id, role_id, user_ids, Vec::new(), cx)
        });
        if accepted {
            self.close_add_members_popover(cx);
        }
    }

    pub(super) fn ensure_member_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.member_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clanRoles.setupMember.searchMembers").into();
        let input_bg = cx.theme().tokens.bg_tertiary;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(MEMBER_CONTROL_HEIGHT))
                .text_size(px(15.0))
                .borderless()
                .bg(input_bg)
        });
        let sub = cx.subscribe(&input, |this, input_entity, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.member_search_query = input_entity.read(cx).value().to_string();
                cx.notify();
            }
        });
        self.member_search_input = Some(input);
        self._member_search_sub = Some(sub);
    }

    fn ensure_add_member_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_member_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clanRoles.setupMember.searchMembers").into();
        let input_bg = cx.theme().tokens.bg_tertiary;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(MEMBER_CONTROL_HEIGHT))
                .text_size(px(15.0))
                .borderless()
                .bg(input_bg)
        });
        let sub = cx.subscribe(&input, |this, input_entity, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.add_member_search_query = input_entity.read(cx).value().to_string();
                cx.notify();
            }
        });
        self.add_member_search_input = Some(input);
        self._add_member_search_sub = Some(sub);
    }
}
