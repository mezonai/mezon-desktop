use std::cmp::Ordering;
use std::collections::HashMap;
use std::iter::Peekable;
use std::str::Chars;

use gpui::App;
use mezon_store::{
    ChannelUserProfile, ClanId, ClanMember, ClanMembersStore, MENTION_SEARCH_MIN_CHARS,
    MentionSearchStore, UserId,
};

use super::permissions_tab::{MemberRow, member_row, member_row_from_clan_member};
use crate::util::text_utils::normalize_search_string;

pub(super) fn searchable(needle: &str) -> bool {
    needle.trim().chars().filter(|ch| !ch.is_control()).count() >= MENTION_SEARCH_MIN_CHARS
}

pub(super) fn search(owner: u64, clan_id: ClanId, needle: &str, cx: &mut App) {
    if !ClanMembersStore::global(cx).read(cx).roster_capped(clan_id) {
        return;
    }
    if let Some(store) = MentionSearchStore::try_global(cx) {
        store.update(cx, |store, cx| {
            store.search(owner, clan_id, None, needle, cx)
        });
    }
}

pub(super) fn end_session(owner: u64, cx: &mut App) {
    if let Some(store) = MentionSearchStore::try_global(cx) {
        store.update(cx, |store, cx| store.end_session(owner, cx));
    }
}

pub(super) fn is_searching(owner: u64, clan_id: ClanId, needle: &str, cx: &App) -> bool {
    MentionSearchStore::try_global(cx)
        .is_some_and(|store| store.read(cx).is_searching(owner, clan_id, None, needle))
}

pub(super) fn answer_seq(clan_id: ClanId, needle: &str, cx: &App) -> Option<u64> {
    if needle.trim().is_empty() {
        return None;
    }
    let store = MentionSearchStore::try_global(cx)?;
    store
        .read(cx)
        .results(clan_id, None, needle)
        .map(|(_, seq, _)| seq)
}

pub(super) fn hits(clan_id: ClanId, needle: &str, cx: &App) -> Vec<ClanMember> {
    if needle.trim().is_empty() {
        return Vec::new();
    }
    let Some(store) = MentionSearchStore::try_global(cx) else {
        return Vec::new();
    };
    let Some((answered, _, members)) = store.read(cx).results(clan_id, None, needle) else {
        return Vec::new();
    };
    let folded = normalize_search_string(needle.trim());
    let answers_needle = normalize_search_string(answered) == folded;
    members
        .iter()
        .filter(|member| answers_needle || folded_match(member, &folded))
        .cloned()
        .collect()
}

pub(super) fn profile(member: &ClanMember) -> ChannelUserProfile {
    ChannelUserProfile {
        username: member.user.username.clone(),
        display_name: member.user.display_name.clone(),
        avatar: member.user.avatar_url.clone(),
    }
}

pub(super) fn picker_row(
    clan_id: ClanId,
    user_id: UserId,
    remote: &HashMap<UserId, ClanMember>,
    cx: &App,
) -> MemberRow {
    let roster = ClanMembersStore::global(cx);
    let roster = roster.read(cx);
    known_row(roster.member(clan_id, user_id), remote.get(&user_id))
        .unwrap_or_else(|| member_row(clan_id, user_id, cx))
}

fn known_row(roster: Option<&ClanMember>, remote: Option<&ClanMember>) -> Option<MemberRow> {
    roster.or(remote).map(member_row_from_clan_member)
}

pub(super) fn names(member: &ClanMember) -> [&str; 3] {
    [
        member.clan_nick.as_str(),
        member.user.display_name.as_str(),
        member.user.username.as_str(),
    ]
}

pub(super) fn rank_by_match<'n, T>(
    rows: &mut [T],
    needle: &str,
    names: impl Fn(&T) -> [&'n str; 3],
) {
    let folded = normalize_search_string(needle.trim());
    if folded.is_empty() {
        return;
    }
    rows.sort_by_cached_key(|row| {
        let names = names(row);
        let rank = match_rank(&names, &folded);
        let shown = names
            .into_iter()
            .find(|name| !name.is_empty())
            .unwrap_or_default();
        (rank, NameKey(normalize_search_string(shown)))
    });
}

fn match_rank(names: &[&str; 3], folded: &str) -> u8 {
    let mut rank = 3;
    for name in names {
        let name = normalize_search_string(name);
        for (at, _) in name.match_indices(folded) {
            if at == 0 {
                return 0;
            }
            let word_start = name[..at]
                .chars()
                .next_back()
                .is_some_and(|ch| !ch.is_alphanumeric());
            rank = rank.min(if word_start { 1 } else { 2 });
        }
    }
    rank
}

#[derive(PartialEq, Eq)]
struct NameKey(String);

