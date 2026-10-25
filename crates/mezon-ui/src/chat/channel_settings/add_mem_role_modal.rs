use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, ListAlignment,
    ListState, SharedString, Subscription, Task, Window, div, list, prelude::*, px,
};
use mezon_client::EveryoneExcludeType;
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ChannelUsersStore, ClanId, ClanMember, ClanMembersStore,
    MentionSearchEvent, MentionSearchStore, RoleId, RolesStore, Settings, UserId,
};

use super::channel_acl::{
    self, SearchMode, is_system_role, member_matches, parse_search, role_matches,
};
use super::permissions_tab::{
    MEMBER_ROW_HEIGHT, MemberRow, ROLE_ROW_HEIGHT, RoleRow, member_avatar, role_glyph,
    role_row_from,
};
use super::remote_members;
use crate::app::shell::Shell;
use crate::components::primitives::{
    Checkbox, Icon, IconName, Input, InputEvent, InputState, TabBar, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const SEARCH_DEBOUNCE_MS: u64 = 300;
const MODAL_WIDTH: f32 = 440.0;
const LIST_HEIGHT: f32 = 270.0;
const LIST_OVERDRAW: f32 = 200.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateRow {
    RoleHeader,
    Role(usize),
    MemberHeader,
    Member(usize),
}

fn candidate_rows(role_count: usize, member_count: usize) -> Vec<CandidateRow> {
    let mut rows = Vec::new();
    if role_count > 0 {
        rows.push(CandidateRow::RoleHeader);
        rows.extend((0..role_count).map(CandidateRow::Role));
    }
    if member_count > 0 {
        rows.push(CandidateRow::MemberHeader);
        rows.extend((0..member_count).map(CandidateRow::Member));
    }
    rows
}

fn appended_rows(old: &[CandidateRow], new: &[CandidateRow]) -> Option<usize> {
    (new.len() > old.len() && new.starts_with(old)).then(|| new.len() - old.len())
}

pub(super) enum AddMemRoleEvent {
    Staged {
        user_ids: Vec<UserId>,
        role_ids: Vec<RoleId>,
        remote_members: Vec<ClanMember>,
    },
    EveryoneExclude {
        kind: EveryoneExcludeType,
        user_ids: Vec<UserId>,
        role_ids: Vec<RoleId>,
        remote_members: Vec<ClanMember>,
    },
}

pub struct AddMemRoleModal {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    search_input: Entity<InputState>,
    query: String,
    excluded_user_ids: Vec<UserId>,
    excluded_role_ids: Vec<RoleId>,
    selected_user_ids: Vec<UserId>,
    selected_role_ids: Vec<RoleId>,
    everyone_kind: Option<EveryoneExcludeType>,
    role_rows: Rc<Vec<RoleRow>>,
    member_ids: Rc<Vec<UserId>>,
    remote_members: Rc<HashMap<UserId, ClanMember>>,
    remote_seq: Option<u64>,
    rows: Rc<Vec<CandidateRow>>,
    rows_query: String,
    list_state: ListState,
    focus_handle: FocusHandle,
    _debounce: Task<()>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AddMemRoleEvent> for AddMemRoleModal {}

impl Focusable for AddMemRoleModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddMemRoleModal {
    #[allow(clippy::too_many_arguments)]
    pub fn open<T: 'static>(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        staged_user_ids: Vec<UserId>,
        staged_role_ids: Vec<RoleId>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Entity<Self> {
        let private = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .is_some_and(|channel| channel.private);
        let modal = cx.new(|cx| {
            Self::new(
                clan_id,
                channel_id,
                settings,
                private,
                staged_user_ids,
                staged_role_ids,
                None,
                window,
                cx,
            )
        });
        let focus_handle = modal.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.clone().into(), cx));
        modal
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_everyone_exclude<T: 'static>(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        kind: EveryoneExcludeType,
        selected_user_ids: Vec<UserId>,
        selected_role_ids: Vec<RoleId>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Entity<Self> {
        let modal = cx.new(|cx| {
            Self::new(
                clan_id,
                channel_id,
                settings,
                false,
                selected_user_ids,
                selected_role_ids,
                Some(kind),
                window,
                cx,
            )
        });
        let focus_handle = modal.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.clone().into(), cx));
        modal
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        private: bool,
        staged_user_ids: Vec<UserId>,
        staged_role_ids: Vec<RoleId>,
        everyone_kind: Option<EveryoneExcludeType>,
        window: &mut Window,
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

        let placeholder = mezon_i18n::t(
            &settings.read(cx).language,
            "channelSetting.addMembersRoles.placeholder",
        )
        .to_string();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .embedded(true)
        });

        let owner = cx.entity_id().as_u64();
        let mut subs = vec![
            cx.subscribe(&search_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_search(cx);
                    this.search_remote(cx);
                }
            }),
            cx.observe(&RolesStore::global(cx), |this, _, cx| {
                this.refresh_candidates(cx);
            }),
            cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
                if remote_members::searchable(&this.typed_needle(cx)) {
                    this.search_remote(cx);
                }
                this.refresh_candidates(cx);
            }),
            cx.observe(&ChannelUsersStore::global(cx), |this, _, cx| {
                this.refresh_candidates(cx);
            }),
            cx.on_release(move |_, cx| remote_members::end_session(owner, cx)),
        ];
        if let Some(store) = MentionSearchStore::try_global(cx) {
            subs.push(cx.subscribe(&store, |this, _, _: &MentionSearchEvent, cx| {
                if this.remote_seq != this.current_remote_seq(cx) {
                    this.refresh_candidates(cx);
                }
            }));
        }

        let (excluded_user_ids, excluded_role_ids, selected_user_ids, selected_role_ids) =
            match everyone_kind {
                Some(_) => (Vec::new(), Vec::new(), staged_user_ids, staged_role_ids),
                None if private => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
                None => (staged_user_ids, staged_role_ids, Vec::new(), Vec::new()),
            };

        let mut this = Self {
            clan_id,
            channel_id,
            settings,
            search_input,
            query: String::new(),
            excluded_user_ids,
            excluded_role_ids,
            selected_user_ids,
            selected_role_ids,
            everyone_kind,
            role_rows: Rc::new(Vec::new()),
            member_ids: Rc::new(Vec::new()),
            remote_members: Rc::new(HashMap::new()),
            remote_seq: None,
            rows: Rc::new(Vec::new()),
            rows_query: String::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW))
                .smooth_line_scroll()
                .suppress_hover_while_scrolling(),
            focus_handle: cx.focus_handle(),
            _debounce: Task::ready(()),
            _subs: subs,
        };
        this.rebuild_candidates(cx);
        this
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SEARCH_DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.query = this.search_input.read(cx).value().to_string();
                this.refresh_candidates(cx);
            });
        });
    }

    fn typed_needle(&self, cx: &App) -> String {
        parse_search(self.search_input.read(cx).value()).needle
    }

    fn search_remote(&mut self, cx: &mut Context<Self>) {
        let needle = self.typed_needle(cx);
        remote_members::search(cx.entity_id().as_u64(), self.clan_id, &needle, cx);
    }

    fn current_remote_seq(&self, cx: &App) -> Option<u64> {
        remote_members::answer_seq(self.clan_id, &parse_search(&self.query).needle, cx)
    }

    fn refresh_candidates(&mut self, cx: &mut Context<Self>) {
        self.rebuild_candidates(cx);
        cx.notify();
    }

    fn rebuild_candidates(&mut self, cx: &App) {
        self.role_rows = Rc::new(self.compute_role_candidates(cx));
        let (member_ids, remote_members) = self.compute_member_candidates(cx);
        if !remote_members.is_empty() {
            let known = Rc::make_mut(&mut self.remote_members);
            for member in remote_members {
                known.insert(member.id(), member);
            }
        }
        self.remote_seq = self.current_remote_seq(cx);
        self.member_ids = Rc::new(member_ids);
        let rows = candidate_rows(self.role_rows.len(), self.member_ids.len());
        if rows != *self.rows {
            match appended_rows(&self.rows, &rows).filter(|_| self.rows_query == self.query) {
                Some(added) => self
                    .list_state
                    .splice(self.rows.len()..self.rows.len(), added),
                None => self.list_state.reset(rows.len()),
            }
            self.rows = Rc::new(rows);
        }
        self.rows_query.clone_from(&self.query);
    }

    fn is_private(&self, cx: &App) -> bool {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .is_some_and(|channel| channel.private)
    }

    fn compute_role_candidates(&self, cx: &App) -> Vec<RoleRow> {
        let search = parse_search(&self.query);
        if search.mode == SearchMode::MembersOnly
            || self.everyone_kind == Some(EveryoneExcludeType::User)
        {
            return Vec::new();
        }
        let store = RolesStore::global(cx);
        let store = store.read(cx);
        let on_channel: HashSet<RoleId> = if self.everyone_kind.is_some() {
            HashSet::new()
        } else {
            store
                .roles_for_channel(self.clan_id, self.channel_id)
                .into_iter()
                .filter(|(_, role)| role.role_channel_active)
                .map(|(role_id, _)| role_id)
                .collect()
        };
        let excluded_roles: HashSet<RoleId> = self.excluded_role_ids.iter().copied().collect();
        store
            .active_roles_in_clan(self.clan_id)
            .into_iter()
            .filter(|(role_id, role)| {
                !on_channel.contains(role_id)
                    && !excluded_roles.contains(role_id)
                    && !is_system_role(role.creator_id)
                    && role_matches(&role.name, &search.needle)
            })
            .map(|(role_id, role)| role_row_from(role_id, role))
            .collect()
    }

    fn compute_member_candidates(&self, cx: &App) -> (Vec<UserId>, Vec<ClanMember>) {
        let search = parse_search(&self.query);
        if self.everyone_kind == Some(EveryoneExcludeType::Role) {
            return (Vec::new(), Vec::new());
        }
        let in_channel: HashSet<UserId> = if self.everyone_kind.is_none() && self.is_private(cx) {
            ChannelUsersStore::global(cx)
                .read(cx)
                .user_ids(self.channel_id)
                .iter()
                .copied()
                .collect()
        } else {
            HashSet::new()
        };
        let excluded_users: HashSet<UserId> = self.excluded_user_ids.iter().copied().collect();
        let roster = ClanMembersStore::global(cx).read(cx);
        let mut member_ids: Vec<UserId> = roster
            .members(self.clan_id)
            .into_iter()
            .filter(|member| {
                let user_id = member.id();
                !in_channel.contains(&user_id)
                    && !excluded_users.contains(&user_id)
                    && member_matches(
                        &member.clan_nick,
                        &member.user.display_name,
                        &member.user.username,
                        &search.needle,
                    )
            })
            .map(|member| member.id())
            .collect();
        let mut listed: HashSet<UserId> = member_ids.iter().copied().collect();
        let remote: Vec<ClanMember> = remote_members::hits(self.clan_id, &search.needle, cx)
            .into_iter()
            .filter(|member| {
                let user_id = member.id();
                !in_channel.contains(&user_id)
                    && !excluded_users.contains(&user_id)
                    && listed.insert(user_id)
            })
            .collect();
        member_ids.extend(remote.iter().map(ClanMember::id));
        {
            let remote_by_id: HashMap<UserId, &ClanMember> =
                remote.iter().map(|member| (member.id(), member)).collect();
            remote_members::rank_by_match(&mut member_ids, &search.needle, |user_id| {
                roster
                    .member(self.clan_id, *user_id)
                    .or_else(|| remote_by_id.get(user_id).copied())
                    .map(remote_members::names)
                    .unwrap_or_default()
            });
        }
        (member_ids, remote)
    }

    fn picked_remote_members(&self, user_ids: &[UserId]) -> Vec<ClanMember> {
        user_ids
            .iter()
            .filter_map(|user_id| self.remote_members.get(user_id).cloned())
            .collect()
    }

    fn toggle_role(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        if let Some(index) = self.selected_role_ids.iter().position(|id| *id == role_id) {
            self.selected_role_ids.remove(index);
        } else {
            self.selected_role_ids.push(role_id);
        }
        cx.notify();
    }

    fn toggle_member(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        if let Some(index) = self.selected_user_ids.iter().position(|id| *id == user_id) {
            self.selected_user_ids.remove(index);
        } else {
            self.selected_user_ids.push(user_id);
        }
        cx.notify();
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn set_everyone_kind(&mut self, kind: EveryoneExcludeType, cx: &mut Context<Self>) {
        if self.everyone_kind == Some(kind) {
            return;
        }
        self.everyone_kind = Some(kind);
        self.refresh_candidates(cx);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let user_ids = std::mem::take(&mut self.selected_user_ids);
        let role_ids = std::mem::take(&mut self.selected_role_ids);
        let picked = self.picked_remote_members(&user_ids);
        if let Some(kind) = self.everyone_kind {
            let (user_ids, role_ids) = match kind {
                EveryoneExcludeType::User => (user_ids, Vec::new()),
                EveryoneExcludeType::Role => (Vec::new(), role_ids),
            };
            cx.emit(AddMemRoleEvent::EveryoneExclude {
                kind,
                user_ids,
                role_ids,
                remote_members: picked,
            });
            Self::close(cx);
            return;
        }
        if !self.is_private(cx) {
            if !user_ids.is_empty() || !role_ids.is_empty() {
                cx.emit(AddMemRoleEvent::Staged {
                    user_ids,
                    role_ids,
                    remote_members: picked,
                });
            }
            Self::close(cx);
            return;
        }
        let Some(api) = channel_acl::api(cx) else {
            Self::close(cx);
            return;
        };
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        if !user_ids.is_empty() {
            let profiles = picked
                .iter()
                .map(|member| (member.id(), remote_members::profile(member)))
                .collect();
            ChannelUsersStore::global(cx).update(cx, |store, cx| {
                store.add_users(channel_id, &user_ids, profiles, cx);
            });
        }
        if !role_ids.is_empty() {
            RolesStore::global(cx).update(cx, |store, cx| {
                store.add_roles_to_channel(clan_id, channel_id, &role_ids, cx);
            });
        }
        let locale = self.settings.read(cx).language.clone();
        cx.spawn(async move |_, cx| {
            let mut failed_users = Vec::new();
            let mut failed_roles = Vec::new();
            if !user_ids.is_empty() {
                let payload = user_ids
                    .iter()
                    .map(|id| id.get().to_string())
                    .collect::<Vec<_>>();
                if let Err(error) = api.add_channel_users(channel_id.get(), payload).await {
                    tracing::error!("add_channel_users failed for {channel_id}: {error}");
                    failed_users = user_ids;
                }
            }
            if !role_ids.is_empty() {
                let payload = role_ids
                    .iter()
                    .map(|id| id.get().to_string())
                    .collect::<Vec<_>>();
                if let Err(error) = api.add_roles_channel_desc(payload, channel_id.get()).await {
                    tracing::error!("add_roles_channel_desc failed for {channel_id}: {error}");
                    failed_roles = role_ids;
                }
            }
            if failed_users.is_empty() && failed_roles.is_empty() {
                return;
            }
            cx.update(|cx| {
                if !failed_users.is_empty() {
                    ChannelUsersStore::global(cx).update(cx, |store, cx| {
                        store.remove_users(channel_id, &failed_users, cx);
                    });
                }
                for role_id in failed_roles {
                    RolesStore::global(cx).update(cx, |store, cx| {
                        store.remove_role_from_channel(clan_id, role_id, channel_id, cx);
                    });
                }
                let message =
                    mezon_i18n::t(&locale, "clanOverviewSetting.toast.saveError").to_string();
                Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
            });
        })
        .detach();
        Self::close(cx);
    }

    fn channel_icon(&self, cx: &App) -> IconName {
        let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
        else {
            return IconName::Hashtag;
        };
        if channel.channel_type == ChannelType::Text && channel.is_age_restricted() {
            return IconName::HashtagWarning;
        }
        match (channel.private, channel.channel_type) {
            (true, ChannelType::Voice) => IconName::SpeakerLocked,
            (true, _) => IconName::HashtagLocked,
            (false, ChannelType::Voice) => IconName::Speaker,
            (false, ChannelType::Stream) => IconName::Stream,
            (false, _) => IconName::Hashtag,
        }
    }

    fn channel_label(&self, cx: &App) -> SharedString {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| SharedString::from(channel.name.clone()))
            .unwrap_or_default()
    }

    fn render_candidate_list(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self.rows.clone();
        let role_rows = self.role_rows.clone();
        let member_ids = self.member_ids.clone();
        let known_members = self.remote_members.clone();
        let selected_roles = self.selected_role_ids.clone();
        let selected_members = self.selected_user_ids.clone();
        let roles_label: SharedString =
            mezon_i18n::t(locale, "channelSetting.addMembersRoles.roles")
                .to_uppercase()
                .into();
        let members_label: SharedString =
            mezon_i18n::t(locale, "channelSetting.addMembersRoles.members")
                .to_uppercase()
                .into();
        let clan_id = self.clan_id;
        let modal = cx.entity();

        div()
            .w_full()
            .h(px(LIST_HEIGHT))
            .overflow_hidden()
            .text_color(theme.tokens.text_theme_primary)
            .child(
                list(self.list_state.clone(), move |ix, _window, cx| {
                    let theme = cx.theme().clone();
                    match rows.get(ix) {
                        Some(CandidateRow::RoleHeader) => {
                            render_section_header(roles_label.clone(), false)
                        }
                        Some(CandidateRow::MemberHeader) => {
                            render_section_header(members_label.clone(), true)
                        }
                        Some(CandidateRow::Role(role_ix)) => match role_rows.get(*role_ix) {
                            Some(row) => render_role_candidate(
                                row,
                                selected_roles.contains(&row.role_id),
                                &theme,
                                modal.clone(),
                                cx,
                            )
                            .into_any_element(),
                            None => div().h(px(ROLE_ROW_HEIGHT)).into_any_element(),
                        },
                        Some(CandidateRow::Member(member_ix)) => match member_ids.get(*member_ix) {
                            Some(user_id) => {
                                let row = remote_members::picker_row(
                                    clan_id,
                                    *user_id,
                                    &known_members,
                                    cx,
                                );
                                render_member_candidate(
                                    &row,
                                    selected_members.contains(user_id),
                                    &theme,
                                    modal.clone(),
                                )
                                .into_any_element()
                            }
                            None => div().h(px(MEMBER_ROW_HEIGHT)).into_any_element(),
                        },
                        None => div().into_any_element(),
                    }
                })
                .size_full(),
            )
    }
}

