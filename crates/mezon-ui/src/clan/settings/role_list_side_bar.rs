use std::collections::HashSet;

use gpui::{
    Context, FontWeight, ListSizingBehavior, SharedString, Window, div, prelude::*, px, size,
    uniform_list,
};

use mezon_store::{ClanRoleDetail, DEFAULT_ROLE_COLOR, RoleId, RolesStore};

use super::role_setting_page::RoleSettingPage;
use crate::components::primitives::{Icon, IconName, v_flex};
use crate::theme::{ActiveTheme, Theme};

const SIDEBAR_ITEM_HEIGHT: f32 = 36.0;

impl RoleSettingPage {
    pub(super) fn render_role_sidebar(
        &self,
        locale: &str,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let roles_store = RolesStore::global(cx);
        let roles: Vec<(RoleId, ClanRoleDetail)> = roles_store
            .read(cx)
            .active_roles_in_clan(self.clan_id)
            .into_iter()
            .map(|(id, role)| (id, role.clone()))
            .collect();
        let selected = self.selected_role_id;
        let draft_name = self.draft_name.clone();
        let draft_color = self.draft_color.clone();
        let creating_role = self.creating_role;
        let entity = cx.entity().clone();
        let locale = locale.to_string();
        let role_count = roles.len() + usize::from(creating_role);

        v_flex()
            .w(gpui::relative(1. / 3.))
            .flex_shrink_0()
            .pr_3()
            .pb(px(80.0))
            .h_full()
            .child(
                div()
                    .id("role-sidebar-back")
                    .flex()
                    .items_center()
                    .gap_1()
                    .mb_4()
                    .cursor_pointer()
                    .child(
                        div().ml(px(-10.0)).child(
                            Icon::new(IconName::ArrowLeft)
                                .size(px(16.0))
                                .text_color(theme.tokens.text_theme_primary),
                        ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(&locale, "clanRoles.roleManagement.back")),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.exit_edit_mode(cx);
                    })),
            )
            .child(
                uniform_list(
                    "role-sidebar-list",
                    role_count,
                    move |range, _window, cx| {
                        let theme = cx.theme().clone();
                        let page = entity.read(cx);
                        let roles: Vec<(RoleId, ClanRoleDetail)> = RolesStore::global(cx)
                            .read(cx)
                            .active_roles_in_clan(page.clan_id)
                            .into_iter()
                            .map(|(id, role)| (id, role.clone()))
                            .collect();
                        range
                            .map(|ix| {
                                if ix < roles.len() {
                                    let (role_id, role) = &roles[ix];
                                    let is_selected =
                                        selected == Some(*role_id) && !page.creating_role;
                                    page.render_sidebar_item(
                                        *role_id,
                                        role,
                                        is_selected,
                                        &theme,
                                        entity.clone(),
                                    )
                                    .into_any_element()
                                } else if creating_role && ix == roles.len() {
                                    page.render_sidebar_draft_item(
                                        draft_name.clone(),
                                        draft_color.clone(),
                                        &theme,
                                    )
                                    .into_any_element()
                                } else {
                                    div().h(px(SIDEBAR_ITEM_HEIGHT)).into_any_element()
                                }
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .with_item_size(size(px(0.0), px(SIDEBAR_ITEM_HEIGHT)))
                .with_sizing_behavior(ListSizingBehavior::Infer)
                .track_scroll(&self.role_sidebar_scroll)
                .flex_1()
                .min_h_0(),
            )
    }

    fn render_sidebar_item(
        &self,
        role_id: RoleId,
        role: &ClanRoleDetail,
        selected: bool,
        theme: &Theme,
        page: gpui::Entity<Self>,
    ) -> impl IntoElement {
        let color = role_color_or_default(&role.color);
        let name: SharedString = role.name.clone().into();
        let icon = if selected {
            self.draft_icon.clone()
        } else {
            role.icon.clone()
        };
        self.render_sidebar_button(
            role_id.to_string(),
            name,
            color,
            icon,
            selected,
            theme,
            Some(role_id),
            page,
        )
    }

    fn render_sidebar_draft_item(
        &self,
        name: String,
        color: String,
        theme: &Theme,
    ) -> impl IntoElement {
        let color = role_color_or_default(&color);
        div()
            .w_full()
            .py(px(6.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .bg(theme.tokens.bg_option_theme)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(role_color_dot(color, theme))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(name),
                    ),
            )
    }

    fn render_sidebar_button(
        &self,
        id: String,
        name: SharedString,
        color: SharedString,
        icon: String,
        selected: bool,
        theme: &Theme,
        role_id: Option<RoleId>,
        page: gpui::Entity<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .w_full()
            .py(px(6.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.tokens.bg_option_theme))
            .when(!selected, |el| {
                el.hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
            })
            .on_click({
                move |_, _, cx| {
                    page.update(cx, |this, cx| {
                        if this.is_dirty(cx) {
                            return;
                        }
                        if let Some(role_id) = role_id {
                            this.select_role(role_id, cx);
                        }
                    });
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(role_color_dot(color, theme))
                    .when(!icon.is_empty(), |row| {
                        row.child(role_icon_thumbnail(icon, theme))
                    })
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if selected {
                                theme.text_primary
                            } else {
                                theme.tokens.text_theme_primary
                            })
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(name),
                    ),
            )
    }
}

pub(super) fn role_color_or_default(color: &str) -> SharedString {
    if color.is_empty() {
        DEFAULT_ROLE_COLOR.into()
    } else {
        color.into()
    }
}

pub(super) fn role_color_dot(color: SharedString, theme: &Theme) -> gpui::Div {
    div()
        .flex_shrink_0()
        .size(px(12.0))
        .rounded_full()
        .bg(parse_role_color(&color).unwrap_or(theme.text_muted))
}

pub(super) fn role_icon_thumbnail(icon: String, theme: &Theme) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .size(px(20.0))
        .rounded(px(4.0))
        .overflow_hidden()
        .bg(theme.bg_tertiary)
        .child(
            gpui::img(icon)
                .size_full()
                .object_fit(gpui::ObjectFit::Cover),
        )
}

pub(super) fn parse_role_color(raw: &str) -> Option<gpui::Rgba> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hex = &trimmed[1..];
    let expanded = match hex.len() {
        3 => hex
            .chars()
            .flat_map(|c| std::iter::repeat_n(c, 2))
            .collect::<String>(),
        6 => hex.to_string(),
        _ => return None,
    };
    let value = u32::from_str_radix(&expanded, 16).ok()?;
    Some(gpui::Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: 1.0,
    })
}

pub(super) fn active_permission_ids(role: &ClanRoleDetail) -> HashSet<i64> {
    role.permissions
        .iter()
        .filter(|p| p.active)
        .map(|p| p.id)
        .collect()
}
