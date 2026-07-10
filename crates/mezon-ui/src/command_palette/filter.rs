use std::cmp::Ordering;

use super::items::{PaletteItem, PaletteItemKind, normalize_search_string};

pub fn filter_and_sort_indices(items: &[PaletteItem], raw_query: &str) -> Vec<usize> {
    if items.is_empty() {
        return Vec::new();
    }

    if raw_query.is_empty() {
        let mut indices: Vec<usize> = (0..items.len()).collect();
        sort_indices(items, &mut indices, raw_query);
        return indices;
    }

    if raw_query.starts_with('@') {
        let search = normalize_search_string(raw_query.get(1..).unwrap_or_default());
        let mut indices: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.kind == PaletteItemKind::Member && item.matches_search(&search)
            })
            .map(|(index, _)| index)
            .collect();
        sort_indices(items, &mut indices, raw_query);
        return indices;
    }

    let search = if raw_query.starts_with('#') {
        normalize_search_string(raw_query.get(1..).unwrap_or_default())
    } else {
        normalize_search_string(raw_query)
    };

    let mut indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.matches_search(&search))
        .map(|(index, _)| index)
        .collect();

    if raw_query.starts_with('#') {
        indices.retain(|&index| items[index].kind == PaletteItemKind::Channel);
    }

    sort_indices(items, &mut indices, raw_query);
    indices
}

fn sort_indices(items: &[PaletteItem], indices: &mut [usize], raw_query: &str) {
    if raw_query.is_empty() || raw_query.starts_with('@') {
        indices.sort_by(|&left, &right| {
            items[right]
                .last_sent_timestamp
                .cmp(&items[left].last_sent_timestamp)
                .then_with(|| items[left].label.cmp(&items[right].label))
        });
        return;
    }

    if raw_query.starts_with('#') {
        let search = normalize_search_string(raw_query.get(1..).unwrap_or_default());
        indices.sort_by(|&left, &right| compare_items(&items[left], &items[right], &search, false));
        return;
    }

    let search = normalize_search_string(raw_query);
    indices.sort_by(|&left, &right| compare_items(&items[left], &items[right], &search, true));
}

fn compare_items(
    left: &PaletteItem,
    right: &PaletteItem,
    search: &str,
    use_name: bool,
) -> Ordering {
    let left_prioritize = left.filter_prioritize.as_str();
    let right_prioritize = right.filter_prioritize.as_str();

    let left_exact = left_prioritize == search;
    let right_exact = right_prioritize == search;
    if left_exact != right_exact {
        return if left_exact {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    let left_index = left_prioritize.find(search);
    let right_index = right_prioritize.find(search);

    if use_name {
        let left_name = left.filter_name.as_str();
        let right_name = right.filter_name.as_str();

        let left_name_exact = left_name == search;
        let right_name_exact = right_name == search;
        if left_name_exact != right_name_exact {
            return if left_name_exact {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }

        let left_name_index = left_name.find(search);
        let right_name_index = right_name.find(search);

        match (left_index, right_index) {
            (None, None) => return cmp_index(left_name_index, right_name_index),
            (Some(left_at), Some(right_at)) if left_at != right_at => {
                return left_at.cmp(&right_at);
            }
            (None, Some(_)) => return Ordering::Greater,
            (Some(_), None) => return Ordering::Less,
            _ => return cmp_index(left_name_index, right_name_index),
        }
    }

    match (left_index, right_index) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left_at), Some(right_at)) => left_at.cmp(&right_at),
    }
}

fn cmp_index(left: Option<usize>, right: Option<usize>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left_at), Some(right_at)) => left_at.cmp(&right_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_palette::items::{PaletteItemKind, normalize_search_string};
    use gpui::SharedString;

    fn item(
        kind: PaletteItemKind,
        label: &str,
        name: &str,
        display: &str,
        blob: &str,
        last_sent_timestamp: i64,
    ) -> PaletteItem {
        PaletteItem {
            kind,
            label: SharedString::from(label),
            subtext: SharedString::default(),
            avatar: SharedString::default(),
            unread_count: 0,
            last_sent_timestamp,
            last_seen_timestamp: 0,
            channel_id: None,
            clan_id: None,
            user_id: None,
            channel_type: None,
            private: false,
            dm_kind: None,
            dm_channel_type: None,
            filter_prioritize: normalize_search_string(label),
            filter_name: normalize_search_string(name),
            filter_display: normalize_search_string(display),
            filter_blob: normalize_search_string(blob),
        }
    }

    #[test]
    fn empty_query_returns_all_sorted_by_timestamp() {
        let items = vec![
            item(PaletteItemKind::Channel, "general", "general", "", "", 10),
            item(PaletteItemKind::Direct, "alice", "alice", "", "", 30),
        ];
        let indices = filter_and_sort_indices(&items, "");
        assert_eq!(indices, vec![1, 0]);
    }

    #[test]
    fn at_prefix_filters_members_only() {
        let items = vec![
            item(PaletteItemKind::Channel, "general", "general", "", "", 10),
            item(
                PaletteItemKind::Member,
                "Alice",
                "alice",
                "Alice Display",
                "alice",
                0,
            ),
        ];
        let indices = filter_and_sort_indices(&items, "@ali");
        assert_eq!(indices, vec![1]);
    }

    #[test]
    fn hash_prefix_filters_channels_only() {
        let items = vec![
            item(PaletteItemKind::Channel, "general", "general", "", "", 10),
            item(
                PaletteItemKind::Direct,
                "general-dm",
                "general-dm",
                "",
                "",
                20,
            ),
        ];
        let indices = filter_and_sort_indices(&items, "#gen");
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn general_search_matches_blob_fields() {
        let items = vec![item(
            PaletteItemKind::Member,
            "Nick",
            "hidden-user",
            "",
            "secret-clan-nick",
            0,
        )];
        let indices = filter_and_sort_indices(&items, "secret");
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn exact_match_sorts_before_prefix_match() {
        let items = vec![
            item(PaletteItemKind::Channel, "dev", "dev", "", "", 0),
            item(
                PaletteItemKind::Channel,
                "development",
                "development",
                "",
                "",
                0,
            ),
        ];
        let indices = filter_and_sort_indices(&items, "dev");
        assert_eq!(indices, vec![0, 1]);
    }

    #[test]
    fn normalize_string_strips_diacritics_and_uppercases() {
        use crate::command_palette::items::normalize_string;
        assert_eq!(normalize_string("café"), "CAFE");
        assert_eq!(normalize_string("Hà Nội"), "HA NOI");
    }

    #[test]
    fn normalize_search_string_replaces_separators() {
        assert_eq!(
            normalize_search_string("foo-bar_baz+qux"),
            "FOO BAR BAZ QUX"
        );
    }
}
