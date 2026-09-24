use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Context, ElementId, Entity, FontWeight, Hsla, ListSizingBehavior, MouseDownEvent,
    SharedString, Subscription, Task, Transformation, UniformListScrollHandle, Window, deferred,
    div, prelude::*, px, radians, rgb, size, uniform_list, white,
};
use mezon_store::{
    ChannelId, ChannelRolePermissionsEvent, ChannelRolePermissionsStore, ChannelUsersEvent,
    ChannelUsersStore, ClanId, ClanMembersEvent, ClanMembersStore, OVERRIDE_TYPE_ALLOW,
    OVERRIDE_TYPE_DENY, OVERRIDE_TYPE_NEUTRAL, PermissionDefinition, PermissionEntity,
    PermissionStore, RoleId, RolesStore, Settings, UserId,
};

use super::channel_acl::{self, member_matches, parse_search, role_matches};
use super::permissions_tab::{
    MemberRow, RoleRow, channel_member_row, member_avatar, member_row, page_count, page_slice,
    role_glyph, role_row_from,
};
use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, PaginationButton,
    Tooltip, h_flex, pagination_button, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const ADD_SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const ENTITY_ROW_HEIGHT: f32 = 48.0;
const ENTITY_PAGE_SIZE: usize = 8;
/// How long a load may run before the permission table admits to loading. A load
/// that lands sooner never shows a loading state at all, so a fast answer cannot flash.
const LOADING_REVEAL_DELAY: Duration = Duration::from_millis(250);
const ENTITY_SEARCH_HEIGHT: f32 = 28.0;
const ADD_PANEL_WIDTH: f32 = 256.0;
const ADD_LIST_HEIGHT: f32 = 256.0;
const ADD_ROLE_ROW_HEIGHT: f32 = 48.0;
const ADD_MEMBER_ROW_HEIGHT: f32 = 48.0;
const PILL_HEIGHT: f32 = 32.0;
const PILL_BUTTON_WIDTH: f32 = 36.0;
const DENY_ACTIVE: u32 = 0xda_37_3c;
const ALLOW_ACTIVE: u32 = 0x16_a3_4a;

pub(super) fn report_acl_failure(locale: &str, cx: &mut App) {
    let message = mezon_i18n::t(locale, "clanOverviewSetting.toast.saveError").to_string();
    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
}

fn permission_title_key(slug: &str) -> Option<&'static str> {
    match slug {
        "administrator" => Some("clanRoles.permissionTitles.administrator"),
        "clan-owner" => Some("clanRoles.permissionTitles.clan-owner"),
        "delete-message" => Some("clanRoles.permissionTitles.delete-message"),
        "manage-channel" => Some("clanRoles.permissionTitles.manage-channel"),
        "manage-clan" => Some("clanRoles.permissionTitles.manage-clan"),
        "manage-thread" => Some("clanRoles.permissionTitles.manage-thread"),
        "send-message" => Some("clanRoles.permissionTitles.send-message"),
        "view-channel" => Some("clanRoles.permissionTitles.view-channel"),
        _ => None,
    }
}

fn localized_permission_title(locale: &str, definition: &PermissionDefinition) -> String {
    if let Some(key) = permission_title_key(&definition.slug) {
        let text = mezon_i18n::t(locale, key);
        if text != key {
            return text.to_string();
        }
    }
    definition.title.clone()
}

fn persisted_choice(active: Option<bool>) -> i32 {
    match active {
        Some(true) => OVERRIDE_TYPE_ALLOW,
        Some(false) => OVERRIDE_TYPE_DENY,
        None => OVERRIDE_TYPE_NEUTRAL,
    }
}

fn effective_choice(pending: &HashMap<i64, i32>, permission_id: i64, active: Option<bool>) -> i32 {
    pending
        .get(&permission_id)
        .copied()
        .unwrap_or_else(|| persisted_choice(active))
}

fn apply_choice(
    pending: &mut HashMap<i64, i32>,
    permission_id: i64,
    choice: i32,
    active: Option<bool>,
) {
    if choice == persisted_choice(active) {
        pending.remove(&permission_id);
    } else {
        pending.insert(permission_id, choice);
    }
}

fn entity_element_id(entity: PermissionEntity) -> ElementId {
    match entity {
        PermissionEntity::Role(role_id) => {
            ("permission-overrides-role", role_id.get() as u64).into()
        }
        PermissionEntity::User(user_id) => {
            ("permission-overrides-user", user_id.get() as u64).into()
        }
    }
}

#[derive(Clone)]
enum EntityVisual {
    Role(RoleRow),
    Member(MemberRow),
}

#[derive(Clone)]
struct EntityRow {
    entity: PermissionEntity,
    /// Name shown in the row and echoed in the permission column's header.
    title: SharedString,
    /// Second line for a member row: the username the roster/listing carries.
    subtitle: SharedString,
    visual: EntityVisual,
}

#[derive(Clone)]
struct PermissionRow {
    id: i64,
    title: SharedString,
}

#[derive(Clone)]
struct AddRoleRow {
    role_id: RoleId,
    title: SharedString,
}