fn render_section_header(label: SharedString, spaced: bool) -> gpui::AnyElement {
    div()
        .w_full()
        .pb_4()
        .when(spaced, |el| el.pt_2())
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(label)
        .into_any_element()
}

fn render_role_candidate(
    row: &RoleRow,
    checked: bool,
    theme: &Theme,
    modal: Entity<AddMemRoleModal>,
    cx: &mut App,
) -> impl IntoElement {
    let role_id = row.role_id;
    h_flex()
        .id(("add-mem-role-role", role_id.get() as u64))
        .h(px(ROLE_ROW_HEIGHT))
        .w_full()
        .px(px(6.0))
        .py_2()
        .items_center()
        .gap_x_2()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|style| style.bg(theme.tokens.bg_item_hover))
        .child(Checkbox::new(("add-mem-role-role-check", role_id.get() as u64)).checked(checked))
        .child(role_glyph(row, cx))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .child(row.title.clone()),
        )
        .on_click(move |_, _, cx| {
            modal.update(cx, |this, cx| this.toggle_role(role_id, cx));
        })
}

fn render_member_candidate(
    row: &MemberRow,
    checked: bool,
    theme: &Theme,
    modal: Entity<AddMemRoleModal>,
) -> impl IntoElement {
    let user_id = row.user_id;
    h_flex()
        .id(("add-mem-role-member", user_id.get() as u64))
        .h(px(MEMBER_ROW_HEIGHT))
        .w_full()
        .px(px(6.0))
        .py_2()
        .items_center()
        .gap_x_2()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|style| style.bg(theme.tokens.bg_item_hover))
        .child(Checkbox::new(("add-mem-role-member-check", user_id.get() as u64)).checked(checked))
        .child(member_avatar(row, px(24.0)))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(theme.tokens.text_secondary)
                .child(row.name.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::LIGHT)
                .child(row.username.clone()),
        )
        .on_click(move |_, _, cx| {
            modal.update(cx, |this, cx| this.toggle_member(user_id, cx));
        })
}

