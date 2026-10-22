use mezon_proto::api;

pub const SEARCH_PAGE_SIZE: i32 = 25;

const FILTER_WORD_PREFIXES: &[&str] = &[">", "~", "&", "from:", "mentions:", "mention:", "has:"];

const HAS_FILTER_VALUES: &[&str] = &["image", "video", "link"];

pub fn has_filter_options() -> &'static [&'static str] {
    HAS_FILTER_VALUES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDropdownMode {
    Options,
    FromUser,
    Mentions,
    Has,
    PlainMembers,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSearchTrigger {
    pub kind: char,
    pub needle: String,
}

pub fn search_page_count(total: i32) -> i32 {
    if total <= 0 {
        0
    } else {
        (total + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPageToken {
    Page(i32),
    Ellipsis,
}

pub fn search_page_numbers(current_page: i32, total_pages: i32) -> Vec<SearchPageToken> {
    if total_pages <= 0 {
        return Vec::new();
    }
    if total_pages <= 5 {
        return (1..=total_pages).map(SearchPageToken::Page).collect();
    }
    let mut pages = Vec::new();
    if current_page <= 3 {
        pages.extend((1..=5).map(SearchPageToken::Page));
        if total_pages > 5 {
            pages.push(SearchPageToken::Ellipsis);
            pages.push(SearchPageToken::Page(total_pages));
        }
    } else if current_page >= total_pages - 2 {
        pages.push(SearchPageToken::Page(1));
        pages.push(SearchPageToken::Ellipsis);
        pages.extend(
            ((total_pages - 5)..=total_pages)
                .map(SearchPageToken::Page)
                .collect::<Vec<_>>(),
        );
    } else {
        pages.push(SearchPageToken::Page(1));
        pages.push(SearchPageToken::Ellipsis);
        pages.push(SearchPageToken::Page(current_page - 1));
        pages.push(SearchPageToken::Page(current_page));
        pages.push(SearchPageToken::Page(current_page + 1));
        pages.push(SearchPageToken::Ellipsis);
        pages.push(SearchPageToken::Page(total_pages));
    }
    pages
}

pub fn filter(field_name: impl Into<String>, field_value: impl Into<String>) -> api::FilterParam {
    api::FilterParam {
        field_name: field_name.into(),
        field_value: field_value.into(),
    }
}

pub fn content_filter(query: &str) -> api::FilterParam {
    filter("content", query)
}

pub fn username_filter(username: &str) -> api::FilterParam {
    filter("username", username)
}

pub fn sender_filter_from_id(user_id: &str) -> api::FilterParam {
    filter("sender_id", user_id)
}

pub fn mention_user_filter(user_id: i64) -> api::FilterParam {
    mention_user_filter_from_id(&user_id.to_string())
}

pub fn mention_user_filter_from_id(user_id: &str) -> api::FilterParam {
    filter("mention", format!("\"user_id\":\"{user_id}\""))
}

pub fn has_filter(kind: &str) -> api::FilterParam {
    filter("has", kind)
}

pub fn clan_channel_scope(channel_id: i64, clan_id: i64) -> Vec<api::FilterParam> {
    vec![
        filter("channel_id", channel_id.to_string()),
        filter("clan_id", clan_id.to_string()),
    ]
}

pub fn direct_channel_scope(channel_id: i64) -> Vec<api::FilterParam> {
    vec![
        filter("channel_id", channel_id.to_string()),
        filter("clan_id", "0"),
    ]
}

pub fn build_search_request(
    mut filters: Vec<api::FilterParam>,
    page: i32,
    size: i32,
    sorts: Vec<api::SortParam>,
) -> api::SearchMessageRequest {
    api::SearchMessageRequest {
        filters: std::mem::take(&mut filters),
        from: page,
        size,
        sorts,
    }
}

pub fn build_clan_channel_content_search(
    channel_id: i64,
    clan_id: i64,
    query: &str,
    page: i32,
) -> api::SearchMessageRequest {
    let mut filters = clan_channel_scope(channel_id, clan_id);
    filters.extend(parse_search_query(query));
    build_search_request(filters, page, SEARCH_PAGE_SIZE, Vec::new())
}

pub fn build_direct_content_search(
    channel_id: i64,
    query: &str,
    page: i32,
) -> api::SearchMessageRequest {
    let mut filters = direct_channel_scope(channel_id);
    filters.extend(parse_search_query(query));
    build_search_request(filters, page, SEARCH_PAGE_SIZE, Vec::new())
}

pub fn search_content_highlight_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for filter in parse_search_query(query) {
        if filter.field_name != "content" || filter.field_value.is_empty() {
            continue;
        }
        for word in filter.field_value.split_whitespace() {
            let trimmed = word.trim_matches(|c| c == '"' || c == '\'');
            if trimmed.len() >= 2 {
                terms.push(trimmed.to_string());
            }
        }
    }
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
    terms.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    terms
}

pub fn parse_search_query(query: &str) -> Vec<api::FilterParam> {
    let mut filters = Vec::new();
    let stripped = strip_and_collect_markups(query, &mut filters);
    let mut content_words = Vec::new();
    for word in stripped.split_whitespace() {
        if let Some(filter) = colon_word_filter(word) {
            filters.push(filter);
        } else if let Some(filter) = incomplete_prefix_word_filter(word) {
            filters.push(filter);
        } else if word_starts_filter_prefix(word) {
            continue;
        } else {
            content_words.push(word);
        }
    }
    let content = content_words.join(" ");
    if !content.is_empty() {
        filters.push(content_filter(&content));
    }
    filters
}

pub fn query_has_filter_tokens(query: &str) -> bool {
    if contains_complete_markup(query) {
        return true;
    }
    query.split_whitespace().any(word_starts_filter_prefix)
}

pub fn search_dropdown_mode(query: &str) -> SearchDropdownMode {
    if let Some(trigger) = active_search_trigger(query) {
        return match trigger.kind {
            '>' => SearchDropdownMode::FromUser,
            '~' => SearchDropdownMode::Mentions,
            '&' => SearchDropdownMode::Has,
            _ => SearchDropdownMode::Hidden,
        };
    }
    if query_has_filter_tokens(query) {
        SearchDropdownMode::Hidden
    } else if query.trim().is_empty() {
        SearchDropdownMode::Options
    } else {
        SearchDropdownMode::PlainMembers
    }
}

pub fn should_show_search_dropdown(query: &str) -> bool {
    !matches!(search_dropdown_mode(query), SearchDropdownMode::Hidden)
}

pub fn active_search_trigger(query: &str) -> Option<ActiveSearchTrigger> {
    let stripped = strip_and_collect_markups(query, &mut Vec::new());
    let trimmed = stripped.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let word_start = trimmed.rfind(' ').map(|index| index + 1).unwrap_or(0);
    let last_word = &trimmed[word_start..];
    if query_ends_with_whitespace(query) && is_complete_colon_filter(last_word) {
        return None;
    }
    trigger_from_word(last_word)
}

fn query_ends_with_whitespace(query: &str) -> bool {
    query.chars().next_back().is_some_and(|c| c.is_whitespace())
}

fn is_complete_colon_filter(word: &str) -> bool {
    if let Some(value) = word.strip_prefix("from:") {
        return !value.is_empty();
    }
    if let Some(value) = word.strip_prefix("has:") {
        return !value.is_empty();
    }
    if let Some(value) = word
        .strip_prefix("mentions:")
        .or_else(|| word.strip_prefix("mention:"))
    {
        return !value.is_empty();
    }
    false
}

pub fn insert_filter_markup(current: &str, trigger: char, display: &str, id: &str) -> String {
    let token = match trigger {
        '>' => format!("from:{display} "),
        '&' => format!("has:{id} "),
        '~' => format!("mentions:{display} "),
        _ => format!("{trigger}[{display}]({id}) "),
    };
    if let Some(next) = replace_incomplete_filter_token(current, trigger, &token) {
        return next;
    }
    if let Some(next) = replace_plain_autocomplete_needle(current, display, &token) {
        return next;
    }
    if current.is_empty() {
        token
    } else if current.ends_with(' ') {
        if query_has_filter_tokens(current) {
            current.to_string()
        } else {
            format!("{current}{token}")
        }
    } else {
        format!("{current} {token}")
    }
}

pub fn finalize_incomplete_filter_token(query: &str) -> Option<String> {
    let trimmed_end = query.trim_end();
    if trimmed_end.is_empty() {
        return None;
    }
    let word_start = trimmed_end.rfind(' ').map(|index| index + 1).unwrap_or(0);
    let last_word = &trimmed_end[word_start..];
    let prefix = &trimmed_end[..word_start];
    if last_word.is_empty() {
        return None;
    }
    if is_complete_markup_token(last_word) && !query_ends_with_whitespace(query) {
        return Some(format!("{trimmed_end} "));
    }
    if let Some(name) = last_word.strip_prefix('>')
        && !name.is_empty()
    {
        return Some(format!("{prefix}from:{name} "));
    }
    if let Some(name) = last_word.strip_prefix('~')
        && !name.is_empty()
    {
        return Some(format!("{prefix}mentions:{name} "));
    }
    if let Some(value) = last_word.strip_prefix('&')
        && !value.is_empty()
    {
        return Some(format!("{prefix}has:{value} "));
    }
    if is_complete_colon_filter(last_word) && !query_ends_with_whitespace(query) {
        return Some(format!("{trimmed_end} "));
    }
    None
}

pub fn search_filter_chip_ranges(query: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut byte_index = 0usize;
    for part in query.split_inclusive(char::is_whitespace) {
        let token = part.trim_end_matches(char::is_whitespace);
        let token_start = byte_index;
        let token_end = token_start + token.len();
        if !token.is_empty() && is_chip_filter_token(token) {
            ranges.push(token_start..token_end);
        }
        byte_index += part.len();
    }
    ranges
}

fn is_chip_filter_token(token: &str) -> bool {
    is_complete_markup_token(token) || is_complete_colon_filter(token)
}

fn replace_plain_autocomplete_needle(current: &str, display: &str, token: &str) -> Option<String> {
    if active_search_trigger(current).is_some() || query_has_filter_tokens(current) {
        return None;
    }
    let trimmed_end = current.trim_end();
    if trimmed_end.is_empty() {
        return None;
    }
    if trimmed_end.eq_ignore_ascii_case(display) || !trimmed_end.contains(' ') {
        return Some(token.to_string());
    }
    let word_start = trimmed_end.rfind(' ').map(|index| index + 1).unwrap_or(0);
    let last_word = &trimmed_end[word_start..];
    let prefix = &trimmed_end[..word_start];
    if last_word.eq_ignore_ascii_case(display) {
        return Some(format!("{prefix}{token}"));
    }
    None
}

fn replace_incomplete_filter_token(current: &str, trigger: char, token: &str) -> Option<String> {
    let trimmed_end = current.trim_end();
    let word_start = trimmed_end.rfind(' ').map(|index| index + 1).unwrap_or(0);
    let last_word = &trimmed_end[word_start..];
    let prefix = &trimmed_end[..word_start];
    if query_ends_with_whitespace(current)
        && (is_complete_colon_filter(last_word) || is_complete_markup_token(last_word))
    {
        return None;
    }
    match trigger {
        '>' => {
            if last_word.starts_with('>') && !is_complete_markup_token(last_word) {
                return Some(format!("{prefix}{token}"));
            }
            if last_word.starts_with("from:") {
                return Some(format!("{prefix}{token}"));
            }
        }
        '&' => {
            if last_word.starts_with('&') && !is_complete_markup_token(last_word) {
                return Some(format!("{prefix}{token}"));
            }
            if last_word.starts_with("has:") && !is_complete_colon_filter(last_word) {
                return Some(format!("{prefix}{token}"));
            }
        }
        '~' => {
            if last_word.starts_with('~') && !is_complete_markup_token(last_word) {
                return Some(format!("{prefix}{token}"));
            }
            if (last_word.starts_with("mentions:") || last_word.starts_with("mention:"))
                && !is_complete_colon_filter(last_word)
            {
                return Some(format!("{prefix}{token}"));
            }
        }
        _ => {}
    }
    None
}

pub fn autocomplete_needle(query: &str) -> String {
    active_search_trigger(query)
        .map(|trigger| trigger.needle)
        .unwrap_or_else(|| query.trim().to_string())
}

fn strip_and_collect_markups(query: &str, filters: &mut Vec<api::FilterParam>) -> String {
    let chars: Vec<char> = query.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        if matches!(chars[index], '>' | '~' | '&')
            && let Some((end, trigger, display, id)) = try_parse_markup(&chars, index)
        {
            filters.push(markup_to_filter(trigger, &display, &id));
            output.push(' ');
            index = end;
            continue;
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

fn try_parse_markup(chars: &[char], start: usize) -> Option<(usize, char, String, String)> {
    let trigger = chars[start];
    if chars.get(start + 1) != Some(&'[') {
        return None;
    }
    let mut index = start + 2;
    let display_start = index;
    while index < chars.len() && chars[index] != ']' {
        index += 1;
    }
    if index >= chars.len() || chars[index] != ']' {
        return None;
    }
    let display: String = chars[display_start..index].iter().collect();
    index += 1;
    if chars.get(index) != Some(&'(') {
        return None;
    }
    index += 1;
    let id_start = index;
    while index < chars.len() && chars[index] != ')' {
        index += 1;
    }
    if index >= chars.len() || chars[index] != ')' {
        return None;
    }
    let id: String = chars[id_start..index].iter().collect();
    Some((index + 1, trigger, display, id))
}

fn markup_to_filter(trigger: char, display: &str, id: &str) -> api::FilterParam {
    match trigger {
        '>' if is_user_id(id) => sender_filter_from_id(id),
        '>' => username_filter(display),
        '~' => mention_user_filter_from_id(id),
        '&' => has_filter(id),
        _ => content_filter(display),
    }
}

fn is_user_id(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_digit())
}

fn colon_word_filter(word: &str) -> Option<api::FilterParam> {
    if let Some(value) = word.strip_prefix("from:") {
        return (!value.is_empty()).then(|| username_filter(value));
    }
    if let Some(value) = word.strip_prefix("has:") {
        return (!value.is_empty()).then(|| has_filter(value));
    }
    if let Some(value) = word
        .strip_prefix("mentions:")
        .or_else(|| word.strip_prefix("mention:"))
    {
        return is_user_id(value).then(|| mention_user_filter_from_id(value));
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedSearchUser {
    pub trigger: char,
    pub display: String,
    pub user_id: String,
}

struct UserToken {
    trigger: char,
    name: String,
    raw: String,
    user_id: Option<String>,
}

impl UserToken {
    fn into_text(self, every_sender_resolved: bool) -> String {
        match self.user_id {
            Some(user_id) if self.trigger != '>' || every_sender_resolved => {
                let display: String = self
                    .name
                    .chars()
                    .filter(|c| !matches!(c, '[' | ']' | '(' | ')'))
                    .collect();
                format!("{}[{display}]({user_id})", self.trigger)
            }
            _ => self.raw,
        }
    }
}

enum QueryPiece {
    Word(String),
    User(UserToken),
}

pub fn expand_user_name_tokens(
    query: &str,
    picked: &[PickedSearchUser],
    resolve_user_id: impl Fn(&str) -> Option<String>,
) -> String {
    let mut pieces = Vec::new();
    let mut rest_start = 0;
    for (range, pick) in picked_user_spans(query, picked) {
        push_typed_pieces(
            &query[rest_start..range.start],
            &resolve_user_id,
            &mut pieces,
        );
        pieces.push(QueryPiece::User(UserToken {
            trigger: pick.trigger,
            name: pick.display.clone(),
            raw: query[range.clone()].to_string(),
            user_id: Some(pick.user_id.clone()),
        }));
        rest_start = range.end;
    }
    push_typed_pieces(&query[rest_start..], &resolve_user_id, &mut pieces);
    let every_sender_resolved = !pieces.iter().any(|piece| {
        matches!(piece, QueryPiece::User(token) if token.trigger == '>' && token.user_id.is_none())
    });
    let parts: Vec<String> = pieces
        .into_iter()
        .map(|piece| match piece {
            QueryPiece::Word(word) => word,
            QueryPiece::User(token) => token.into_text(every_sender_resolved),
        })
        .collect();
    let mut expanded = parts.join(" ");
    if query_ends_with_whitespace(query) {
        expanded.push(' ');
    }
    expanded
}

pub fn retain_picked_search_users(picked: &mut Vec<PickedSearchUser>, query: &str) {
    picked.retain(|pick| {
        picked_user_prefix(pick.trigger)
            .is_some_and(|prefix| query.contains(&format!("{prefix}{}", pick.display)))
    });
}

fn picked_user_spans<'a>(
    query: &str,
    picked: &'a [PickedSearchUser],
) -> Vec<(std::ops::Range<usize>, &'a PickedSearchUser)> {
    let mut longest_first: Vec<&PickedSearchUser> = picked
        .iter()
        .filter(|pick| !pick.display.is_empty() && is_user_id(&pick.user_id))
        .collect();
    longest_first.sort_by_key(|pick| std::cmp::Reverse(pick.display.len()));
    let mut spans: Vec<(std::ops::Range<usize>, &PickedSearchUser)> = Vec::new();
    for pick in longest_first {
        let Some(prefix) = picked_user_prefix(pick.trigger) else {
            continue;
        };
        let token = format!("{prefix}{}", pick.display);
        for (start, matched) in query.match_indices(&token) {
            let end = start + matched.len();
            let at_word_start = query[..start]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
            let at_word_end = query[end..].chars().next().is_none_or(char::is_whitespace);
            let overlaps = spans
                .iter()
                .any(|(range, _)| start < range.end && range.start < end);
            if at_word_start && at_word_end && !overlaps {
                spans.push((start..end, pick));
            }
        }
    }
    spans.sort_by_key(|(range, _)| range.start);
    spans
}

fn picked_user_prefix(trigger: char) -> Option<&'static str> {
    match trigger {
        '>' => Some("from:"),
        '~' => Some("mentions:"),
        _ => None,
    }
}

fn push_typed_pieces(
    text: &str,
    resolve_user_id: &impl Fn(&str) -> Option<String>,
    pieces: &mut Vec<QueryPiece>,
) {
    for word in text.split_whitespace() {
        let piece = match user_name_token(word) {
            Some((trigger, name)) => QueryPiece::User(UserToken {
                trigger,
                name: name.to_string(),
                raw: word.to_string(),
                user_id: resolve_typed_user_id(trigger, name, resolve_user_id),
            }),
            None => QueryPiece::Word(word.to_string()),
        };
        pieces.push(piece);
    }
}

fn resolve_typed_user_id(
    trigger: char,
    name: &str,
    resolve_user_id: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    resolve_user_id(name)
        .filter(|id| is_user_id(id))
        .or_else(|| (trigger == '>' && is_user_id(name)).then(|| name.to_string()))
}

fn user_name_token(word: &str) -> Option<(char, &str)> {
    if let Some(name) = word.strip_prefix("from:") {
        return (!name.is_empty()).then_some(('>', name));
    }
    if let Some(name) = word.strip_prefix('>') {
        return (!name.is_empty() && !is_complete_markup_token(word)).then_some(('>', name));
    }
    let name = word
        .strip_prefix("mentions:")
        .or_else(|| word.strip_prefix("mention:"))?;
    (!name.is_empty() && !is_user_id(name)).then_some(('~', name))
}

fn incomplete_prefix_word_filter(word: &str) -> Option<api::FilterParam> {
    if is_complete_markup_token(word) {
        return None;
    }
    if let Some(value) = word.strip_prefix('>') {
        return (!value.is_empty()).then(|| username_filter(value));
    }
    if let Some(value) = word.strip_prefix('&') {
        return (!value.is_empty()).then(|| has_filter(value));
    }
    None
}

fn word_starts_filter_prefix(word: &str) -> bool {
    FILTER_WORD_PREFIXES
        .iter()
        .any(|prefix| word.starts_with(prefix))
}

fn contains_complete_markup(query: &str) -> bool {
    let chars: Vec<char> = query.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if matches!(chars[index], '>' | '~' | '&') && try_parse_markup(&chars, index).is_some() {
            return true;
        }
        index += 1;
    }
    false
}

fn trigger_from_word(word: &str) -> Option<ActiveSearchTrigger> {
    if is_complete_markup_token(word) {
        return None;
    }
    if let Some(needle) = word.strip_prefix('>') {
        return Some(ActiveSearchTrigger {
            kind: '>',
            needle: needle.to_string(),
        });
    }
    if let Some(needle) = word.strip_prefix('~') {
        return Some(ActiveSearchTrigger {
            kind: '~',
            needle: needle.to_string(),
        });
    }
    if let Some(needle) = word.strip_prefix('&') {
        return Some(ActiveSearchTrigger {
            kind: '&',
            needle: needle.to_string(),
        });
    }
    if let Some(needle) = word.strip_prefix("from:") {
        return Some(ActiveSearchTrigger {
            kind: '>',
            needle: needle.to_string(),
        });
    }
    if let Some(needle) = word
        .strip_prefix("mentions:")
        .or_else(|| word.strip_prefix("mention:"))
    {
        return Some(ActiveSearchTrigger {
            kind: '~',
            needle: needle.to_string(),
        });
    }
    if let Some(needle) = word.strip_prefix("has:") {
        return Some(ActiveSearchTrigger {
            kind: '&',
            needle: needle.to_string(),
        });
    }
    None
}

fn is_complete_markup_token(token: &str) -> bool {
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() || !matches!(chars[0], '>' | '~' | '&') {
        return false;
    }
    try_parse_markup(&chars, 0).is_some_and(|(end, _, _, _)| end == chars.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn search_request_encodes_filters_pagination_and_sorts() {
        let req = build_search_request(
            vec![
                filter("channel_id", "100"),
                filter("clan_id", "200"),
                content_filter("hello world"),
            ],
            2,
            SEARCH_PAGE_SIZE,
            vec![api::SortParam {
                field_name: "create_time".into(),
                order: "DESC".into(),
            }],
        );
        let encoded = req.encode_to_vec();
        let decoded = api::SearchMessageRequest::decode(encoded.as_slice()).expect("decode");
        assert_eq!(decoded.from, 2);
        assert_eq!(decoded.size, SEARCH_PAGE_SIZE);
        assert_eq!(decoded.filters.len(), 3);
        assert_eq!(decoded.filters[0].field_name, "channel_id");
        assert_eq!(decoded.filters[0].field_value, "100");
        assert_eq!(decoded.filters[1].field_name, "clan_id");
        assert_eq!(decoded.filters[2].field_value, "hello world");
        assert_eq!(decoded.sorts.len(), 1);
        assert_eq!(decoded.sorts[0].field_name, "create_time");
    }

    #[test]
    fn clan_channel_content_search_builder_matches_server_contract() {
        let req = build_clan_channel_content_search(42, 7, "test", 1);
        assert_eq!(req.from, 1);
        assert_eq!(req.size, SEARCH_PAGE_SIZE);
        assert_eq!(req.filters.len(), 3);
        assert_eq!(req.filters[0].field_value, "42");
        assert_eq!(req.filters[1].field_value, "7");
        assert_eq!(req.filters[2].field_name, "content");
    }

    #[test]
    fn direct_content_search_uses_clan_id_zero() {
        let req = build_direct_content_search(99, "dm query", 1);
        assert_eq!(req.filters.len(), 3);
        assert_eq!(req.filters[0].field_value, "99");
        assert_eq!(req.filters[1].field_name, "clan_id");
        assert_eq!(req.filters[1].field_value, "0");
    }

    #[test]
    fn mention_filter_matches_react_format() {
        let f = mention_user_filter(12345);
        assert_eq!(f.field_name, "mention");
        assert_eq!(f.field_value, "\"user_id\":\"12345\"");
    }

    #[test]
    fn parse_search_query_extracts_content_and_markup_filters() {
        let filters = parse_search_query("hello >[alice](42) world");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].field_name, "sender_id");
        assert_eq!(filters[0].field_value, "42");
        assert_eq!(filters[1].field_name, "content");
        assert_eq!(filters[1].field_value, "hello world");
    }

    #[test]
    fn parse_search_query_maps_mention_and_has_markups() {
        let filters = parse_search_query("~[bob](99) &[image](image)");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].field_name, "mention");
        assert_eq!(filters[0].field_value, "\"user_id\":\"99\"");
        assert_eq!(filters[1].field_name, "has");
        assert_eq!(filters[1].field_value, "image");
    }

    #[test]
    fn parse_search_query_supports_colon_prefix_tokens() {
        let filters = parse_search_query("from:alice has:video notes");
        assert_eq!(filters.len(), 3);
        assert_eq!(filters[0].field_name, "username");
        assert_eq!(filters[0].field_value, "alice");
        assert_eq!(filters[1].field_name, "has");
        assert_eq!(filters[1].field_value, "video");
        assert_eq!(filters[2].field_name, "content");
        assert_eq!(filters[2].field_value, "notes");
    }

    #[test]
    fn query_has_filter_tokens_ignores_embedded_greater_than() {
        assert!(!query_has_filter_tokens("a>b"));
        assert!(query_has_filter_tokens(">alice"));
        assert!(query_has_filter_tokens("text >[alice](1)"));
    }

    #[test]
    fn active_search_trigger_detects_incomplete_from_prefix() {
        let trigger = active_search_trigger("notes >ali").expect("trigger");
        assert_eq!(trigger.kind, '>');
        assert_eq!(trigger.needle, "ali");
    }

    #[test]
    fn active_search_trigger_none_for_complete_markup() {
        assert!(active_search_trigger(">[alice](1)").is_none());
    }

    #[test]
    fn search_dropdown_mode_hides_when_filters_complete() {
        assert_eq!(
            search_dropdown_mode(">[alice](1) hello"),
            SearchDropdownMode::Hidden
        );
        assert_eq!(search_dropdown_mode(">ali"), SearchDropdownMode::FromUser);
        assert_eq!(search_dropdown_mode(""), SearchDropdownMode::Options);
        assert_eq!(
            search_dropdown_mode("plain"),
            SearchDropdownMode::PlainMembers
        );
    }

    #[test]
    fn insert_filter_markup_replaces_incomplete_trigger() {
        let next = insert_filter_markup("notes >ali", '>', "alice", "42");
        assert_eq!(next, "notes from:alice ");
    }

    #[test]
    fn insert_filter_markup_replaces_bare_from_trigger() {
        let next = insert_filter_markup(">", '>', "alice", "42");
        assert_eq!(next, "from:alice ");
    }

    #[test]
    fn insert_filter_markup_replaces_bare_mention_trigger() {
        let next = insert_filter_markup("~", '~', "bob", "99");
        assert_eq!(next, "mentions:bob ");
    }

    #[test]
    fn insert_filter_markup_replaces_incomplete_mention_trigger() {
        let next = insert_filter_markup("~bo", '~', "bob", "99");
        assert_eq!(next, "mentions:bob ");
    }

    #[test]
    fn complete_mentions_colon_hides_dropdown() {
        assert_eq!(
            search_dropdown_mode("mentions:bob "),
            SearchDropdownMode::Hidden
        );
    }

    fn pick(trigger: char, display: &str, user_id: &str) -> PickedSearchUser {
        PickedSearchUser {
            trigger,
            display: display.to_string(),
            user_id: user_id.to_string(),
        }
    }

    fn filter_pairs(query: &str) -> Vec<(String, String)> {
        parse_search_query(query)
            .into_iter()
            .map(|f| (f.field_name, f.field_value))
            .collect()
    }

    fn pair(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn expand_user_name_tokens_rewrites_to_markup() {
        let expanded = expand_user_name_tokens("mentions:bob hello", &[], |name| {
            (name == "bob").then(|| "99".to_string())
        });
        assert_eq!(expanded, "~[bob](99) hello");
    }

    #[test]
    fn renamed_sender_is_searched_by_id_not_by_stored_username() {
        let expanded = expand_user_name_tokens("from:carol.renamed hello", &[], |name| {
            (name == "carol.renamed").then(|| "77".to_string())
        });
        assert_eq!(expanded, ">[carol.renamed](77) hello");
        assert_eq!(
            filter_pairs(&expanded),
            vec![pair("sender_id", "77"), pair("content", "hello")]
        );
    }

    #[test]
    fn unresolved_sender_falls_back_to_username_filter() {
        let expanded = expand_user_name_tokens("from:ghost", &[], |_| None);
        assert_eq!(expanded, "from:ghost");
        assert_eq!(filter_pairs(&expanded), vec![pair("username", "ghost")]);
    }

    #[test]
    fn senders_stay_usernames_when_one_of_them_does_not_resolve() {
        let expanded = expand_user_name_tokens("from:alice from:oldname", &[], |name| {
            (name == "alice").then(|| "1".to_string())
        });
        assert_eq!(expanded, "from:alice from:oldname");
        assert_eq!(
            filter_pairs(&expanded),
            vec![pair("username", "alice"), pair("username", "oldname")]
        );
    }

    #[test]
    fn picked_multi_word_name_keeps_its_user_id() {
        let picked = [pick('>', "Van Gia", "42")];
        let expanded = expand_user_name_tokens("from:Van Gia hello", &picked, |name| {
            (name == "Van").then(|| "7".to_string())
        });
        assert_eq!(expanded, ">[Van Gia](42) hello");
        assert_eq!(
            filter_pairs(&expanded),
            vec![pair("sender_id", "42"), pair("content", "hello")]
        );
    }

    #[test]
    fn picked_name_with_brackets_still_parses() {
        let picked = [pick('>', "[QA] Linh", "5")];
        let expanded = expand_user_name_tokens("from:[QA] Linh ", &picked, |_| None);
        assert_eq!(filter_pairs(&expanded), vec![pair("sender_id", "5")]);
    }

    #[test]
    fn picked_sender_and_mention_with_the_same_name_keep_their_own_ids() {
        let picked = [pick('>', "Linh", "1"), pick('~', "Linh", "2")];
        let expanded =
            expand_user_name_tokens("from:Linh mentions:Linh ", &picked, |_| Some("9".into()));
        assert_eq!(
            filter_pairs(&expanded),
            vec![pair("sender_id", "1"), pair("mention", "\"user_id\":\"2\"")]
        );
    }

    #[test]
    fn typed_name_matching_part_of_a_picked_name_is_not_rewritten() {
        let picked = [pick('>', "Van Gia", "42")];
        let expanded = expand_user_name_tokens("from:Van hello", &picked, |_| None);
        assert_eq!(expanded, "from:Van hello");
    }

    #[test]
    fn picked_users_are_dropped_once_their_chip_leaves_the_query() {
        let mut picked = vec![pick('>', "Bob", "1"), pick('~', "Linh", "2")];
        retain_picked_search_users(&mut picked, "mentions:Linh hello");
        assert_eq!(picked, vec![pick('~', "Linh", "2")]);
        retain_picked_search_users(&mut picked, "");
        assert!(picked.is_empty());
    }

    #[test]
    fn greater_than_shorthand_before_other_words_resolves_to_id() {
        let expanded = expand_user_name_tokens(">alice hello", &[], |name| {
            (name == "alice").then(|| "3".to_string())
        });
        assert_eq!(
            filter_pairs(&expanded),
            vec![pair("sender_id", "3"), pair("content", "hello")]
        );
    }

    #[test]
    fn numeric_sender_without_member_match_is_used_as_id() {
        let expanded = expand_user_name_tokens("from:12345", &[], |_| None);
        assert_eq!(filter_pairs(&expanded), vec![pair("sender_id", "12345")]);
    }

    #[test]
    fn sender_markup_without_numeric_id_keeps_username_filter() {
        let filters = parse_search_query(">[alice](alice)");
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].field_name, "username");
        assert_eq!(filters[0].field_value, "alice");
    }

    #[test]
    fn insert_filter_markup_replaces_incomplete_from_colon() {
        let next = insert_filter_markup("notes from:ali", '>', "alice", "42");
        assert_eq!(next, "notes from:alice ");
    }

    #[test]
    fn insert_filter_markup_replaces_plain_text_needle() {
        let next = insert_filter_markup("vangiachu", '>', "gia.chuvan", "42");
        assert_eq!(next, "from:gia.chuvan ");
    }

    #[test]
    fn insert_filter_markup_replaces_partial_plain_text_needle() {
        let next = insert_filter_markup("van", '>', "gia.chuvan", "42");
        assert_eq!(next, "from:gia.chuvan ");
    }

    #[test]
    fn insert_filter_markup_replaces_multi_word_display() {
        let next = insert_filter_markup("Van Gia", '>', "Van Gia", "42");
        assert_eq!(next, "from:Van Gia ");
    }

    #[test]
    fn insert_filter_markup_replaces_last_word_in_multi_word_plain_query() {
        let next = insert_filter_markup("notes vangiachu", '>', "vangiachu", "42");
        assert_eq!(next, "notes from:vangiachu ");
    }

    #[test]
    fn insert_filter_markup_mention_uses_display_colon() {
        let next = insert_filter_markup("~van", '~', "gia.chuvan", "42");
        assert_eq!(next, "mentions:gia.chuvan ");
    }

    #[test]
    fn plain_autocomplete_appends_when_filters_already_present() {
        let next = insert_filter_markup("from:alice bob", '>', "bob", "9");
        assert_eq!(next, "from:alice bob from:bob ");
    }

    #[test]
    fn incomplete_from_colon_without_user_id_keeps_colon_token() {
        let next = insert_filter_markup("notes >ali", '>', "alice", "");
        assert_eq!(next, "notes from:alice ");
    }

    #[test]
    fn incomplete_from_colon_uses_display_name() {
        let next = insert_filter_markup("from:ali", '>', "gia.chuvan", "42");
        assert_eq!(next, "from:gia.chuvan ");
    }

    #[test]
    fn incomplete_has_trigger_finalizes_to_colon_token() {
        let next = insert_filter_markup("&im", '&', "image", "image");
        assert_eq!(next, "has:image ");
    }

    #[test]
    fn insert_filter_markup_does_not_corrupt_trailing_space() {
        let next = insert_filter_markup("from:alice ", '>', "alice", "42");
        assert_eq!(next, "from:alice ");
    }

    #[test]
    fn insert_filter_markup_noop_for_completed_from_chip() {
        let next = insert_filter_markup("from:alice ", '>', "bob", "9");
        assert_eq!(next, "from:alice ");
    }

    #[test]
    fn insert_filter_markup_uses_has_colon_for_attachment_filter() {
        let next = insert_filter_markup("notes &im", '&', "image", "image");
        assert_eq!(next, "notes has:image ");
    }

    #[test]
    fn finalize_incomplete_from_trigger_adds_colon_and_space() {
        assert_eq!(
            finalize_incomplete_filter_token(">gia.chuvan"),
            Some("from:gia.chuvan ".to_string())
        );
        assert_eq!(
            finalize_incomplete_filter_token("notes >gia.chuvan"),
            Some("notes from:gia.chuvan ".to_string())
        );
        assert_eq!(finalize_incomplete_filter_token(">"), None);
        assert_eq!(finalize_incomplete_filter_token("from:gia.chuvan "), None);
        assert_eq!(
            finalize_incomplete_filter_token("from:gia.chuvan"),
            Some("from:gia.chuvan ".to_string())
        );
    }

    #[test]
    fn finalize_incomplete_mentions_and_has_triggers() {
        assert_eq!(
            finalize_incomplete_filter_token("~bob"),
            Some("mentions:bob ".to_string())
        );
        assert_eq!(
            finalize_incomplete_filter_token("&image"),
            Some("has:image ".to_string())
        );
    }

    #[test]
    fn search_filter_chip_ranges_cover_completed_tokens() {
        let ranges = search_filter_chip_ranges("from:gia.chuvan hello has:image ");
        assert_eq!(ranges, vec![0..15, 22..31]);
        assert!(search_filter_chip_ranges(">gia.chuvan").is_empty());
        assert!(search_filter_chip_ranges("from:").is_empty());
    }

    #[test]
    fn complete_from_colon_hides_dropdown() {
        assert_eq!(
            search_dropdown_mode("from:alice "),
            SearchDropdownMode::Hidden
        );
    }

    #[test]
    fn complete_from_markup_hides_dropdown() {
        assert_eq!(
            search_dropdown_mode(">[alice](42) "),
            SearchDropdownMode::Hidden
        );
    }

    #[test]
    fn incomplete_from_colon_keeps_from_user_dropdown() {
        assert_eq!(
            search_dropdown_mode("from:ali"),
            SearchDropdownMode::FromUser
        );
    }

    #[test]
    fn parse_search_query_does_not_treat_mentions_colon_username_as_from() {
        let filters = parse_search_query("mentions:gia.chuvan");
        assert!(filters.is_empty());
    }

    #[test]
    fn parse_search_query_mentions_colon_numeric_id() {
        let filters = parse_search_query("mentions:99");
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].field_name, "mention");
        assert_eq!(filters[0].field_value, "\"user_id\":\"99\"");
    }

    #[test]
    fn search_page_count_uses_div_ceil() {
        assert_eq!(search_page_count(0), 0);
        assert_eq!(search_page_count(25), 1);
        assert_eq!(search_page_count(26), 2);
        assert_eq!(search_page_count(30), 2);
    }

    #[test]
    fn search_page_numbers_collapses_long_ranges() {
        let pages = search_page_numbers(4, 10);
        assert_eq!(pages.len(), 7);
        assert_eq!(pages[0], SearchPageToken::Page(1));
        assert_eq!(pages[4], SearchPageToken::Page(5));
        assert_eq!(pages[6], SearchPageToken::Page(10));
    }

    #[test]
    fn parse_incomplete_from_prefix_as_username_filter() {
        let filters = parse_search_query(">gia.chuvan");
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].field_name, "username");
        assert_eq!(filters[0].field_value, "gia.chuvan");
    }

    #[test]
    fn parse_incomplete_from_prefix_with_content_remainder() {
        let filters = parse_search_query(">alice hello");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].field_name, "username");
        assert_eq!(filters[0].field_value, "alice");
        assert_eq!(filters[1].field_name, "content");
        assert_eq!(filters[1].field_value, "hello");
    }

    #[test]
    fn search_content_highlight_terms_collects_content_words() {
        let terms = search_content_highlight_terms("from:alice hello world");
        assert_eq!(terms, vec!["hello", "world"]);
    }

    #[test]
    fn search_content_highlight_terms_ignores_short_tokens() {
        let terms = search_content_highlight_terms("a");
        assert!(terms.is_empty());
    }

    #[test]
    fn clan_channel_content_search_includes_parsed_filters() {
        let req = build_clan_channel_content_search(42, 7, "from:alice hello", 1);
        assert_eq!(req.filters.len(), 4);
        assert_eq!(req.filters[2].field_name, "username");
        assert_eq!(req.filters[3].field_name, "content");
    }
}