impl Ord for NameKey {
    fn cmp(&self, other: &Self) -> Ordering {
        natural_cmp(&self.0, &other.0).then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for NameKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek(), b.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let x = number(&mut a);
                let y = number(&mut b);
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.cmp(y);
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn number(chars: &mut Peekable<Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(ch) = chars.next_if(char::is_ascii_digit) {
        digits.push(ch);
    }
    digits.trim_start_matches('0').to_string()
}

fn folded_match(member: &ClanMember, folded: &str) -> bool {
    [
        member.clan_nick.as_str(),
        member.user.display_name.as_str(),
        member.user.username.as_str(),
    ]
    .into_iter()
    .any(|field| normalize_search_string(field).contains(folded))
}

#[cfg(test)]
mod tests {
    use mezon_store::User;

    use super::*;

    fn member(nick: &str, display_name: &str, username: &str) -> ClanMember {
        ClanMember {
            user: User {
                id: UserId(7),
                username: username.into(),
                display_name: display_name.into(),
                avatar_url: "https://cdn/user.png".into(),
                ..User::default()
            },
            clan_nick: nick.into(),
            clan_avatar: "https://cdn/clan.png".into(),
            ..ClanMember::default()
        }
    }

    #[test]
    fn a_reused_answer_is_narrowed_ignoring_case_and_accents() {
        let folded = normalize_search_string("chau gi");
        assert!(folded_match(&member("", "Châu Giang", "cg"), &folded));
        assert!(folded_match(&member("CHAU GIANG", "", "cg"), &folded));
        assert!(!folded_match(&member("", "Châu Ngọc", "chau"), &folded));
    }

    #[test]
    fn the_stored_profile_matches_what_the_channel_listing_returns() {
        let picked = member("Nick", "Display", "user");
        let stored = profile(&picked);
        assert_eq!(stored.display_name, "Display");
        assert_eq!(stored.username, "user");
        assert_eq!(stored.avatar, "https://cdn/user.png");
        assert_eq!(member_row_from_clan_member(&picked).name.as_ref(), "Nick");
    }

    #[test]
    fn the_roster_wins_over_a_remembered_server_row() {
        let roster = member("Renamed", "Display", "user");
        let remembered = member("Old Nick", "Display", "user");
        let row = known_row(Some(&roster), Some(&remembered)).expect("a row");
        assert_eq!(row.name.as_ref(), "Renamed");
        let row = known_row(None, Some(&remembered)).expect("a row");
        assert_eq!(row.name.as_ref(), "Old Nick");
        assert!(known_row(None, None).is_none());
    }

    fn ranked(rows: &[[&'static str; 3]], needle: &str) -> Vec<&'static str> {
        let mut rows = rows.to_vec();
        rank_by_match(&mut rows, needle, |row| *row);
        rows.iter()
            .map(|row| {
                row.iter()
                    .copied()
                    .find(|name| !name.is_empty())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn names_starting_with_the_search_rank_above_words_then_contained_matches() {
        let rows = [
            ["", "Magic Mike", "mm"],
            ["", "Zed", "zed"],
            ["", "Tran Gi Anh", "tga"],
            ["Someone", "", "giang.tran"],
            ["", "Giàng A Páo", "gap"],
        ];
        assert_eq!(
            ranked(&rows, " Gi "),
            ["Giàng A Páo", "Someone", "Tran Gi Anh", "Magic Mike", "Zed"]
        );
    }

    #[test]
    fn equal_matches_follow_the_shown_name_with_numbers_by_value() {
        let rows = [
            ["", "Old Timer 099", ""],
            ["Old Timer 098", "", ""],
            ["", "Old Timer 090", "oldtimer_090"],
            ["Old Timer 9", "", ""],
            ["Old Timer 10", "", ""],
        ];
        assert_eq!(
            ranked(&rows, "old timer"),
            [
                "Old Timer 9",
                "Old Timer 10",
                "Old Timer 090",
                "Old Timer 098",
                "Old Timer 099"
            ]
        );
        assert_eq!(
            ranked(&rows, "  "),
            [
                "Old Timer 099",
                "Old Timer 098",
                "Old Timer 090",
                "Old Timer 9",
                "Old Timer 10"
            ]
        );
    }

    #[test]
    fn a_username_match_ranks_by_its_separators_as_words() {
        let rows = [
            ["", "Member 0009", "member_0009"],
            ["", "Member 0900", "member_0900"],
        ];
        assert_eq!(ranked(&rows, "09"), ["Member 0900", "Member 0009"]);
        let rows = [
            ["Ostrander", "", ""],
            ["", "", "giang.tran"],
            ["Matran Tran", "", ""],
        ];
        assert_eq!(
            ranked(&rows, "tran"),
            ["giang.tran", "Matran Tran", "Ostrander"]
        );
    }

    #[test]
    fn a_single_character_is_not_searched() {
        assert!(!searchable(""));
        assert!(!searchable(" g "));
        assert!(searchable("gi"));
    }
}