impl Render for AddMemRoleModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .occlude()
            .w(px(MODAL_WIDTH))
            .p_6()
            .rounded(px(5.0))
            .bg(theme.tokens.theme_setting_primary)
            .text_size(px(15.0))
            .text_color(theme.tokens.text_theme_primary)
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(mezon_i18n::t(
                        locale.as_ref(),
                        "channelSetting.addMembersRoles.title",
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .items_center()
                    .gap_1()
                    .child(
                        Icon::new(self.channel_icon(cx))
                            .size(px(20.0))
                            .flex_shrink_0()
                            .text_color(theme.tokens.bg_icon_theme),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(16.0))
                            .child(self.channel_label(cx)),
                    ),
            )
            .child(
                v_flex()
                    .py_3()
                    .child(
                        div()
                            .w_full()
                            .pl_3()
                            .py(px(6.0))
                            .rounded_lg()
                            .bg(theme.tokens.bg_input_secondary)
                            .child(Input::new(&self.search_input)),
                    )
                    .child(div().pt_2().text_xs().child(mezon_i18n::t(
                        locale.as_ref(),
                        "channelSetting.addMembersRoles.instruction",
                    ))),
            )
            .when_some(self.everyone_kind, |el, kind| {
                let modal = cx.entity();
                let selected = match kind {
                    EveryoneExcludeType::Role => 0,
                    EveryoneExcludeType::User => 1,
                };
                el.child(
                    div().w_full().pb_3().child(
                        TabBar::new(vec![
                            mezon_i18n::t(locale.as_ref(), "channelSetting.addMembersRoles.roles")
                                .into(),
                            mezon_i18n::t(
                                locale.as_ref(),
                                "channelSetting.addMembersRoles.members",
                            )
                            .into(),
                        ])
                        .selected(selected)
                        .on_select(move |index, _, cx| {
                            let kind = if index == 0 {
                                EveryoneExcludeType::Role
                            } else {
                                EveryoneExcludeType::User
                            };
                            modal.update(cx, |this, cx| this.set_everyone_kind(kind, cx));
                        }),
                    ),
                )
            })
            .child(self.render_candidate_list(locale.as_ref(), &theme, cx))
            .child(
                h_flex()
                    .w_full()
                    .mt_10()
                    .justify_center()
                    .text_size(px(14.0))
                    .child(
                        div()
                            .id("add-mem-role-cancel")
                            .px_4()
                            .py_2()
                            .mr_5()
                            .rounded_lg()
                            .cursor_pointer()
                            .bg(gpui::rgb(0xd1_d5_db))
                            .text_color(gpui::rgb(0x37_41_51))
                            .hover(|style| style.bg(gpui::rgb(0x9c_a3_af)))
                            .child(mezon_i18n::t(
                                locale.as_ref(),
                                "channelSetting.addMembersRoles.cancel",
                            ))
                            .on_click(|_, _, cx| Self::close(cx)),
                    )
                    .child(
                        div()
                            .id("add-mem-role-done")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .cursor_pointer()
                            .bg(theme.tokens.button_theme_primary)
                            .text_color(gpui::white())
                            .hover(|style| style.bg(theme.tokens.bg_button_primary_hover))
                            .child(mezon_i18n::t(
                                locale.as_ref(),
                                "channelSetting.addMembersRoles.done",
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateRow, appended_rows, candidate_rows};

    #[test]
    fn both_sections_share_one_flat_row_model() {
        assert_eq!(
            candidate_rows(2, 3),
            vec![
                CandidateRow::RoleHeader,
                CandidateRow::Role(0),
                CandidateRow::Role(1),
                CandidateRow::MemberHeader,
                CandidateRow::Member(0),
                CandidateRow::Member(1),
                CandidateRow::Member(2),
            ]
        );
    }

    #[test]
    fn an_empty_section_contributes_no_header() {
        assert_eq!(
            candidate_rows(0, 2),
            vec![
                CandidateRow::MemberHeader,
                CandidateRow::Member(0),
                CandidateRow::Member(1),
            ]
        );
        assert_eq!(
            candidate_rows(1, 0),
            vec![CandidateRow::RoleHeader, CandidateRow::Role(0)]
        );
        assert!(candidate_rows(0, 0).is_empty());
    }

    #[test]
    fn server_rows_arriving_for_the_same_list_are_appended() {
        assert_eq!(
            appended_rows(&candidate_rows(1, 2), &candidate_rows(1, 5)),
            Some(3)
        );
        assert_eq!(
            appended_rows(&candidate_rows(0, 0), &candidate_rows(0, 2)),
            Some(3)
        );
        assert_eq!(
            appended_rows(&candidate_rows(1, 2), &candidate_rows(2, 2)),
            None
        );
        assert_eq!(
            appended_rows(&candidate_rows(1, 5), &candidate_rows(1, 2)),
            None
        );
        assert_eq!(
            appended_rows(&candidate_rows(1, 2), &candidate_rows(1, 2)),
            None
        );
    }

    #[test]
    fn swapping_section_sizes_changes_the_row_model_at_equal_length() {
        let a = candidate_rows(2, 3);
        let b = candidate_rows(3, 2);
        assert_eq!(a.len(), b.len());
        assert_ne!(a, b);
    }
}
