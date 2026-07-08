use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px};

use mezon_store::{ChannelId, ChannelType, DirectKind};

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

pub fn build_display_rows(
    items: &[PaletteItem],
    filtered_indices: &[usize],
    raw_query: &str,
    previous_channel_ids: &[ChannelId],
    labels: &PaletteSectionLabels,
) -> Vec<PaletteDisplayRow> {
    if raw_query.is_empty() {
        build_grouped_rows(items, filtered_indices, previous_channel_ids, labels)
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
        if is_mention_item(item) {
            mentions.push(item_index);
        } else if is_unread_list_item(item) {
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

fn is_mention_item(item: &PaletteItem) -> bool {
    item.unread_count > 0
        && matches!(
            item.channel_type,
            Some(ChannelType::Text) | Some(ChannelType::Thread)
        )
}

fn is_unread_list_item(item: &PaletteItem) -> bool {
    if !item.is_unread() {
        return false;
    }
    match item.kind {
        PaletteItemKind::Channel => matches!(
            item.channel_type,
            Some(ChannelType::Text) | Some(ChannelType::Thread)
        ),
        PaletteItemKind::Direct => {
            matches!(item.dm_kind, Some(DirectKind::Dm | DirectKind::Group))
        }
        PaletteItemKind::Member => false,
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
    use crate::command_palette::items::{normalize_search_string, PaletteItemKind};
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
        let rows = build_display_rows(&items, &sorted, "", &previous, &labels());
        assert_eq!(rows.len(), 6);
        assert!(matches!(rows[0], PaletteDisplayRow::SectionHeader(_)));
        assert!(matches!(rows[1], PaletteDisplayRow::Item { item_index: 0 }));
        assert!(matches!(rows[3], PaletteDisplayRow::Item { item_index: 1 }));
        assert!(matches!(rows[5], PaletteDisplayRow::Item { item_index: 2 }));
    }

    #[test]
    fn search_query_uses_flat_rows() {
        let items = vec![channel_item(1, "general", ChannelType::Text, 0, 10, 10)];
        let filtered = vec![0];
        let rows = build_display_rows(&items, &filtered, "gen", &[], &labels());
        assert_eq!(rows, vec![PaletteDisplayRow::Item { item_index: 0 }]);
    }
}
