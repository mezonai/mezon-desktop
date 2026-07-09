use mezon_proto::api;

pub const SEARCH_PAGE_SIZE: i32 = 25;

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

pub fn mention_user_filter(user_id: i64) -> api::FilterParam {
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
    if !query.is_empty() {
        filters.push(content_filter(query));
    }
    build_search_request(filters, page, SEARCH_PAGE_SIZE, Vec::new())
}

pub fn build_direct_content_search(
    channel_id: i64,
    query: &str,
    page: i32,
) -> api::SearchMessageRequest {
    let mut filters = direct_channel_scope(channel_id);
    if !query.is_empty() {
        filters.push(content_filter(query));
    }
    build_search_request(filters, page, SEARCH_PAGE_SIZE, Vec::new())
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
}
