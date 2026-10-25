use std::sync::Arc;

use gpui::{App, Global};
use mezon_client::{AppApi, EveryoneExcludeType};
use mezon_store::{ChannelType, RoleId, UserId};

struct GlobalChannelAclApi(Arc<AppApi>);

impl Global for GlobalChannelAclApi {}

pub fn init(api: Arc<AppApi>, cx: &mut App) {
    cx.set_global(GlobalChannelAclApi(api));
}

pub fn api(cx: &App) -> Option<Arc<AppApi>> {
    cx.try_global::<GlobalChannelAclApi>()
        .map(|global| global.0.clone())
}

pub fn channel_private_payload(private_enabled: bool) -> i32 {
    if private_enabled { 0 } else { 1 }
}

pub fn acl_user_ids(
    private_enabled: bool,
    selected: &[UserId],
    creator_id: Option<UserId>,
) -> Vec<i64> {
    let mut ids: Vec<i64> = Vec::new();
    if private_enabled {
        for user_id in selected {
            if user_id.is_zero() || ids.contains(&user_id.get()) {
                continue;
            }
            ids.push(user_id.get());
        }
    }
    if let Some(creator_id) = creator_id
        && !creator_id.is_zero()
        && !ids.contains(&creator_id.get())
    {
        ids.push(creator_id.get());
    }
    ids
}

pub fn everyone_applies_to(private: bool, is_thread: bool, channel_type: ChannelType) -> bool {
    !private && !is_thread && channel_type == ChannelType::Text
}

pub fn everyone_exclude_ids(
    kind: EveryoneExcludeType,
    user_ids: &[UserId],
    role_ids: &[RoleId],
) -> Vec<i64> {
    match kind {
        EveryoneExcludeType::User => user_ids
            .iter()
            .copied()
            .map(UserId::get)
            .filter(|id| *id != 0)
            .collect(),
        EveryoneExcludeType::Role => role_ids
            .iter()
            .copied()
            .map(RoleId::get)
            .filter(|id| *id != 0)
            .collect(),
    }
}

pub fn everyone_is_dirty(
    enabled: bool,
    initial_enabled: bool,
    user_ids: &[UserId],
    initial_user_ids: &[UserId],
    role_ids: &[RoleId],
    initial_role_ids: &[RoleId],
) -> bool {
    if enabled != initial_enabled {
        return true;
    }
    if !enabled {
        return false;
    }
    user_ids != initial_user_ids || role_ids != initial_role_ids
}

pub fn everyone_set_calls(
    enabled: bool,
    kind: EveryoneExcludeType,
    user_ids: &[UserId],
    initial_user_ids: &[UserId],
    role_ids: &[RoleId],
    initial_role_ids: &[RoleId],
) -> Vec<(EveryoneExcludeType, Vec<i64>)> {
    if !enabled {
        return vec![(EveryoneExcludeType::User, Vec::new())];
    }
    let mut calls = Vec::new();
    if user_ids != initial_user_ids {
        calls.push((
            EveryoneExcludeType::User,
            everyone_exclude_ids(EveryoneExcludeType::User, user_ids, role_ids),
        ));
    }
    if role_ids != initial_role_ids {
        calls.push((
            EveryoneExcludeType::Role,
            everyone_exclude_ids(EveryoneExcludeType::Role, user_ids, role_ids),
        ));
    }
    if calls.is_empty() {
        calls.push((kind, everyone_exclude_ids(kind, user_ids, role_ids)));
    }
    calls
}

pub fn acl_role_ids(private_enabled: bool, selected: &[RoleId]) -> Vec<i64> {
    if !private_enabled {
        return Vec::new();
    }
    let mut ids: Vec<i64> = Vec::new();
    for role_id in selected {
        if role_id.is_zero() || ids.contains(&role_id.get()) {
            continue;
        }
        ids.push(role_id.get());
    }
    ids
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    RolesAndMembers,
    MembersOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub mode: SearchMode,
    pub needle: String,
}

pub fn parse_search(input: &str) -> SearchQuery {
    let normalized = input.trim().to_lowercase();
    match normalized.strip_prefix('@') {
        Some(rest) => SearchQuery {
            mode: SearchMode::MembersOnly,
            needle: rest.to_string(),
        },
        None => SearchQuery {
            mode: SearchMode::RolesAndMembers,
            needle: normalized,
        },
    }
}

pub fn member_matches(clan_nick: &str, display_name: &str, username: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    clan_nick.to_lowercase().contains(needle)
        || display_name.to_lowercase().contains(needle)
        || username.to_lowercase().contains(needle)
}

pub fn role_matches(title: &str, needle: &str) -> bool {
    title.trim().to_lowercase().contains(needle.trim())
}