pub struct PermissionOverrides {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    expanded: bool,
    selected: Option<PermissionEntity>,
    selected_index: Option<usize>,
    /// Whether the selected entity's load has outlasted [`LOADING_REVEAL_DELAY`].
    loading_revealed: bool,
    _loading_reveal: Task<()>,
    saving: bool,
    pending: HashMap<i64, i32>,
    entities: Rc<Vec<EntityRow>>,
    visible_entities: Rc<Vec<EntityRow>>,
    entity_query: String,
    entity_page: usize,
    entity_search: Option<Entity<InputState>>,
    entity_search_sub: Option<Subscription>,
    permissions: Rc<Vec<PermissionRow>>,
    add_open: bool,
    add_input: Option<Entity<InputState>>,
    add_query: String,
    add_roles: Rc<Vec<AddRoleRow>>,
    add_members: Rc<Vec<UserId>>,
    add_member_scroll: UniformListScrollHandle,
    add_input_sub: Option<Subscription>,
    add_search_debounce: Task<()>,
    _subs: Vec<Subscription>,
}

impl PermissionOverrides {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        ChannelUsersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(channel_id, cx);
        });
        RolesStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });
        PermissionStore::global(cx).update(cx, |store, cx| {
            store.ensure_catalog_loaded(cx);
        });

        let mut subs = vec![
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
            cx.observe(&RolesStore::global(cx), |this, _, cx| this.refresh(cx)),
            cx.subscribe(
                &ChannelUsersStore::global(cx),
                |this, _, event: &ChannelUsersEvent, cx| {
                    let ChannelUsersEvent::Changed { channel_id } = event;
                    if *channel_id == this.channel_id {
                        this.refresh(cx);
                    }
                },
            ),
            cx.subscribe(
                &ClanMembersStore::global(cx),
                |this, _, event: &ClanMembersEvent, cx| {
                    if event.clan_id() == this.clan_id {
                        this.refresh(cx);
                    }
                },
            ),
            cx.observe(&PermissionStore::global(cx), |this, _, cx| this.refresh(cx)),
        ];
        if let Some(store) = ChannelRolePermissionsStore::try_global(cx) {
            subs.push(cx.observe(&store, |_, _, cx| cx.notify()));
            subs.push(cx.subscribe(
                &store,
                |this, store, event: &ChannelRolePermissionsEvent, cx| match event {
                    ChannelRolePermissionsEvent::Changed { channel_id, entity }
                        if *channel_id == this.channel_id && Some(*entity) == this.selected =>
                    {
                        if this.saving && !store.read(cx).is_saving(*channel_id, *entity) {
                            this.pending.clear();
                            this.saving = false;
                        }
                        cx.notify();
                    }
                    ChannelRolePermissionsEvent::SaveFailed { channel_id, entity }
                        if *channel_id == this.channel_id && Some(*entity) == this.selected =>
                    {
                        this.saving = false;
                        let locale = this.settings.read(cx).language.clone();
                        let message = mezon_i18n::t(&locale, "clanOverviewSetting.toast.saveError")
                            .to_string();
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        cx.notify();
                    }
                    _ => {}
                },
            ));
        }

        let mut this = Self {
            clan_id,
            channel_id,
            settings,
            expanded: true,
            selected: None,
            selected_index: None,
            loading_revealed: false,
            _loading_reveal: Task::ready(()),
            saving: false,
            pending: HashMap::new(),
            entities: Rc::new(Vec::new()),
            visible_entities: Rc::new(Vec::new()),
            entity_query: String::new(),
            entity_page: 0,
            entity_search: None,
            entity_search_sub: None,
            permissions: Rc::new(Vec::new()),
            add_open: false,
            add_input: None,
            add_query: String::new(),
            add_roles: Rc::new(Vec::new()),
            add_members: Rc::new(Vec::new()),
            add_member_scroll: UniformListScrollHandle::new(),
            add_input_sub: None,
            add_search_debounce: Task::ready(()),
            _subs: subs,
        };
        this.rebuild(cx);
        this
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_empty() || self.saving {
            return;
        }
        self.pending.clear();
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_empty() || self.saving {
            return;
        }
        let Some(entity) = self.selected else {
            return;
        };
        let Some(store) = ChannelRolePermissionsStore::try_global(cx) else {
            return;
        };
        if store.read(cx).is_saving(self.channel_id, entity) {
            return;
        }
        self.saving = true;
        let pending = self.pending.clone();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        store.update(cx, |store, cx| {
            store.save(clan_id, channel_id, entity, &pending, cx);
        });
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rebuild(cx);
        cx.notify();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        self.entities = Rc::new(self.compute_entities(cx));
        self.apply_entity_filter();
        self.permissions = Rc::new(self.compute_permissions(&locale, cx));
        self.rebuild_add_candidates(cx);
        self.ensure_selection(cx);
    }

    fn compute_entities(&self, cx: &App) -> Vec<EntityRow> {
        let mut rows = Vec::new();
        let roles_store = RolesStore::global(cx);
        for (role_id, role) in roles_store
            .read(cx)
            .roles_for_channel(self.clan_id, self.channel_id)
        {
            let visual = role_row_from(role_id, role);
            rows.push(EntityRow {
                entity: PermissionEntity::Role(role_id),
                title: visual.title.clone(),
                subtitle: SharedString::default(),
                visual: EntityVisual::Role(visual),
            });
        }
        let users = ChannelUsersStore::global(cx);
        for &user_id in users.read(cx).user_ids(self.channel_id) {
            // The same row the member list draws, identity fallback included: the clan
            // roster is capped server-side and drops anyone who left the clan, so this
            // row would otherwise be untitled.
            let member = channel_member_row(self.clan_id, self.channel_id, user_id, cx);
            rows.push(EntityRow {
                entity: PermissionEntity::User(user_id),
                title: if member.name.is_empty() {
                    member.username.clone()
                } else {
                    member.name.clone()
                },
                subtitle: member.username.clone(),
                visual: EntityVisual::Member(member),
            });
        }
        rows
    }

    /// Entities matching the column's own search box, page-clamped like the member list.
    fn apply_entity_filter(&mut self) {
        let needle = parse_search(&self.entity_query).needle;
        let visible = if needle.is_empty() {
            self.entities.clone()
        } else {
            Rc::new(
                self.entities
                    .iter()
                    .filter(|row| {
                        member_matches(&row.title, "", &row.subtitle, &needle)
                            || role_matches(&row.title, &needle)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        self.entity_page = self
            .entity_page
            .min(page_count(visible.len(), ENTITY_PAGE_SIZE).saturating_sub(1));
        self.visible_entities = visible;
    }

    fn ensure_entity_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entity_search.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        // This column lists roles as well, so it borrows the add panel's wording.
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "channelSetting.addMembersRoles.searchPlaceholder").into();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(ENTITY_SEARCH_HEIGHT))
                .text_size(px(13.0))
                .embedded(true)
        });
        self.entity_search_sub =
            Some(cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if *event != InputEvent::Change {
                    return;
                }
                this.entity_query = input.read(cx).value().to_string();
                this.entity_page = 0;
                this.apply_entity_filter();
                cx.notify();
            }));
        self.entity_search = Some(input);
    }

    fn go_to_entity_page(&mut self, pick: impl FnOnce(usize) -> usize, cx: &mut Context<Self>) {
        let pages = page_count(self.visible_entities.len(), ENTITY_PAGE_SIZE);
        let next = pick(self.entity_page).min(pages.saturating_sub(1));
        if next == self.entity_page {
            return;
        }
        self.entity_page = next;
        cx.notify();
    }

    fn selected_title(&self) -> Option<SharedString> {
        self.entities
            .get(self.selected_index?)
            .map(|row| row.title.clone())
    }

    fn compute_permissions(&self, locale: &str, cx: &App) -> Vec<PermissionRow> {
        let Some(store) = PermissionStore::try_global(cx) else {
            return Vec::new();
        };
        store
            .read(cx)
            .channel_scoped_definitions()
            .map(|definition| PermissionRow {
                id: definition.id,
                title: localized_permission_title(locale, definition).into(),
            })
            .collect()
    }

    fn rebuild_add_candidates(&mut self, cx: &App) {
        if !self.add_open {
            self.add_roles = Rc::new(Vec::new());
            self.add_members = Rc::new(Vec::new());
            return;
        }
        let needle = self.add_query.trim().to_lowercase();
        let on_channel_members: std::collections::HashSet<_> = ChannelUsersStore::global(cx)
            .read(cx)
            .user_ids(self.channel_id)
            .iter()
            .copied()
            .collect();
        let roles_store = RolesStore::global(cx);
        let roles_store = roles_store.read(cx);
        let on_channel: Vec<RoleId> = roles_store
            .roles_for_channel(self.clan_id, self.channel_id)
            .into_iter()
            .map(|(role_id, _)| role_id)
            .collect();
        self.add_roles = Rc::new(
            roles_store
                .active_roles_in_clan(self.clan_id)
                .into_iter()
                .filter(|(role_id, role)| {
                    !on_channel.contains(role_id) && role_matches(&role.name, &needle)
                })
                .map(|(role_id, role)| AddRoleRow {
                    role_id,
                    title: role.name.clone().into(),
                })
                .collect(),
        );
        let members_store = ClanMembersStore::global(cx);
        self.add_members = Rc::new(
            members_store
                .read(cx)
                .members(self.clan_id)
                .into_iter()
                .filter(|member| {
                    !on_channel_members.contains(&member.id())
                        && member_matches(
                            &member.clan_nick,
                            &member.user.display_name,
                            &member.user.username,
                            &needle,
                        )
                })
                .map(|member| member.id())
                .collect(),
        );
    }

    fn ensure_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(selected) = self.selected {
            self.selected_index = self.entities.iter().position(|row| row.entity == selected);
            if self.selected_index.is_none() {
                self.selected = None;
                self.pending.clear();
                self.saving = false;
            } else {
                self.ensure_selected_loaded(selected, cx);
                return;
            }
        }
        let Some(first) = self.entities.first().map(|row| row.entity) else {
            return;
        };
        self.select_entity(first, cx);
    }

    fn ensure_selected_loaded(&self, entity: PermissionEntity, cx: &mut Context<Self>) {
        let Some(store) = ChannelRolePermissionsStore::try_global(cx) else {
            return;
        };
        if store.read(cx).is_loaded(self.channel_id, entity) {
            return;
        }
        let channel_id = self.channel_id;
        store.update(cx, |store, cx| {
            store.ensure_loaded(channel_id, entity, cx);
        });
    }

    fn select_entity(&mut self, entity: PermissionEntity, cx: &mut Context<Self>) {
        self.selected = Some(entity);
        self.selected_index = self.entities.iter().position(|row| row.entity == entity);
        self.pending.clear();
        self.arm_loading_reveal(entity, cx);
        let channel_id = self.channel_id;
        if let Some(store) = ChannelRolePermissionsStore::try_global(cx) {
            store.update(cx, |store, cx| {
                store.ensure_loaded(channel_id, entity, cx);
            });
        }
        cx.notify();
    }

    fn arm_loading_reveal(&mut self, entity: PermissionEntity, cx: &mut Context<Self>) {
        self.loading_revealed = false;
        let loaded = ChannelRolePermissionsStore::try_global(cx)
            .is_some_and(|store| store.read(cx).is_loaded(self.channel_id, entity));
        if loaded {
            self._loading_reveal = Task::ready(());
            return;
        }
        self._loading_reveal = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOADING_REVEAL_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_revealed = true;
                cx.notify();
            });
        });
    }

    /// Start loading an entity the pointer is resting on. The click usually follows
    /// well after the answer does, so selecting it has nothing left to wait for.
    fn prefetch_entity(&self, entity: PermissionEntity, cx: &mut Context<Self>) {
        self.ensure_selected_loaded(entity, cx);
    }

    /// The first moments of a load, before [`LOADING_REVEAL_DELAY`] runs out: the table
    /// keeps its resting look instead of blanking and dimming for a few frames.
    fn loading_in_grace(&self, cx: &App) -> bool {
        !self.loading_revealed
            && self.selected.is_some_and(|entity| {
                ChannelRolePermissionsStore::try_global(cx)
                    .is_some_and(|store| store.read(cx).is_loading(self.channel_id, entity))
            })
    }

    fn request_select(&mut self, entity: PermissionEntity, cx: &mut Context<Self>) {
        if !self.pending.is_empty() || self.selected == Some(entity) {
            return;
        }
        self.select_entity(entity, cx);
    }

    fn persisted_active(&self, permission_id: i64, cx: &App) -> Option<bool> {
        let entity = self.selected?;
        ChannelRolePermissionsStore::try_global(cx)?
            .read(cx)
            .permission_active(self.channel_id, entity, permission_id)
    }

    fn set_choice(&mut self, permission_id: i64, choice: i32, cx: &mut Context<Self>) {
        if !self.can_edit(cx) {
            return;
        }
        let active = self.persisted_active(permission_id, cx);
        apply_choice(&mut self.pending, permission_id, choice, active);
        cx.notify();
    }

    fn can_edit(&self, cx: &App) -> bool {
        !self.saving
            && self.selected.is_some_and(|entity| {
                ChannelRolePermissionsStore::try_global(cx)
                    .is_some_and(|store| store.read(cx).is_loaded(self.channel_id, entity))
            })
    }

    fn toggle_expanded(&mut self, cx: &mut Context<Self>) {
        self.expanded = !self.expanded;
        cx.notify();
    }

    fn toggle_add_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_open = !self.add_open;
        if self.add_open {
            self.ensure_add_input(window, cx);
        }
        self.rebuild_add_candidates(cx);
        cx.notify();
    }

    fn close_add_popup(&mut self, cx: &mut Context<Self>) {
        if !self.add_open {
            return;
        }
        self.add_open = false;
        self.add_search_debounce = Task::ready(());
        self.rebuild_add_candidates(cx);
        cx.notify();
    }

    fn ensure_add_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_input.is_some() {
            return;
        }
        let placeholder = mezon_i18n::t(
            &self.settings.read(cx).language,
            "channelSetting.channelPermission.bottomSheet.roleMemberPlaceholder",
        )
        .to_string();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .embedded(true)
        });
        self.add_input_sub = Some(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_add_search(cx);
            }
        }));
        self.add_input = Some(input);
    }

    fn schedule_add_search(&mut self, cx: &mut Context<Self>) {
        self.add_search_debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ADD_SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                let Some(input) = this.add_input.clone() else {
                    return;
                };
                let query = input.read(cx).value().to_string();
                if query == this.add_query {
                    return;
                }
                this.add_query = query;
                this.rebuild_add_candidates(cx);
                cx.notify();
            });
        });
    }

    fn add_role(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        self.close_add_popup(cx);
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        RolesStore::global(cx).update(cx, |store, cx| {
            store.add_roles_to_channel(clan_id, channel_id, &[role_id], cx);
        });
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        let locale = self.settings.read(cx).language.clone();
        cx.spawn(async move |_, cx| {
            if let Err(error) = api
                .add_roles_channel_desc(vec![role_id.get().to_string()], channel_id.get())
                .await
            {
                tracing::error!("add_roles_channel_desc failed for {channel_id}: {error}");
                cx.update(|cx| {
                    RolesStore::global(cx).update(cx, |store, cx| {
                        store.remove_role_from_channel(clan_id, role_id, channel_id, cx);
                    });
                    report_acl_failure(&locale, cx);
                });
            }
        })
        .detach();
    }

    fn add_member(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        self.close_add_popup(cx);
        let channel_id = self.channel_id;
        ChannelUsersStore::global(cx).update(cx, |store, cx| {
            store.add_users(channel_id, &[user_id], cx);
        });
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        let locale = self.settings.read(cx).language.clone();
        cx.spawn(async move |_, cx| {
            if let Err(error) = api
                .add_channel_users(channel_id.get(), vec![user_id.get().to_string()])
                .await
            {
                tracing::error!("add_channel_users failed for {channel_id}: {error}");
                cx.update(|cx| {
                    ChannelUsersStore::global(cx).update(cx, |store, cx| {
                        store.remove_users(channel_id, &[user_id], cx);
                    });
                    report_acl_failure(&locale, cx);
                });
            }
        })
        .detach();
    }

    fn render_header(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.expanded;
        h_flex().child(
            h_flex()
                .id("permission-overrides-header")
                .items_center()
                .gap_x_3p5()
                .cursor_pointer()
                .text_color(theme.tokens.text_theme_primary)
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(mezon_i18n::t(
                            locale,
                            "channelSetting.channelPermission.permissionOverrides",
                        )),
                )
                .child(
                    Icon::new(IconName::ArrowDown)
                        .size(px(20.0))
                        .text_color(theme.tokens.text_theme_primary)
                        .when(!expanded, |icon| {
                            icon.with_transformation(Transformation::rotate(radians(-FRAC_PI_2)))
                        }),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_expanded(cx))),
        )
    }

    fn render_entity_header(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .relative()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_x_2()
                    .items_center()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(
                        mezon_i18n::t(
                            locale,
                            "channelSetting.channelPermission.bottomSheet.rolesMembers",
                        )
                        .to_uppercase(),
                    )
                    .when(!self.entities.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_shrink_0()
                                .px_2()
                                .rounded_full()
                                .bg(theme.tokens.bg_tertiary)
                                .child(self.entity_count_label()),
                        )
                    }),
            )
            // Only the button toggles the panel now: the header row used to swallow every
            // click, so tapping the label alone opened the add popup.
            .child(
                Button::new("permission-overrides-add")
                    .label(mezon_i18n::t(
                        locale,
                        "channelSetting.channelPermission.bottomSheet.add",
                    ))
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_add_popup(window, cx);
                    })),
            )
            .when(self.add_open, |el| {
                el.child(deferred(self.render_add_panel(locale, theme, cx)))
            })
    }

    fn render_add_panel(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let roles = self.add_roles.clone();
        let members = self.add_members.clone();
        let count = roles.len() + members.len();
        let clan_id = self.clan_id;
        let view = cx.entity();
        div()
            .absolute()
            .top(px(36.0))
            .left_0()
            .w(px(ADD_PANEL_WIDTH))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_add_popup(cx)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .child(Icon::new(IconName::Search).size(px(16.0)))
                    .when_some(self.add_input.clone(), |el, input| {
                        el.child(div().flex_1().min_w_0().child(Input::new(&input)))
                    }),
            )
            .when(count == 0, |el| {
                el.child(
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(
                            locale,
                            "channelSetting.channelPermission.noMembersFound",
                        )),
                )
            })
            .when(count > 0, |el| {
                el.child(
                    uniform_list(
                        "permission-overrides-add-list",
                        count,
                        move |range, _, cx| {
                            let theme = cx.theme().clone();
                            range
                                .map(|ix| {
                                    if let Some(role) = roles.get(ix) {
                                        render_add_role_row(role, &theme, view.clone())
                                            .into_any_element()
                                    } else {
                                        let member =
                                            member_row(clan_id, members[ix - roles.len()], cx);
                                        render_add_member_row(&member, &theme, view.clone())
                                            .into_any_element()
                                    }
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    .with_item_size(size(px(0.0), px(ADD_MEMBER_ROW_HEIGHT)))
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .track_scroll(&self.add_member_scroll)
                    .suppress_hover_while_scrolling()
                    .h(px(ADD_LIST_HEIGHT))
                    .w_full(),
                )
            })
    }

    fn render_entity_list(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.entities.is_empty() {
            return div().into_any_element();
        }
        if self.visible_entities.is_empty() {
            return div()
                .py_2()
                .text_sm()
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(
                    locale,
                    "channelSetting.channelPermission.noMembersFound",
                ))
                .into_any_element();
        }
        let view = cx.entity();
        v_flex()
            .w_full()
            .children(
                page_slice(&self.visible_entities, self.entity_page, ENTITY_PAGE_SIZE)
                    .iter()
                    .map(|row| {
                        render_entity_row(
                            row,
                            self.selected == Some(row.entity),
                            theme,
                            view.clone(),
                            cx,
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_entity_column(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_entity_search(window, cx);
        v_flex()
            .w(px(260.0))
            .flex_shrink_0()
            .min_w_0()
            .child(self.render_entity_header(locale, theme, cx))
            .when_some(self.entity_search.clone(), |col, input| {
                col.child(
                    h_flex()
                        .mt_2()
                        .w_full()
                        .px_2()
                        .gap_2()
                        .rounded_lg()
                        .bg(theme.tokens.bg_input_secondary)
                        .child(Icon::new(IconName::Search).size(px(16.0)))
                        .child(div().flex_1().min_w_0().child(Input::new(&input))),
                )
            })
            .child(
                div()
                    .mt_2()
                    .w_full()
                    .min_h(px(ENTITY_ROW_HEIGHT * ENTITY_PAGE_SIZE as f32))
                    .child(self.render_entity_list(locale, theme, cx)),
            )
            .child(self.render_entity_pagination(cx))
    }

    fn entity_count_label(&self) -> SharedString {
        let total = self.entities.len();
        let matched = self.visible_entities.len();
        if matched == total {
            total.to_string().into()
        } else {
            format!("{matched}/{total}").into()
        }
    }

    fn render_entity_pagination(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = page_count(self.visible_entities.len(), ENTITY_PAGE_SIZE);
        if pages <= 1 {
            return div().into_any_element();
        }
        let current = self.entity_page.min(pages - 1);
        let theme = cx.theme().clone();
        let mut bar = h_flex()
            .w_full()
            .pt_2()
            .gap_1()
            .items_center()
            .justify_center();
        bar = bar.child(
            pagination_button(
                "permission-overrides-entities",
                PaginationButton::Previous,
                current == 0,
                false,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.go_to_entity_page(|page| page.saturating_sub(1), cx)
            })),
        );
        bar = bar.child(
            div()
                .w(px(88.0))
                .flex_shrink_0()
                .text_center()
                .text_sm()
                .child(format!("{} / {}", current + 1, pages)),
        );
        bar.child(
            pagination_button(
                "permission-overrides-entities",
                PaginationButton::Next,
                current + 1 >= pages,
                false,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.go_to_entity_page(|page| page.saturating_add(1), cx)
            })),
        )
        .into_any_element()
    }

    fn render_permission_column(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (loaded, loading) = self
            .selected
            .and_then(|entity| {
                let store = ChannelRolePermissionsStore::try_global(cx)?;
                let store = store.read(cx);
                Some((
                    store.is_loaded(self.channel_id, entity),
                    store.is_loading(self.channel_id, entity),
                ))
            })
            .unwrap_or_default();
        let show_loading = loading && self.loading_revealed;
        let mut list = v_flex().w_full().gap_2();
        for row in self.permissions.iter() {
            let choice = if loaded {
                Some(effective_choice(
                    &self.pending,
                    row.id,
                    self.persisted_active(row.id, cx),
                ))
            } else if loading && !show_loading {
                // Most entities carry no override, so the resting state is what a fast
                // load lands on anyway; editing stays off until the real answer is in.
                Some(effective_choice(&self.pending, row.id, None))
            } else {
                None
            };
            list = list.child(self.render_permission_row(row, choice, theme, cx));
        }
        let status = h_flex()
            .h(px(PILL_HEIGHT))
            .flex_shrink_0()
            .items_center()
            .text_xs()
            .text_color(theme.tokens.text_secondary)
            .when(show_loading, |el| {
                el.child(mezon_i18n::t(locale, "root.loading"))
            })
            .when(self.selected.is_some() && !loaded && !loading, |el| {
                el.child(
                    Button::new("permission-overrides-retry")
                        .label(mezon_i18n::t(
                            locale,
                            "channelSetting.channelPermission.loadPermissions",
                        ))
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(entity) = this.selected {
                                this.ensure_selected_loaded(entity, cx);
                            }
                        })),
                )
            });
        v_flex()
            .flex_1()
            .min_w_0()
            .text_color(theme.tokens.text_theme_primary)
            .child(
                v_flex()
                    .mb_4()
                    .w_full()
                    .gap_2()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(PILL_HEIGHT))
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .child(mezon_i18n::t(
                                        locale,
                                        "channelSetting.channelPermission.generalChannelPermission",
                                    )),
                            )
                            .child(status),
                    )
                    .when_some(self.selected_title(), |el, title| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                    })
                    .child(div().text_xs().child(mezon_i18n::t(
                        locale,
                        "channelSetting.channelPermission.choiceLegend",
                    ))),
            )
            .child(list)
            .when(!self.pending.is_empty(), |el| {
                el.child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(theme.tokens.bg_input_secondary)
                        .text_sm()
                        .child(mezon_i18n::t(
                            locale,
                            "channelSetting.channelPermission.saveBeforeSwitch",
                        )),
                )
            })
    }

    fn render_permission_row(
        &self,
        row: &PermissionRow,
        choice: Option<i32>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .py_2()
            .border_b_1()
            .border_color(theme.tokens.border_primary)
            .child(
                div()
                    .min_w_0()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(row.title.clone()),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h(px(PILL_HEIGHT))
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .bg(theme.tokens.theme_setting_primary)
                    .child(self.render_pill_button(row.id, OVERRIDE_TYPE_DENY, choice, theme, cx))
                    .child(self.render_pill_button(
                        row.id,
                        OVERRIDE_TYPE_NEUTRAL,
                        choice,
                        theme,
                        cx,
                    ))
                    .child(self.render_pill_button(row.id, OVERRIDE_TYPE_ALLOW, choice, theme, cx)),
            )
    }

    fn render_pill_button(
        &self,
        permission_id: i64,
        option: i32,
        choice: Option<i32>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = choice == Some(option);
        let can_edit = self.can_edit(cx);
        let locale = &self.settings.read(cx).language;
        let label = mezon_i18n::t(
            locale,
            match option {
                OVERRIDE_TYPE_DENY => "channelSetting.channelPermission.deny",
                OVERRIDE_TYPE_ALLOW => "channelSetting.channelPermission.allow",
                _ => "channelSetting.channelPermission.inherit",
            },
        );
        let (element_id, icon) = match option {
            OVERRIDE_TYPE_DENY => ("permission-overrides-deny", IconName::Close),
            OVERRIDE_TYPE_ALLOW => ("permission-overrides-allow", IconName::IconTick),
            _ => ("permission-overrides-neutral", IconName::IconOr),
        };
        let icon_color = if active && option != OVERRIDE_TYPE_NEUTRAL {
            white()
        } else {
            Hsla::from(theme.tokens.text_theme_primary)
        };
        div()
            .id((element_id, permission_id as u64))
            .tooltip(move |_, cx| Tooltip::build(label, cx))
            .when(!can_edit && !self.loading_in_grace(cx), |el| {
                el.opacity(0.5)
            })
            .w(px(PILL_BUTTON_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .when(can_edit, |el| el.cursor_pointer())
            .when(active && option == OVERRIDE_TYPE_DENY, |el| {
                el.bg(rgb(DENY_ACTIVE))
            })
            .when(active && option == OVERRIDE_TYPE_ALLOW, |el| {
                el.bg(rgb(ALLOW_ACTIVE))
            })
            .when(active && option == OVERRIDE_TYPE_NEUTRAL, |el| {
                el.bg(theme.tokens.bg_active_member_channel)
            })
            .child(Icon::new(icon).size(px(16.0)).text_color(icon_color))
            .when(can_edit, |el| {
                el.on_click(
                    cx.listener(move |this, _, _, cx| this.set_choice(permission_id, option, cx)),
                )
            })
    }
}

fn render_entity_row(
    row: &EntityRow,
    selected: bool,
    theme: &Theme,
    view: Entity<PermissionOverrides>,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let entity = row.entity;
    // Same shape as the member list above, so a name means the same thing in both places.
    let glyph = match &row.visual {
        EntityVisual::Role(role) => role_glyph(role, cx),
        EntityVisual::Member(member) => member_avatar(member, px(24.0)).into_any_element(),
    };
    let prefetch_view = view.clone();
    h_flex()
        .id(entity_element_id(entity))
        .on_hover(move |hovered, _, cx| {
            if *hovered {
                prefetch_view.update(cx, |this, cx| this.prefetch_entity(entity, cx));
            }
        })
        .h(px(ENTITY_ROW_HEIGHT))
        .w_full()
        .py(px(6.0))
        .px(px(10.0))
        .gap_x_2()
        .items_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.tokens.text_theme_primary)
        .hover(|style| style.bg(theme.tokens.bg_item_hover))
        .when(selected, |el| el.bg(theme.tokens.bg_active_member_channel))
        .child(glyph)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().text_sm().child(row.title.clone()))
                .when(
                    !row.subtitle.is_empty() && row.subtitle != row.title,
                    |el| {
                        el.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.tokens.text_secondary)
                                .child(row.subtitle.clone()),
                        )
                    },
                ),
        )
        .on_click(move |_, _, cx| {
            view.update(cx, |this, cx| this.request_select(entity, cx));
        })
}

