use gpui::{App, Context, FontWeight, Window, div, prelude::*, px};

use mezon_store::{ClanRoleDetail, RolesStore, everyone_slug};

use super::role_color_picker;
use super::role_icon_picker;
use super::role_setting_page::RoleSettingPage;
use crate::components::primitives::{Input, InputEvent, InputState, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};

fn render_section_divider(theme: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .border_t_1()
        .border_color(theme.border)
        .h(px(16.0))
}

fn render_required_section_label(
    locale: &str,
    label_key: &'static str,
    theme: &Theme,
    show_inline_error: bool,
    error_key: Option<&'static str>,
) -> impl IntoElement {
    let label = mezon_i18n::t(locale, label_key).to_string().to_uppercase();
    h_flex()
        .gap(px(2.0))
        .mb_2()
        .items_center()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(label),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.status_dnd)
                .child("*"),
        )
        .when(show_inline_error, |row| {
            if let Some(key) = error_key {
                row.child(
                    div()
                        .pl_2()
                        .text_xs()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.status_dnd)
                        .child(mezon_i18n::t(locale, key).to_string()),
                )
            } else {
                row
            }
        })
}

fn render_section_label(locale: &str, label_key: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text_primary)
        .mb_2()
        .child(mezon_i18n::t(locale, label_key).to_string().to_uppercase())
}

fn render_helper_text(locale: &str, key: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(theme.text_secondary)
        .mb_2()
        .child(mezon_i18n::t(locale, key).to_string())
}

impl RoleSettingPage {
    pub(super) fn render_display_tab(
        &mut self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_name_input(window, cx);
        let can_edit_display = self.can_edit_display(cx);
        let name_input = self.name_input.clone();
        let name_missing = self.draft_name.trim().is_empty();
        let role_name = self.draft_name.clone();

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .w_full()
                    .pr_5()
                    .child(render_required_section_label(
                        locale,
                        "clanRoles.roleManagement.roleName",
                        theme,
                        name_missing && can_edit_display,
                        Some("clanRoles.roleManagement.roleNameIsRequired"),
                    ))
                    .child(if can_edit_display {
                        div()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_input_secondary)
                            .when(!can_edit, |el| el.opacity(0.6))
                            .when_some(name_input, |el, input| el.child(Input::new(&input)))
                            .into_any_element()
                    } else {
                        div()
                            .h(px(40.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_input_secondary)
                            .opacity(0.6)
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(role_name)
                            .into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .w_full()
                    .pr_5()
                    .child(render_section_divider(theme))
                    .child(render_required_section_label(
                        locale,
                        "clanRoles.roleManagement.roleColour",
                        theme,
                        false,
                        None,
                    ))
                    .child(render_helper_text(
                        locale,
                        "clanRoles.roleManagement.membersUseColour",
                        theme,
                    ))
                    .child(role_color_picker::render_role_color_section(
                        self,
                        locale,
                        theme,
                        can_edit_display,
                        window,
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .w_full()
                    .pr_5()
                    .child(render_section_divider(theme))
                    .child(render_section_label(
                        locale,
                        "clanRoles.roleManagement.roleIcon",
                        theme,
                    ))
                    .child(render_helper_text(
                        locale,
                        "clanRoles.roleManagement.roleIconDescription",
                        theme,
                    ))
                    .child(role_icon_picker::render_role_icon_section(
                        self,
                        locale,
                        theme,
                        can_edit_display,
                        window,
                        cx,
                    )),
            )
    }

    pub(super) fn ensure_name_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.name_input.is_some() {
            return;
        }
        let input_bg = cx.theme().tokens.bg_input_secondary;
        let initial = self.draft_name.clone();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .height(px(40.0))
                .text_size(px(15.0))
                .borderless()
                .bg(input_bg)
        });
        name_input.update(cx, |input, cx| {
            input.set_value(&initial, window, cx);
        });
        let sub = cx.subscribe(&name_input, |this, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.sync_name_from_input(cx);
            }
        });
        self.name_input = Some(name_input);
        self._name_sub = Some(sub);
    }

    pub(super) fn sync_name_from_input(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = &self.name_input {
            self.draft_name = input.read(cx).value().to_string();
            cx.notify();
        }
    }

    pub(super) fn sync_name_input_from_draft(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(input) = &self.name_input {
            input.update(cx, |state, cx| {
                state.set_value(&self.draft_name, window, cx);
            });
        }
        if self.custom_color_picker_open {
            self.sync_picker_from_draft_color();
            self.sync_rgb_inputs_from_picker(window, cx);
        }
    }

    pub(super) fn is_everyone_role(&self, cx: &App) -> bool {
        let Some(role_id) = self.selected_role_id else {
            return false;
        };
        let roles = RolesStore::global(cx).read(cx);
        roles
            .clan_role(self.clan_id, role_id)
            .is_some_and(|role| role.slug == everyone_slug(self.clan_id))
    }

    pub(super) fn selected_role_detail<'a>(&self, cx: &'a App) -> Option<&'a ClanRoleDetail> {
        let role_id = self.selected_role_id?;
        RolesStore::global(cx)
            .read(cx)
            .clan_role(self.clan_id, role_id)
    }
}