pub fn is_system_role(creator_id: UserId) -> bool {
    creator_id.is_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_toggle_is_inverted_at_the_api_boundary() {
        assert_eq!(channel_private_payload(true), 0);
        assert_eq!(channel_private_payload(false), 1);
    }

    #[test]
    fn creator_is_force_merged_when_private() {
        let ids = acl_user_ids(true, &[UserId(2), UserId(3)], Some(UserId(7)));
        assert_eq!(ids, vec![2, 3, 7]);
    }

    #[test]
    fn creator_is_not_duplicated_when_already_selected() {
        let ids = acl_user_ids(true, &[UserId(7), UserId(2), UserId(7)], Some(UserId(7)));
        assert_eq!(ids, vec![7, 2]);
    }

    #[test]
    fn going_public_keeps_only_the_creator() {
        let ids = acl_user_ids(false, &[UserId(2), UserId(3)], Some(UserId(7)));
        assert_eq!(ids, vec![7]);
    }

    #[test]
    fn missing_creator_yields_empty_public_payload() {
        assert!(acl_user_ids(false, &[UserId(2)], None).is_empty());
        assert!(acl_user_ids(false, &[UserId(2)], Some(UserId(0))).is_empty());
    }

    #[test]
    fn roles_are_sent_only_while_private() {
        assert_eq!(
            acl_role_ids(true, &[RoleId(4), RoleId(4), RoleId(5)]),
            vec![4, 5]
        );
        assert!(acl_role_ids(false, &[RoleId(4)]).is_empty());
    }

    #[test]
    fn at_prefix_switches_to_members_only() {
        assert_eq!(
            parse_search("  @Wumpus "),
            SearchQuery {
                mode: SearchMode::MembersOnly,
                needle: "wumpus".to_string(),
            }
        );
        assert_eq!(
            parse_search("Mods"),
            SearchQuery {
                mode: SearchMode::RolesAndMembers,
                needle: "mods".to_string(),
            }
        );
        assert_eq!(
            parse_search("@"),
            SearchQuery {
                mode: SearchMode::MembersOnly,
                needle: String::new(),
            }
        );
    }

    #[test]
    fn member_match_covers_all_three_name_fields() {
        assert!(member_matches("Nick", "Display", "username", "nic"));
        assert!(member_matches("Nick", "Display", "username", "spla"));
        assert!(member_matches("Nick", "Display", "username", "sern"));
        assert!(!member_matches("Nick", "Display", "username", "zzz"));
        assert!(member_matches("", "", "", ""));
    }

    #[test]
    fn system_roles_are_excluded_from_candidates() {
        assert!(is_system_role(UserId(0)));
        assert!(!is_system_role(UserId(12)));
    }

    #[test]
    fn everyone_override_is_only_for_public_text_channels() {
        assert!(everyone_applies_to(false, false, ChannelType::Text));
        assert!(!everyone_applies_to(true, false, ChannelType::Text));
        assert!(!everyone_applies_to(false, true, ChannelType::Text));
        assert!(!everyone_applies_to(false, false, ChannelType::Voice));
        assert!(!everyone_applies_to(false, false, ChannelType::Stream));
        assert!(!everyone_applies_to(false, false, ChannelType::App));
    }

    #[test]
    fn everyone_exclude_ids_follow_the_selected_type() {
        assert_eq!(
            everyone_exclude_ids(
                EveryoneExcludeType::User,
                &[UserId(1), UserId(0), UserId(2)],
                &[RoleId(9)],
            ),
            vec![1, 2]
        );
        assert_eq!(
            everyone_exclude_ids(
                EveryoneExcludeType::Role,
                &[UserId(1)],
                &[RoleId(9), RoleId(0), RoleId(8)],
            ),
            vec![9, 8]
        );
    }

    #[test]
    fn everyone_dirty_ignores_lists_while_the_toggle_is_off() {
        assert!(!everyone_is_dirty(
            false,
            false,
            &[UserId(1)],
            &[],
            &[],
            &[],
        ));
        assert!(everyone_is_dirty(true, false, &[], &[], &[], &[]));
        assert!(everyone_is_dirty(true, true, &[], &[], &[RoleId(1)], &[],));
        assert!(!everyone_is_dirty(
            true,
            true,
            &[UserId(1)],
            &[UserId(1)],
            &[],
            &[],
        ));
    }

    #[test]
    fn everyone_dirty_ignores_tab_kind_when_lists_are_unchanged() {
        assert!(!everyone_is_dirty(
            true,
            true,
            &[UserId(1)],
            &[UserId(1)],
            &[RoleId(8)],
            &[RoleId(8)],
        ));
    }

    #[test]
    fn everyone_set_calls_one_type_when_only_that_list_changed() {
        assert_eq!(
            everyone_set_calls(
                true,
                EveryoneExcludeType::Role,
                &[UserId(1)],
                &[UserId(1), UserId(2)],
                &[RoleId(9)],
                &[RoleId(9)],
            ),
            vec![(EveryoneExcludeType::User, vec![1])]
        );
        assert_eq!(
            everyone_set_calls(
                true,
                EveryoneExcludeType::User,
                &[UserId(1)],
                &[UserId(1)],
                &[RoleId(8)],
                &[RoleId(9), RoleId(8)],
            ),
            vec![(EveryoneExcludeType::Role, vec![8])]
        );
    }

    #[test]
    fn everyone_set_calls_both_types_including_empty_exclude_ids() {
        assert_eq!(
            everyone_set_calls(
                true,
                EveryoneExcludeType::User,
                &[],
                &[UserId(1)],
                &[],
                &[RoleId(9)],
            ),
            vec![
                (EveryoneExcludeType::User, Vec::new()),
                (EveryoneExcludeType::Role, Vec::new()),
            ]
        );
        assert_eq!(
            everyone_set_calls(
                true,
                EveryoneExcludeType::User,
                &[UserId(2)],
                &[UserId(1)],
                &[RoleId(8)],
                &[RoleId(9)],
            ),
            vec![
                (EveryoneExcludeType::User, vec![2]),
                (EveryoneExcludeType::Role, vec![8]),
            ]
        );
    }

    #[test]
    fn everyone_set_calls_keeps_override_on_when_lists_are_unchanged() {
        assert_eq!(
            everyone_set_calls(true, EveryoneExcludeType::User, &[], &[], &[], &[],),
            vec![(EveryoneExcludeType::User, Vec::new())]
        );
        assert_eq!(
            everyone_set_calls(
                false,
                EveryoneExcludeType::Role,
                &[UserId(1)],
                &[UserId(1)],
                &[RoleId(9)],
                &[RoleId(9)],
            ),
            vec![(EveryoneExcludeType::User, Vec::new())]
        );
    }
}
