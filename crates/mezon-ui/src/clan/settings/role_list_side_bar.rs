use std::collections::HashSet;

use gpui::{
    App, Context, Entity, FontWeight, Hsla, ListSizingBehavior, Render, SharedString, Svg, Window,
    div, img, prelude::*, px, rgb, size, uniform_list,
};

use mezon_store::{ClanRoleDetail, DEFAULT_ROLE_COLOR, RoleId, RolesStore};

use super::role_setting_page::RoleSettingPage;
use crate::chat::role_style::role_fallback_color;
use crate::components::primitives::{Icon, IconName, v_flex};
use crate::theme::{ActiveTheme, Theme};

const SIDEBAR_ITEM_HEIGHT: f32 = 36.0;
pub(super) const DRAG_INDICATOR_COLOR: u32 = 0x22c55e;
const ROLE_LOCK_COLOR: u32 = 0xaeaeae;

#[derive(Clone)]
pub(super) struct RoleReorderDrag {
    pub(super) index: usize,
    pub(super) name: SharedString,
    pub(super) color: SharedString,
}

pub(super) struct RoleDragPreview {
    pub(super) name: SharedString,
    pub(super) color: SharedString,
}

impl Render for RoleDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .font_family(crate::theme::ui_font_family(cx))
            .py(px(6.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .bg(theme.tokens.bg_option_theme)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(role_color_dot(self.color.clone(), theme))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(self.name.clone()),
                    ),
            )
    }
}

pub(super) struct SidebarRoleItem<'a> {
    pub role_id: RoleId,
    pub role: &'a ClanRoleDetail,
    pub index: usize,
    pub selected: bool,
    pub can_manage: bool,
}