fn render_add_role_row(
    row: &AddRoleRow,
    theme: &Theme,
    view: Entity<PermissionOverrides>,
) -> impl IntoElement {
    let role_id = row.role_id;
    h_flex()
        .id(("permission-overrides-add-role", role_id.get() as u64))
        .h(px(ADD_ROLE_ROW_HEIGHT))
        .w_full()
        .px_3()
        .py_2()
        .items_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|style| {
            style
                .bg(theme.tokens.bg_item_hover)
                .text_color(theme.tokens.text_secondary)
        })
        .child(div().flex_1().min_w_0().truncate().child(row.title.clone()))
        .on_click(move |_, _, cx| {
            view.update(cx, |this, cx| this.add_role(role_id, cx));
        })
}

fn render_add_member_row(
    row: &super::permissions_tab::MemberRow,
    theme: &Theme,
    view: Entity<PermissionOverrides>,
) -> impl IntoElement {
    let user_id = row.user_id;
    h_flex()
        .id(("permission-overrides-add-member", user_id.get() as u64))
        .h(px(ADD_MEMBER_ROW_HEIGHT))
        .w_full()
        .px_3()
        .py_2()
        .gap_x_2()
        .items_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|style| {
            style
                .bg(theme.tokens.bg_item_hover)
                .text_color(theme.tokens.text_secondary)
        })
        .child(member_avatar(row, px(32.0)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(row.name.clone()),
        )
        .on_click(move |_, _, cx| {
            view.update(cx, |this, cx| this.add_member(user_id, cx));
        })
}

