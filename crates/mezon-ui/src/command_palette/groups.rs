use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px};

use mezon_store::{ChannelId, ChannelType, ClanId, DirectKind};

use crate::theme::Theme;

use super::items::{PaletteItem, PaletteItemKind, ROW_PX};

const GROUP_ITEM_LIMIT: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteDisplayRow {
    SectionHeader(SharedString),
    Item { item_index: usize },
}

pub struct PaletteSectionLabels {
    pub previous: SharedString,
    pub mentions: SharedString,
    pub unread: SharedString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteBrowseContext {
    Direct,
    Clan(ClanId),
}

pub fn build_display_rows(
    items: &[PaletteItem],
    filtered_indices: &[usize],
    raw_query: &str,
    previous_channel_ids: &[ChannelId],
    browse_context: Option<PaletteBrowseContext>,
    labels: &PaletteSectionLabels,
) -> Vec<PaletteDisplayRow> {
    if raw_query.is_empty() {
        build_grouped_rows(
            items,
            filtered_indices,
            previous_channel_ids,
            browse_context,
            labels,
        )
    } else {
        filtered_indices
            .iter()
            .map(|&item_index| PaletteDisplayRow::Item { item_index })
            .collect()
    }
}

fn build_grouped_rows(
    items: &[PaletteItem],
    sorted_indices: &[usize],
    previous_channel_ids: &[ChannelId],
    browse_context: Option<PaletteBrowseContext>,
    labels: &PaletteSectionLabels,
) -> Vec<PaletteDisplayRow> {
    let mut remaining: Vec<usize> = sorted_indices.to_vec();
    let mut recent = Vec::new();
    for channel_id in previous_channel_ids {
        if let Some(pos) = remaining.iter().position(|&ix| {
            items
                .get(ix)
                .and_then(|item| item.channel_id)
                .is_some_and(|id| id == *channel_id)
        }) {
            recent.push(remaining.remove(pos));
        }
    }

    let mut mentions = Vec::new();
    let mut unread = Vec::new();

    for item_index in remaining {
        let Some(item) = items.get(item_index) else {
            continue;
        };
        let Some(context) = browse_context else {
            continue;
        };
        if !item_matches_browse_context(item, context) {
            continue;
        }
        if is_mention_item(item, context) {
            mentions.push(item_index);
        } else if is_unread_list_item(item, context) {
            unread.push(item_index);
        }
    }

    let mut rows = Vec::new();
    if !recent.is_empty() {
        rows.push(PaletteDisplayRow::SectionHeader(labels.previous.clone()));
        rows.extend(
            recent
                .into_iter()
                .map(|item_index| PaletteDisplayRow::Item { item_index }),
        );
    }
    if !mentions.is_empty() {
        rows.push(PaletteDisplayRow::SectionHeader(labels.mentions.clone()));
        rows.extend(
            mentions
                .into_iter()
                .take(GROUP_ITEM_LIMIT)
                .map(|item_index| PaletteDisplayRow::Item { item_index }),
        );
    }
    if !unread.is_empty() {
        rows.push(PaletteDisplayRow::SectionHeader(labels.unread.clone()));
        rows.extend(
            unread
                .into_iter()
                .take(GROUP_ITEM_LIMIT)
                .map(|item_index| PaletteDisplayRow::Item { item_index }),
        );
    }
    rows
}

const DM_GROUP_CHANNEL_TYPE: u32 = 2;
const DM_PEER_CHANNEL_TYPE: u32 = 3;

fn is_dm_tab_conversation(item: &PaletteItem) -> bool {
    match item.kind {
        PaletteItemKind::Direct => matches!(
            item.dm_kind,
            Some(DirectKind::Dm | DirectKind::Group) | None
        ),
        PaletteItemKind::Channel => {
            item.clan_id.is_none_or(|id| id.is_zero())
                && matches!(
                    item.channel_type,
                    Some(ChannelType::Unknown(DM_GROUP_CHANNEL_TYPE))
                        | Some(ChannelType::Unknown(DM_PEER_CHANNEL_TYPE))
                )
        }
        PaletteItemKind::Member => false,
    }
}

fn item_matches_browse_context(item: &PaletteItem, context: PaletteBrowseContext) -> bool {
    match context {
        PaletteBrowseContext::Direct => is_dm_tab_conversation(item),
        PaletteBrowseContext::Clan(clan_id) => {
            item.kind == PaletteItemKind::Channel && item.clan_id == Some(clan_id)
        }
    }
}

fn is_mention_item(item: &PaletteItem, _context: PaletteBrowseContext) -> bool {
    item.unread_count > 0
        && matches!(
            item.channel_type,
            Some(ChannelType::Text) | Some(ChannelType::Thread)
        )
}

fn is_unread_list_item(item: &PaletteItem, context: PaletteBrowseContext) -> bool {
    if !item.is_unread() {
        return false;
    }
    match context {
        PaletteBrowseContext::Direct => is_dm_tab_conversation(item),
        PaletteBrowseContext::Clan(_) => {
            matches!(item.kind, PaletteItemKind::Channel)
                && matches!(
                    item.channel_type,
                    Some(ChannelType::Text) | Some(ChannelType::Thread)
                )
        }
    }
}

pub fn render_section_header(theme: &Theme, label: &SharedString) -> AnyElement {
    div()
        .h(px(ROW_PX))
        .flex()
        .items_center()
        .px(px(10.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.tokens.text_theme_primary_hover)
        .child(label.to_string().to_uppercase())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_palette::items::{PaletteItemKind, normalize_search_string};
    use gpui::SharedString;
    use mezon_store::ClanId;

    fn channel_item(
        id: i64,
        label: &str,
        channel_type: ChannelType,
        unread_count: u32,
        last_sent: i64,
        last_seen: i64,
    ) -> PaletteItem {
        PaletteItem {
            kind: PaletteItemKind::Channel,
            label: SharedString::from(label),
            subtext: SharedString::default(),
            avatar: SharedString::default(),
            unread_count,
            last_sent_timestamp: last_sent,
            last_seen_timestamp: last_seen,
            channel_id: Some(ChannelId(id)),
            clan_id: Some(ClanId(1)),
            user_id: None,
            channel_type: Some(channel_type),
            private: false,
            dm_kind: None,
            dm_channel_type: None,
            filter_prioritize: normalize_search_string(label),
            filter_name: normalize_search_string(label),
            filter_display: String::new(),
            filter_blob: normalize_search_string(label),
        }
    }

    fn labels() -> PaletteSectionLabels {
        PaletteSectionLabels {
            previous: "Previous".into(),
            mentions: "Mentions".into(),
            unread: "Unread".into(),
        }
    }

    #[test]
    fn grouped_view_builds_recent_mentions_and_unread_sections() {
        let items = vec![
            channel_item(1, "recent", ChannelType::Text, 0, 100, 100),
            channel_item(2, "mention", ChannelType::Text, 2, 90, 90),
            channel_item(3, "unread", ChannelType::Text, 0, 80, 10),
        ];
        let sorted = vec![0, 1, 2];
        let previous = vec![ChannelId(1)];
        let rows = build_display_rows(
            &items,
            &sorted,
            "",
            &previous,
            Some(PaletteBrowseContext::Clan(ClanId(1))),
            &labels(),
        );
        assert_eq!(rows.len(), 6);
        assert!(matches!(rows[0], PaletteDisplayRow::SectionHeader(_)));
        assert!(matches!(rows[1], PaletteDisplayRow::Item { item_index: 0 }));
        assert!(matches!(rows[3], PaletteDisplayRow::Item { item_index: 1 }));
        assert!(matches!(rows[5], PaletteDisplayRow::Item { item_index: 2 }));
    }

    #[test]
    fn grouped_view_scopes_mentions_and_unread_to_browse_context() {
        let mut dm = channel_item(10, "dm-unread", ChannelType::Text, 0, 50, 10);
        dm.kind = PaletteItemKind::Direct;
        dm.clan_id = None;
        dm.channel_type = None;
        dm.dm_kind = Some(DirectKind::Dm);
        let items = vec![
            channel_item(1, "clan-mention", ChannelType::Text, 2, 90, 90),
            dm,
        ];
        let sorted = vec![0, 1];
        let clan_rows = build_display_rows(
            &items,
            &sorted,
            "",
            &[],
            Some(PaletteBrowseContext::Clan(ClanId(1))),
            &labels(),
        );
        let mention_items: Vec<_> = clan_rows
            .iter()
            .filter_map(|row| match row {
                PaletteDisplayRow::Item { item_index } => Some(*item_index),
                _ => None,
            })
            .collect();
        assert_eq!(mention_items, vec![0]);

        let dm_rows = build_display_rows(
            &items,
            &sorted,
            "",
            &[],
            Some(PaletteBrowseContext::Direct),
            &labels(),
        );
        let dm_unread: Vec<_> = dm_rows
            .iter()
            .filter_map(|row| match row {
                PaletteDisplayRow::Item { item_index } => Some(*item_index),
                _ => None,
            })
            .collect();
        assert_eq!(dm_unread, vec![1]);
    }

    #[test]
    fn grouped_view_includes_user_channel_group_on_dm_tab() {
        let mut group = channel_item(20, "group-unread", ChannelType::Unknown(2), 1, 60, 10);
        group.clan_id = Some(ClanId(0));
        let items = vec![group];
        let rows = build_display_rows(
            &items,
            &[0],
            "",
            &[],
            Some(PaletteBrowseContext::Direct),
            &labels(),
        );
        let unread: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                PaletteDisplayRow::Item { item_index } => Some(*item_index),
                _ => None,
            })
            .collect();
        assert_eq!(unread, vec![0]);
    }

    #[test]
    fn search_query_uses_flat_rows() {
        let items = vec![channel_item(1, "general", ChannelType::Text, 0, 10, 10)];
        let filtered = vec![0];
        let rows = build_display_rows(&items, &filtered, "gen", &[], None, &labels());
        assert_eq!(rows, vec![PaletteDisplayRow::Item { item_index: 0 }]);
    }
}