impl RoleSettingPage {
    pub(super) fn render_role_sidebar(
        &self,
        locale: &str,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let roles_store = RolesStore::global(cx);
        let visible_roles = self.sidebar_roles(roles_store.read(cx)).len();
        let selected = self.selected_role_id;
        let draft_name = self.draft_name.clone();
        let draft_color = self.draft_color.clone();
        let creating_role = self.creating_role;
        let entity = cx.entity().clone();
        let locale = locale.to_string();
        let role_count = visible_roles + usize::from(creating_role);

        let icon_cache = crate::image_cache::shared_role_icon_cache(cx);
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
                        let roles_store = RolesStore::global(cx);
                        let roles = page.sidebar_roles(roles_store.read(cx));
                        range
                            .map(|ix| {
                                if ix < roles.len() {
                                    let (role_id, role) = roles[ix];
                                    let is_selected =
                                        selected == Some(role_id) && !page.creating_role;
                                    page.render_sidebar_item(
                                        SidebarRoleItem {
                                            role_id,
                                            role,
                                            index: ix,
                                            selected: is_selected,
                                            can_manage: page.can_manage_role(role, cx),
                                        },
                                        &theme,
                                        entity.clone(),
                                        &icon_cache,
                                        cx,
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
                .smooth_line_scroll()
                .suppress_hover_while_scrolling()
                .flex_1()
                .min_h_0(),
            )
    }

    fn render_sidebar_item(
        &self,
        item: SidebarRoleItem<'_>,
        theme: &Theme,
        page: Entity<Self>,
        icon_cache: &Entity<crate::image_cache::LruImageCache>,
        cx: &App,
    ) -> impl IntoElement {
        let SidebarRoleItem {
            role_id,
            role,
            index,
            selected,
            can_manage,
        } = item;
        let color = role_color_or_default(&role.color);
        let name: SharedString = role.name.clone().into();
        let icon = if selected {
            self.draft_icon.clone()
        } else {
            role.icon.clone()
        };
        let drag_name = name.clone();
        let drag_color = color.clone();
        let drop_page = page.clone();

        div()
            .id(("role-sidebar-item", role_id.get() as u64))
            .w_full()
            .py(px(6.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.tokens.bg_option_theme))
            .when(!selected, |el| {
                el.hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
            })
            .when(can_manage, |el| {
                el.on_drag(
                    RoleReorderDrag {
                        index,
                        name: drag_name,
                        color: drag_color,
                    },
                    |drag, _, _, cx| {
                        cx.stop_propagation();
                        let name = drag.name.clone();
                        let color = drag.color.clone();
                        cx.new(|_| RoleDragPreview { name, color })
                    },
                )
            })
            .drag_over::<RoleReorderDrag>(move |style, drag, _, _| {
                if drag.index > index {
                    style.border_t_2().border_color(rgb(DRAG_INDICATOR_COLOR))
                } else {
                    style.border_b_2().border_color(rgb(DRAG_INDICATOR_COLOR))
                }
            })
            .on_drop(move |drag: &RoleReorderDrag, _, cx| {
                let from = drag.index;
                drop_page.update(cx, |this, cx| {
                    this.stage_role_reorder(from, index, cx);
                });
            })
            .on_click({
                move |_, _, cx| {
                    page.update(cx, |this, cx| {
                        if this.is_dirty(cx) {
                            return;
                        }
                        this.select_role(role_id, cx);
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
                        row.child(role_icon_thumbnail(icon, theme, icon_cache, cx))
                    })
                    .when(!can_manage, |row| row.child(role_lock_icon()))
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

    pub(super) fn sidebar_roles<'a>(
        &self,
        store: &'a RolesStore,
    ) -> Vec<(RoleId, &'a ClanRoleDetail)> {
        match self.pending_role_order.as_ref() {
            Some(order) => order
                .iter()
                .filter_map(|id| store.clan_role(self.clan_id, *id).map(|role| (*id, role)))
                .collect(),
            None => store.active_roles_in_clan(self.clan_id),
        }
    }

    pub(super) fn has_pending_role_order(&self) -> bool {
        self.pending_role_order.is_some()
    }

    pub(super) fn clear_pending_role_order(&mut self) {
        self.pending_role_order = None;
    }

    pub(super) fn stage_role_reorder(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let current = self.stored_role_order(cx);
        let staged = match self.pending_role_order.as_deref() {
            Some(order) => reconciled_role_ids(order, &current),
            None => current.clone(),
        };
        let next = reordered_role_ids(&staged, from, to);
        self.pending_role_order = (next != current).then_some(next);
        cx.notify();
    }

    pub(super) fn commit_pending_role_order(&mut self, cx: &mut Context<Self>) {
        let Some(staged) = self.pending_role_order.take() else {
            return;
        };
        let ordered = reconciled_role_ids(&staged, &self.stored_role_order(cx));
        RolesStore::global(cx).update(cx, |store, cx| {
            store.update_role_order(self.clan_id, ordered, cx);
        });
        cx.notify();
    }

    fn stored_role_order(&self, cx: &Context<Self>) -> Vec<RoleId> {
        RolesStore::global(cx)
            .read(cx)
            .active_roles_in_clan(self.clan_id)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
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

pub(super) fn role_icon_thumbnail(
    icon: String,
    theme: &Theme,
    cache: &Entity<crate::image_cache::LruImageCache>,
    cx: &App,
) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .size(px(20.0))
        .rounded(px(4.0))
        .overflow_hidden()
        .bg(theme.bg_tertiary)
        .child(
            gpui::img(crate::util::imgproxy::role_icon_url(cx, &icon))
                .size_full()
                .aspect_square()
                .object_fit(gpui::ObjectFit::Cover)
                .image_cache(cache),
        )
}

pub(super) fn role_tint(color: &str) -> Hsla {
    parse_role_color(&role_color_or_default(color))
        .map(Hsla::from)
        .unwrap_or_else(role_fallback_color)
}

pub(super) fn role_glyph(
    icon: &str,
    color: &str,
    cache: &Entity<crate::image_cache::LruImageCache>,
    cx: &App,
) -> gpui::AnyElement {
    if icon.is_empty() {
        Icon::new(IconName::RoleIcon)
            .size(px(20.0))
            .flex_shrink_0()
            .text_color(role_tint(color))
            .into_any_element()
    } else {
        img(crate::util::imgproxy::role_icon_url(cx, icon))
            .size(px(20.0))
            .aspect_square()
            .flex_shrink_0()
            .image_cache(cache)
            .into_any_element()
    }
}

pub(super) fn role_lock_icon() -> Svg {
    Icon::new(IconName::IconLock)
        .size(px(12.0))
        .flex_shrink_0()
        .text_color(rgb(ROLE_LOCK_COLOR))
}

pub(super) fn reordered_role_ids(ids: &[RoleId], from: usize, to: usize) -> Vec<RoleId> {
    let mut next = ids.to_vec();
    if from == to || from >= next.len() || to >= next.len() {
        return next;
    }
    let moved = next.remove(from);
    next.insert(to, moved);
    next
}

pub(super) fn reconciled_role_ids(staged: &[RoleId], current: &[RoleId]) -> Vec<RoleId> {
    let mut ordered: Vec<RoleId> = staged
        .iter()
        .copied()
        .filter(|id| current.contains(id))
        .collect();
    for id in current {
        if !ordered.contains(id) {
            ordered.push(*id);
        }
    }
    ordered
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

#[cfg(test)]
mod role_reorder_tests {
    use super::{reconciled_role_ids, reordered_role_ids};
    use mezon_store::RoleId;

    fn ids(values: [i64; 4]) -> Vec<RoleId> {
        values.into_iter().map(RoleId::new).collect()
    }

    #[test]
    fn moving_an_item_down_shifts_the_range_up() {
        let order = ids([1, 2, 3, 4]);
        assert_eq!(reordered_role_ids(&order, 0, 2), ids([2, 3, 1, 4]));
    }

    #[test]
    fn moving_an_item_up_shifts_the_range_down() {
        let order = ids([1, 2, 3, 4]);
        assert_eq!(reordered_role_ids(&order, 3, 1), ids([1, 4, 2, 3]));
    }

    #[test]
    fn moving_onto_itself_or_out_of_range_keeps_the_order() {
        let order = ids([1, 2, 3, 4]);
        assert_eq!(reordered_role_ids(&order, 2, 2), order);
        assert_eq!(reordered_role_ids(&order, 4, 0), order);
        assert_eq!(reordered_role_ids(&order, 0, 9), order);
    }

    #[test]
    fn reconcile_drops_removed_roles_and_appends_new_ones() {
        let staged = vec![RoleId::new(3), RoleId::new(1), RoleId::new(9)];
        let current = vec![RoleId::new(1), RoleId::new(2), RoleId::new(3)];
        assert_eq!(
            reconciled_role_ids(&staged, &current),
            vec![RoleId::new(3), RoleId::new(1), RoleId::new(2)]
        );
    }
}