impl Render for PermissionOverrides {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let show_main = self.expanded;

        v_flex()
            .w_full()
            .child(self.render_header(&locale, &theme, cx))
            .when(show_main, |el| {
                el.child(
                    h_flex()
                        .mt_4()
                        .w_full()
                        .gap_x_4()
                        .items_start()
                        .child(self.render_entity_column(&locale, &theme, window, cx))
                        .child(self.render_permission_column(&locale, &theme, cx)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverting_to_the_persisted_value_drops_the_entry() {
        let mut pending = HashMap::new();
        apply_choice(&mut pending, 1, OVERRIDE_TYPE_DENY, Some(true));
        assert_eq!(pending.get(&1), Some(&OVERRIDE_TYPE_DENY));

        apply_choice(&mut pending, 1, OVERRIDE_TYPE_ALLOW, Some(true));
        assert!(pending.is_empty());

        apply_choice(&mut pending, 2, OVERRIDE_TYPE_NEUTRAL, Some(false));
        assert_eq!(pending.get(&2), Some(&OVERRIDE_TYPE_NEUTRAL));
        apply_choice(&mut pending, 2, OVERRIDE_TYPE_DENY, Some(false));
        assert!(pending.is_empty());
    }

    #[test]
    fn neutral_on_an_unset_permission_drops_the_entry() {
        let mut pending = HashMap::from([(7, OVERRIDE_TYPE_ALLOW)]);
        apply_choice(&mut pending, 7, OVERRIDE_TYPE_NEUTRAL, None);
        assert!(pending.is_empty());

        apply_choice(&mut pending, 8, OVERRIDE_TYPE_NEUTRAL, None);
        assert!(pending.is_empty());
    }

    #[test]
    fn changing_away_from_the_persisted_value_records_the_entry() {
        let mut pending = HashMap::new();
        apply_choice(&mut pending, 1, OVERRIDE_TYPE_ALLOW, None);
        apply_choice(&mut pending, 2, OVERRIDE_TYPE_DENY, None);
        apply_choice(&mut pending, 3, OVERRIDE_TYPE_ALLOW, Some(false));
        apply_choice(&mut pending, 4, OVERRIDE_TYPE_NEUTRAL, Some(true));

        assert_eq!(pending.get(&1), Some(&OVERRIDE_TYPE_ALLOW));
        assert_eq!(pending.get(&2), Some(&OVERRIDE_TYPE_DENY));
        assert_eq!(pending.get(&3), Some(&OVERRIDE_TYPE_ALLOW));
        assert_eq!(pending.get(&4), Some(&OVERRIDE_TYPE_NEUTRAL));
    }

    #[test]
    fn persisted_state_maps_to_the_tri_state_options() {
        assert_eq!(persisted_choice(Some(true)), OVERRIDE_TYPE_ALLOW);
        assert_eq!(persisted_choice(Some(false)), OVERRIDE_TYPE_DENY);
        assert_eq!(persisted_choice(None), OVERRIDE_TYPE_NEUTRAL);
    }

    #[test]
    fn effective_choice_prefers_the_pending_diff() {
        let pending = HashMap::from([(1, OVERRIDE_TYPE_DENY)]);
        assert_eq!(
            effective_choice(&pending, 1, Some(true)),
            OVERRIDE_TYPE_DENY
        );
        assert_eq!(
            effective_choice(&pending, 2, Some(true)),
            OVERRIDE_TYPE_ALLOW
        );
        assert_eq!(effective_choice(&pending, 3, None), OVERRIDE_TYPE_NEUTRAL);
    }

    #[test]
    fn permission_title_falls_back_to_the_server_title() {
        let known = PermissionDefinition {
            id: 1,
            slug: "send-message".into(),
            title: "Server Title".into(),
            description: String::new(),
            level: 0,
            scope: 2,
        };
        assert_eq!(localized_permission_title("en", &known), "Send Messages");

        let unknown = PermissionDefinition {
            id: 2,
            slug: "brand-new-permission".into(),
            title: "Brand New".into(),
            description: String::new(),
            level: 0,
            scope: 2,
        };
        assert_eq!(localized_permission_title("en", &unknown), "Brand New");
    }
}
