use std::collections::HashSet;

use gpui::{
    App, Context, Entity, FocusHandle, FontWeight, ListSizingBehavior, Pixels, SharedString,
    Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, size, uniform_list,
};

use mezon_store::{
    ClanId, ClanRoleDetail, DEFAULT_ROLE_COLOR, PermissionStore, RoleDraft, RoleId, RolesEvent,
    RolesStore, Settings, UserId, everyone_slug,
};

use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

use super::role_color_picker;
use super::role_icon_picker::RoleIconPickerTab;
use super::role_list_side_bar::{self, active_permission_ids as role_active_permission_ids};
use crate::app::shell::Shell;
use crate::chat::message::ReactionPicker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RolePageMode {
    List,
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoleEditTab {
    Display,
    Permissions,
    ManageMembers,
}

pub struct RoleSettingPage {
    pub(super) clan_id: ClanId,
    pub(super) settings: Entity<Settings>,
    pub(super) mode: RolePageMode,
    pub(super) selected_role_id: Option<RoleId>,
    pub(super) creating_role: bool,
    pub(super) edit_tab: RoleEditTab,
    list_search_query: String,
    pub(super) permission_search_query: String,
    pub(super) member_search_query: String,
    pub(super) add_member_search_query: String,
    pub(super) add_members_popover_open: bool,
    pub(super) add_members_button_bounds: gpui::Bounds<Pixels>,
    pub(super) add_member_selection: HashSet<UserId>,
    pub(super) draft_name: String,
    pub(super) draft_color: String,
    pub(super) draft_icon: String,
    saved_name: String,
    saved_color: String,
    saved_icon: String,
    pub(super) draft_permission_ids: HashSet<i64>,
    saved_permission_ids: HashSet<i64>,
    list_search_input: Option<Entity<InputState>>,
    pub(super) permission_search_input: Option<Entity<InputState>>,
    pub(super) member_search_input: Option<Entity<InputState>>,
    pub(super) add_member_search_input: Option<Entity<InputState>>,
    pub(super) name_input: Option<Entity<InputState>>,
    pub(super) custom_color_picker_open: bool,
    pub(super) custom_color_anchor_bounds: gpui::Bounds<Pixels>,
    pub(super) sv_field_bounds: gpui::Bounds<Pixels>,
    pub(super) hue_slider_bounds: gpui::Bounds<Pixels>,
    pub(super) picker_hue: f32,
    pub(super) picker_sat: f32,
    pub(super) picker_val: f32,
    pub(super) rgb_r_input: Option<Entity<InputState>>,
    pub(super) rgb_g_input: Option<Entity<InputState>>,
    pub(super) rgb_b_input: Option<Entity<InputState>>,
    pub(super) icon_uploading: bool,
    pub(super) role_icon_picker_open: bool,
    pub(super) role_icon_picker_tab: RoleIconPickerTab,
    pub(super) role_icon_picker_focus: FocusHandle,
    pub(super) role_icon_emoji_picker: Option<Entity<ReactionPicker>>,
    pub(super) role_sidebar_scroll: UniformListScrollHandle,
    pub(super) role_list_scroll: UniformListScrollHandle,
    pub(super) member_list_scroll: UniformListScrollHandle,
    pub(super) add_member_list_scroll: UniformListScrollHandle,
    pub(super) _role_icon_emoji_picker_sub: Option<Subscription>,
    _list_search_sub: Option<Subscription>,
    pub(super) _permission_search_sub: Option<Subscription>,
    pub(super) _member_search_sub: Option<Subscription>,
    pub(super) _add_member_search_sub: Option<Subscription>,
    pub(super) _name_sub: Option<Subscription>,
    pub(super) _rgb_input_subs: Vec<Subscription>,
    pub(super) _icon_upload_task: Option<Task<()>>,
    _roles_sub: Subscription,
    _permissions_sub: Subscription,
}

impl RoleSettingPage {
    pub fn new(clan_id: ClanId, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        RolesStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });
        PermissionStore::global(cx).update(cx, |store, cx| {
            store.ensure_catalog_loaded(cx);
            store.load_clan_permissions(clan_id, cx);
        });

        let roles_sub = cx.subscribe(
            &RolesStore::global(cx),
            |this, _, event: &RolesEvent, cx| {
                match event {
                    RolesEvent::Created { clan_id, role_id } if *clan_id == this.clan_id => {
                        if let Some(role) = RolesStore::global(cx)
                            .read(cx)
                            .clan_role(*clan_id, *role_id)
                            .cloned()
                        {
                            this.mode = RolePageMode::Edit;
                            this.selected_role_id = Some(*role_id);
                            this.creating_role = false;
                            this.edit_tab = RoleEditTab::Display;
                            this.load_draft_from_role(&role);
                            this.reset_name_input();
                        }
                    }
                    RolesEvent::RoleSaved { clan_id, role_id }
                        if *clan_id == this.clan_id && this.selected_role_id == Some(*role_id) =>
                    {
                        this.commit_saved_baseline();
                    }
                    RolesEvent::RoleSaveFailed { clan_id, role_id }
                        if *clan_id == this.clan_id && this.selected_role_id == Some(*role_id) =>
                    {
                        let locale = this.settings.read(cx).language.clone();
                        let message = mezon_i18n::t(&locale, "clanOverviewSetting.toast.saveError")
                            .to_string();
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                    }
                    _ => {}
                }
                cx.notify();
            },
        );
        let permissions_sub = cx.subscribe(&PermissionStore::global(cx), |_, _, _, cx| cx.notify());

        Self {
            clan_id,
            settings,
            mode: RolePageMode::List,
            selected_role_id: None,
            creating_role: false,
            edit_tab: RoleEditTab::Display,
            list_search_query: String::new(),
            permission_search_query: String::new(),
            member_search_query: String::new(),
            add_member_search_query: String::new(),
            add_members_popover_open: false,
            add_members_button_bounds: gpui::Bounds::default(),
            add_member_selection: HashSet::new(),
            draft_name: String::new(),
            draft_color: DEFAULT_ROLE_COLOR.to_string(),
            draft_icon: String::new(),
            saved_name: String::new(),
            saved_color: DEFAULT_ROLE_COLOR.to_string(),
            saved_icon: String::new(),
            draft_permission_ids: HashSet::new(),
            saved_permission_ids: HashSet::new(),
            list_search_input: None,
            permission_search_input: None,
            member_search_input: None,
            add_member_search_input: None,
            name_input: None,
            custom_color_picker_open: false,
            custom_color_anchor_bounds: gpui::Bounds::default(),
            sv_field_bounds: gpui::Bounds::default(),
            hue_slider_bounds: gpui::Bounds::default(),
            picker_hue: 0.0,
            picker_sat: 1.0,
            picker_val: 1.0,
            rgb_r_input: None,
            rgb_g_input: None,
            rgb_b_input: None,
            icon_uploading: false,
            role_icon_picker_open: false,
            role_icon_picker_tab: RoleIconPickerTab::Image,
            role_icon_picker_focus: cx.focus_handle(),
            role_icon_emoji_picker: None,
            role_sidebar_scroll: UniformListScrollHandle::new(),
            role_list_scroll: UniformListScrollHandle::new(),
            member_list_scroll: UniformListScrollHandle::new(),
            add_member_list_scroll: UniformListScrollHandle::new(),
            _role_icon_emoji_picker_sub: None,
            _list_search_sub: None,
            _permission_search_sub: None,
            _member_search_sub: None,
            _add_member_search_sub: None,
            _name_sub: None,
            _rgb_input_subs: Vec::new(),
            _icon_upload_task: None,
            _roles_sub: roles_sub,
            _permissions_sub: permissions_sub,
        }
    }

    pub fn role_icon_picker_focus(&self) -> FocusHandle {
        self.role_icon_picker_focus.clone()
    }

    fn reset_name_input(&mut self) {
        self.name_input = None;
        self._name_sub = None;
    }

    fn commit_saved_baseline(&mut self) {
        self.saved_name = self.draft_name.trim().to_string();
        self.saved_color = self.draft_color.clone();
        self.saved_icon = self.draft_icon.clone();
        self.saved_permission_ids = self.draft_permission_ids.clone();
    }

    pub fn release(&mut self) {
        self._icon_upload_task.take();
        self.icon_uploading = false;
        self.role_icon_picker_open = false;
        self.role_icon_emoji_picker = None;
        self._role_icon_emoji_picker_sub = None;
        self._list_search_sub.take();
        self._permission_search_sub.take();
        self._member_search_sub.take();
        self._add_member_search_sub.take();
        self._name_sub.take();
        self._rgb_input_subs.clear();
        self.list_search_input = None;
        self.permission_search_input = None;
        self.member_search_input = None;
        self.add_member_search_input = None;
        self.name_input = None;
        self.rgb_r_input = None;
        self.rgb_g_input = None;
        self.rgb_b_input = None;
        self.custom_color_picker_open = false;
        self.add_members_popover_open = false;
        self.add_member_selection.clear();
    }

    pub fn is_in_edit_mode(&self) -> bool {
        self.mode == RolePageMode::Edit
    }

    pub fn should_show_save_bar(&self, cx: &App) -> bool {
        self.mode == RolePageMode::Edit && self.is_dirty(cx) && !self.is_saving(cx)
    }

    pub fn is_role_icon_picker_open(&self) -> bool {
        self.role_icon_picker_open
    }

    pub fn is_saving(&self, cx: &App) -> bool {
        RolesStore::global(cx).read(cx).is_saving(self.clan_id)
    }

    pub(super) fn is_dirty(&self, cx: &App) -> bool {
        if self.is_everyone_role(cx) {
            return self.draft_permission_ids != self.saved_permission_ids;
        }
        self.draft_name.trim() != self.saved_name.trim()
            || self.draft_color != self.saved_color
            || self.draft_icon != self.saved_icon
            || self.draft_permission_ids != self.saved_permission_ids
    }

    pub(super) fn can_edit_display(&self, cx: &App) -> bool {
        self.can_edit_role(cx) && !self.is_everyone_role(cx)
    }

    fn can_edit_role(&self, cx: &App) -> bool {
        let perms = PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(self.clan_id, cx);
        if !perms.has_manage_clan {
            return false;
        }
        if perms.is_clan_owner {
            return true;
        }
        let Some(role) = self.selected_role_detail(cx) else {
            return true;
        };
        if RolesStore::global(cx).read(cx).role_has_administrator(role) {
            return false;
        }
        PermissionStore::global(cx)
            .read(cx)
            .current_permission_level(self.clan_id, cx)
            .is_none_or(|level| role.max_level_permission < level)
    }

    fn can_manage_role(&self, role: &ClanRoleDetail, cx: &App) -> bool {
        let perms = PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(self.clan_id, cx);
        if !perms.has_manage_clan {
            return false;
        }
        if perms.is_clan_owner {
            return true;
        }
        if RolesStore::global(cx).read(cx).role_has_administrator(role) {
            return false;
        }
        PermissionStore::global(cx)
            .read(cx)
            .current_permission_level(self.clan_id, cx)
            .is_none_or(|level| role.max_level_permission < level)
    }

    fn filtered_roles(&self, cx: &App) -> Vec<(RoleId, ClanRoleDetail)> {
        let query = self.list_search_query.to_ascii_lowercase();
        RolesStore::global(cx)
            .read(cx)
            .active_roles_in_clan(self.clan_id)
            .into_iter()
            .filter(|(_, role)| query.is_empty() || role.name.to_ascii_lowercase().contains(&query))
            .map(|(id, role)| (id, role.clone()))
            .collect()
    }

    pub(super) fn select_role(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        let Some(role) = RolesStore::global(cx)
            .read(cx)
            .clan_role(self.clan_id, role_id)
            .cloned()
        else {
            return;
        };
        self.mode = RolePageMode::Edit;
        self.selected_role_id = Some(role_id);
        self.creating_role = false;
        self.edit_tab = RoleEditTab::Display;
        self.load_draft_from_role(&role);
        self.reset_name_input();
        RolesStore::global(cx).update(cx, |store, cx| {
            store.ensure_role_users_loaded(role_id, cx);
        });
        cx.notify();
    }

    fn enter_edit_mode(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        self.select_role(role_id, cx);
    }

    pub(super) fn exit_edit_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = RolePageMode::List;
        self.selected_role_id = None;
        self.creating_role = false;
        self.custom_color_picker_open = false;
        self.clear_draft();
        cx.notify();
    }

    fn clear_draft(&mut self) {
        self.draft_name.clear();
        self.draft_color = DEFAULT_ROLE_COLOR.to_string();
        self.draft_icon.clear();
        self.saved_name.clear();
        self.saved_color = DEFAULT_ROLE_COLOR.to_string();
        self.saved_icon.clear();
        self.draft_permission_ids.clear();
        self.saved_permission_ids.clear();
        self.custom_color_picker_open = false;
        self.add_members_popover_open = false;
        self.add_member_selection.clear();
        self.role_icon_picker_open = false;
        self.role_icon_emoji_picker = None;
        self._role_icon_emoji_picker_sub = None;
        self.member_search_query.clear();
        self.add_member_search_query.clear();
    }

    fn load_draft_from_role(&mut self, role: &ClanRoleDetail) {
        self.draft_name = role.name.clone();
        self.draft_color = if role.color.is_empty() {
            DEFAULT_ROLE_COLOR.to_string()
        } else {
            role.color.clone()
        };
        self.draft_icon = role.icon.clone();
        self.saved_name = self.draft_name.clone();
        self.saved_color = self.draft_color.clone();
        self.saved_icon = self.draft_icon.clone();
        self.draft_permission_ids = role_active_permission_ids(role);
        self.saved_permission_ids = self.draft_permission_ids.clone();
    }

    pub(super) fn set_draft_color(&mut self, color: String, cx: &mut Context<Self>) {
        self.draft_color = color;
        cx.notify();
    }

    pub(super) fn open_custom_color_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.custom_color_picker_open = true;
        self.sync_picker_from_draft_color();
        self.ensure_rgb_inputs(window, cx);
        cx.notify();
    }

    pub(super) fn close_custom_color_picker(&mut self, cx: &mut Context<Self>) {
        self.custom_color_picker_open = false;
        cx.notify();
    }

    pub(super) fn sync_picker_from_draft_color(&mut self) {
        if let Some((r, g, b)) = role_color_picker::hex_to_rgb(&self.draft_color) {
            let (h, s, v) = role_color_picker::rgb_to_hsv(r, g, b);
            self.picker_hue = h;
            self.picker_sat = s;
            self.picker_val = v;
        }
    }

    pub(super) fn apply_picker_to_draft_color(&mut self, cx: &mut Context<Self>) {
        self.draft_color =
            role_color_picker::hsv_to_hex(self.picker_hue, self.picker_sat, self.picker_val);
        cx.notify();
    }

    pub(super) fn set_permission_active(
        &mut self,
        permission_id: i64,
        active: bool,
        cx: &mut Context<Self>,
    ) {
        if active {
            self.draft_permission_ids.insert(permission_id);
        } else {
            self.draft_permission_ids.remove(&permission_id);
        }
        cx.notify();
    }

    fn start_create_role(&mut self, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let default_name = mezon_i18n::t(&locale, "clanRoles.createNewRole.newRole").to_string();
        let roles = RolesStore::global(cx);
        let max_permission_id = roles.read(cx).resolve_max_permission_id(self.clan_id, cx);
        let draft = RoleDraft {
            title: default_name.clone(),
            color: DEFAULT_ROLE_COLOR.to_string(),
            icon: String::new(),
            add_permission_ids: Vec::new(),
            remove_permission_ids: Vec::new(),
            add_user_ids: Vec::new(),
            remove_user_ids: Vec::new(),
        };
        self.creating_role = true;
        self.mode = RolePageMode::Edit;
        self.selected_role_id = None;
        self.edit_tab = RoleEditTab::Display;
        self.draft_name = default_name;
        self.draft_color = DEFAULT_ROLE_COLOR.to_string();
        self.draft_icon.clear();
        self.draft_permission_ids.clear();
        self.reset_name_input();
        roles.update(cx, |store, cx| {
            store.create_role(self.clan_id, draft, max_permission_id, cx);
        });
        cx.notify();
    }

    fn open_everyone_role(&mut self, cx: &mut Context<Self>) {
        if let Some(role_id) = RolesStore::global(cx)
            .read(cx)
            .everyone_role_id(self.clan_id)
        {
            self.enter_edit_mode(role_id, cx);
        }
    }

    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft_name = self.saved_name.clone();
        self.draft_color = self.saved_color.clone();
        self.draft_icon = self.saved_icon.clone();
        self.draft_permission_ids = self.saved_permission_ids.clone();
        self.sync_name_input_from_draft(window, cx);
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.draft_name.trim().is_empty() {
            return;
        }
        let add_permission_ids: Vec<i64> = self
            .draft_permission_ids
            .difference(&self.saved_permission_ids)
            .copied()
            .collect();
        let remove_permission_ids: Vec<i64> = self
            .saved_permission_ids
            .difference(&self.draft_permission_ids)
            .copied()
            .collect();
        let is_everyone = self.is_everyone_role(cx);
        let draft = RoleDraft {
            title: if is_everyone {
                self.saved_name.trim().to_string()
            } else {
                self.draft_name.trim().to_string()
            },
            color: if is_everyone {
                self.saved_color.clone()
            } else {
                self.draft_color.clone()
            },
            icon: if is_everyone {
                self.saved_icon.clone()
            } else {
                self.draft_icon.clone()
            },
            add_permission_ids,
            remove_permission_ids,
            add_user_ids: Vec::new(),
            remove_user_ids: Vec::new(),
        };
        let roles = RolesStore::global(cx);
        let max_permission_id = roles.read(cx).resolve_max_permission_id(self.clan_id, cx);
        if self.creating_role {
            roles.update(cx, |store, cx| {
                store.create_role(self.clan_id, draft, max_permission_id, cx);
            });
        } else if let Some(role_id) = self.selected_role_id {
            roles.update(cx, |store, cx| {
                store.update_role(self.clan_id, role_id, draft, max_permission_id, cx);
            });
        }
        cx.notify();
    }

    pub fn request_delete_role(
        &self,
        role_id: RoleId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.settings.read(cx).language.clone();
        crate::app::shell::Shell::global(cx).update(cx, |shell, cx| {
            shell.confirm_delete_role(self.clan_id, role_id, &locale, window, cx);
        });
    }

    fn ensure_list_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.list_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clanRoles.mainRoles.searchRoles").into();
        let input_bg = cx.theme().tokens.bg_tertiary;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(36.0))
                .text_size(px(15.0))
                .borderless()
                .bg(input_bg)
        });
        let sub = cx.subscribe(&input, |this, input_entity, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.list_search_query = input_entity.read(cx).value().to_string();
                cx.notify();
            }
        });
        self.list_search_input = Some(input);
        self._list_search_sub = Some(sub);
    }

    fn render_list_mode(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_list_search_input(window, cx);
        let roles = self.filtered_roles(cx);
        let role_count = roles.len();
        let can_manage = PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(self.clan_id, cx)
            .has_manage_clan;

        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(locale, "clanRoles.mainRoles.useRolesToGroup")),
            )
            .child(
                div()
                    .id("clan-roles-everyone")
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_hover)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_secondary))
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                locale,
                                                "clanRoles.mainRoles.defaultPermissions",
                                            )),
                                    )
                                    .child(div().text_xs().text_color(theme.text_secondary).child(
                                        mezon_i18n::t(
                                            locale,
                                            "clanRoles.mainRoles.everyoneDescription",
                                        ),
                                    )),
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(18.0))
                                    .text_color(theme.text_secondary),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_everyone_role(cx);
                    })),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .when_some(self.list_search_input.clone(), |row, input| {
                        row.child(
                            div()
                                .flex_1()
                                .rounded_lg()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.tokens.bg_tertiary)
                                .child(Input::new(&input)),
                        )
                    })
                    .when(can_manage, |row| {
                        row.child(
                            Button::new("clan-roles-create")
                                .label(mezon_i18n::t(locale, "clanRoles.mainRoles.createRole"))
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_create_role(cx);
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(locale, "clanRoles.mainRoles.membersUseColor")),
            )
            .child(self.render_roles_table(locale, theme, &roles, role_count, cx))
    }

    fn render_roles_table(
        &self,
        locale: &str,
        theme: &Theme,
        roles: &[(RoleId, ClanRoleDetail)],
        role_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        const ROW_HEIGHT: f32 = 56.0;
        let roles_header = format!(
            "{} - {}",
            mezon_i18n::t(locale, "clanRoles.roles"),
            role_count
        );
        let members_header = mezon_i18n::t(locale, "clanRoles.members");
        let entity = cx.entity().clone();
        let locale = locale.to_string();

        v_flex()
            .w_full()
            .overflow_hidden()
            .child(
                h_flex()
                    .h(px(ROW_HEIGHT))
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .w(gpui::relative(0.5))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(roles_header.to_uppercase()),
                    )
                    .child(
                        div()
                            .w(gpui::relative(0.25))
                            .text_center()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(members_header.to_uppercase()),
                    )
                    .child(div().w(gpui::relative(0.25))),
            )
            .child(if roles.is_empty() {
                div()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_base()
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(&locale, "clanRoles.mainRoles.noRoles"))
                    .into_any_element()
            } else {
                uniform_list(
                    "clan-roles-table",
                    roles.len(),
                    move |range, _window, cx| {
                        let theme = cx.theme().clone();
                        let page = entity.read(cx);
                        let roles = page.filtered_roles(cx);
                        range
                            .map(|ix| match roles.get(ix) {
                                Some((role_id, role)) => page
                                    .render_list_role_row(
                                        &locale,
                                        &theme,
                                        *role_id,
                                        role,
                                        page.can_manage_role(role, cx),
                                        entity.clone(),
                                    )
                                    .into_any_element(),
                                None => div().h(px(ROW_HEIGHT)).into_any_element(),
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .with_item_size(size(px(0.0), px(ROW_HEIGHT)))
                .with_sizing_behavior(ListSizingBehavior::Infer)
                .track_scroll(&self.role_list_scroll)
                .w_full()
                .into_any_element()
            })
    }

    fn render_list_role_row(
        &self,
        locale: &str,
        theme: &Theme,
        role_id: RoleId,
        role: &ClanRoleDetail,
        can_manage: bool,
        page: Entity<Self>,
    ) -> impl IntoElement {
        let is_everyone = role.slug == everyone_slug(self.clan_id);
        let row_group = SharedString::from(format!("clan-role-row-{}", role_id.get()));
        div()
            .id(("clan-role-row", role_id.get() as u64))
            .group(row_group.clone())
            .w_full()
            .h(px(56.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
            .child(
                h_flex()
                    .w(gpui::relative(0.5))
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .px_2()
                    .child(role_list_side_bar::role_color_dot(
                        role_list_side_bar::role_color_or_default(&role.color),
                        theme,
                    ))
                    .when(!role.icon.is_empty(), |row| {
                        row.child(role_list_side_bar::role_icon_thumbnail(
                            role.icon.clone(),
                            theme,
                        ))
                    })
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(role.name.clone()),
                    ),
            )
            .child(
                div()
                    .w(gpui::relative(0.25))
                    .text_center()
                    .text_base()
                    .text_color(theme.text_primary)
                    .child(if is_everyone {
                        mezon_i18n::t(locale, "clanRoles.allMembers").to_string()
                    } else {
                        role.member_count.to_string()
                    }),
            )
            .child(
                h_flex()
                    .w(gpui::relative(0.25))
                    .h_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .when(can_manage, |actions| {
                        let make_edit_button = |page: Entity<Self>| {
                            div()
                                .id(("clan-role-edit", role_id.get() as u64))
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(36.0))
                                .rounded_full()
                                .cursor_pointer()
                                .bg(theme.status_dnd)
                                .opacity(0.)
                                .group_hover(row_group.clone(), |s| s.opacity(1.))
                                .child(
                                    Icon::new(IconName::PenEdit)
                                        .size(px(20.))
                                        .text_color(theme.text_primary),
                                )
                                .on_click(move |_, _, cx| {
                                    page.update(cx, |this, cx| {
                                        this.enter_edit_mode(role_id, cx);
                                    });
                                })
                        };

                        actions.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(if is_everyone {
                                    div().size(px(36.0)).into_any_element()
                                } else {
                                    make_edit_button(page.clone()).into_any_element()
                                })
                                .child(if is_everyone {
                                    make_edit_button(page.clone()).into_any_element()
                                } else {
                                    div()
                                        .id(("clan-role-delete", role_id.get() as u64))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(36.0))
                                        .rounded_full()
                                        .cursor_pointer()
                                        .bg(theme.status_dnd)
                                        .child(
                                            Icon::new(IconName::DeleteMessageRightClick)
                                                .size(px(20.))
                                                .text_color(theme.text_primary),
                                        )
                                        .on_click({
                                            let delete_page = page.clone();
                                            move |_, window, cx| {
                                                delete_page.update(cx, |this, cx| {
                                                    this.request_delete_role(role_id, window, cx);
                                                });
                                            }
                                        })
                                        .into_any_element()
                                }),
                        )
                    }),
            )
    }

    fn render_edit_mode(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_edit = self.can_edit_role(cx);
        let is_everyone = self.is_everyone_role(cx);
        let is_clan_owner = PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(self.clan_id, cx)
            .is_clan_owner;
        let header = if self.creating_role {
            mezon_i18n::t(locale, "clanRoles.roleManagement.newRole").to_string()
        } else {
            format!(
                "{} - {}",
                mezon_i18n::t(locale, "clanRoles.roleManagement.editRole"),
                self.draft_name
            )
        };

        h_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .items_start()
            .child(self.render_role_sidebar(locale, theme, window, cx))
            .child(div().w(px(1.0)).self_stretch().bg(theme.border))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .overflow_hidden()
                    .pl_3()
                    .pr_5()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .mb_4()
                            .child(header.to_uppercase()),
                    )
                    .child(self.render_edit_tabs(locale, theme, is_everyone, cx))
                    .child(
                        div()
                            .id("role-edit-tab-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .pb(px(80.0))
                            .child(match self.edit_tab {
                                RoleEditTab::Display => self
                                    .render_display_tab(locale, theme, can_edit, window, cx)
                                    .into_any_element(),
                                RoleEditTab::Permissions => self
                                    .render_permissions_tab(
                                        locale,
                                        theme,
                                        can_edit,
                                        is_everyone,
                                        is_clan_owner,
                                        window,
                                        cx,
                                    )
                                    .into_any_element(),
                                RoleEditTab::ManageMembers => self
                                    .render_manage_members_tab(locale, theme, can_edit, window, cx)
                                    .into_any_element(),
                            }),
                    ),
            )
    }

    fn render_edit_tabs(
        &self,
        locale: &str,
        theme: &Theme,
        is_everyone: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let member_count = self
            .selected_role_id
            .map(|role_id| {
                let loaded = RolesStore::global(cx).read(cx).role_users(role_id).len();
                if loaded > 0 {
                    loaded
                } else {
                    self.selected_role_detail(cx)
                        .map(|role| role.member_count)
                        .unwrap_or(0)
                }
            })
            .unwrap_or(0);
        let manage_label = format!(
            "{} ({member_count})",
            mezon_i18n::t(locale, "clanRoles.roleManagement.manageMembers")
        );

        let mut tabs = h_flex()
            .flex_shrink_0()
            .w_full()
            .mb_3()
            .border_b_1()
            .border_color(theme.border);

        tabs = tabs.child(self.render_edit_tab_button(
            RoleEditTab::Display,
            "role-tab-display",
            mezon_i18n::t(locale, "clanRoles.roleManagement.display").to_string(),
            theme,
            cx,
        ));

        tabs = tabs.child(self.render_edit_tab_button(
            RoleEditTab::Permissions,
            "role-tab-permissions",
            mezon_i18n::t(locale, "clanRoles.roleManagement.permissions").to_string(),
            theme,
            cx,
        ));

        if !is_everyone {
            tabs = tabs.child(self.render_edit_tab_button(
                RoleEditTab::ManageMembers,
                "role-tab-manage-members",
                manage_label,
                theme,
                cx,
            ));
        }

        tabs
    }

    fn render_edit_tab_button(
        &self,
        tab: RoleEditTab,
        id: &'static str,
        label: String,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.edit_tab == tab;
        div()
            .id(id)
            .relative()
            .flex_1()
            .flex()
            .justify_center()
            .items_center()
            .py(px(5.0))
            .cursor_pointer()
            .text_base()
            .font_weight(FontWeight::MEDIUM)
            .text_color(if active {
                theme.text_primary
            } else {
                theme.tokens.text_theme_primary
            })
            .child(label)
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(2.0))
                    .when(active, |el| el.bg(gpui::rgb(0x60a5fa))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.edit_tab != tab {
                    this.edit_tab = tab;
                    cx.notify();
                }
            }))
    }
}

impl Render for RoleSettingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        div()
            .when(self.mode == RolePageMode::Edit, |el| {
                el.w_full().h_full().min_h_0()
            })
            .child(match self.mode {
                RolePageMode::List => self
                    .render_list_mode(&locale, &theme, window, cx)
                    .into_any_element(),
                RolePageMode::Edit => self
                    .render_edit_mode(&locale, &theme, window, cx)
                    .into_any_element(),
            })
    }
}

pub fn render_role_save_bar(
    page: Entity<RoleSettingPage>,
    locale: &str,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let saving = page.read(cx).is_saving(cx);
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
                                .text_color(theme.text_secondary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "clanSettings.modalSaveChanges.title",
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_4()
                                .items_center()
                                .child(
                                    Button::new("clan-role-reset")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "clanSettings.modalSaveChanges.reset",
                                        ))
                                        .ghost()
                                        .on_click({
                                            let page = page.clone();
                                            move |_, window, cx| {
                                                page.update(cx, |page, cx| {
                                                    page.reset(window, cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    Button::new("clan-role-save")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "clanSettings.modalSaveChanges.saveChanges",
                                        ))
                                        .primary()
                                        .disabled(saving)
                                        .on_click({
                                            let page = page.clone();
                                            move |_, _, cx| {
                                                page.update(cx, |page, cx| page.save(cx));
                                            }
                                        }),
                                ),
                        ),
                ),
        )
}
