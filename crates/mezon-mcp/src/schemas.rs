use serde_json::{Map, Value, json};
use std::sync::Arc;

fn object(properties: Value, required: &[&str]) -> Map<String, Value> {
    serde_json::from_value(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    }))
    .unwrap_or_default()
}

fn empty() -> Map<String, Value> {
    object(json!({}), &[])
}

fn id(desc: &str) -> Value {
    json!({
        "description": desc,
        "oneOf": [
            { "type": "integer" },
            { "type": "string", "pattern": "^[0-9]+$" }
        ]
    })
}

fn string(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn bool(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}

fn integer(desc: &str, default: Option<i64>) -> Value {
    let mut value = json!({ "type": "integer", "description": desc });
    if let Some(default) = default {
        value["default"] = json!(default);
    }
    value
}

fn string_array(desc: &str) -> Value {
    json!({
        "type": "array",
        "description": desc,
        "items": { "type": "string" }
    })
}

fn clan_channel() -> Map<String, Value> {
    object(
        json!({
            "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
            "channel_id": id("Channel snowflake id."),
        }),
        &["clan_id", "channel_id"],
    )
}

fn clan_channel_message() -> Map<String, Value> {
    object(
        json!({
            "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
            "channel_id": id("Channel snowflake id."),
            "message_id": id("Target message snowflake id."),
        }),
        &["clan_id", "channel_id", "message_id"],
    )
}

pub fn input_schema(name: &str) -> Arc<Map<String, Value>> {
    match name {
        "get_app_info"
        | "ping"
        | "get_connection_status"
        | "list_clans"
        | "list_dm_channels"
        | "list_friends"
        | "get_account"
        | "get_current_context"
        | "get_scroll_state"
        | "close_panel"
        | "get_settings"
        | "get_voice_status"
        | "list_stickers"
        | "capture_window"
        | "capture_chat"
        | "go_back"
        | "go_forward"
        | "show_window"
        | "logout"
        | "refresh"
        | "close_modal"
        | "member_menu_state"
        | "member_menu_close"
        | "clan_menu_state"
        | "clan_menu_close"
        | "open_create_clan_modal"
        | "quit_app" => Arc::new(empty()),
        "list_banned_users" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id; 0 asks clan-wide."),
            }),
            &["clan_id"],
        )),
        "member_menu_open" => Arc::new(object(
            json!({
                "user_id": id("Member snowflake id from get_member_list."),
                "x": integer("Menu anchor x in window points (default 0).", Some(0)),
                "y": integer("Menu anchor y in window points (default 0).", Some(0)),
            }),
            &["user_id"],
        )),
        "member_menu_pick" => Arc::new(object(
            json!({
                "index": integer("Item index from member_menu_open/member_menu_state.", None),
                "value": integer(
                    "Submenu option value; required for the Ban row (seconds, 0 = until lifted).",
                    None,
                ),
            }),
            &["index"],
        )),
        "clan_menu_open" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id from list_clans."),
                "x": integer("Menu anchor x in window points (default 0).", Some(0)),
                "y": integer("Menu anchor y in window points (default 0).", Some(0)),
            }),
            &["clan_id"],
        )),
        "clan_menu_pick" => Arc::new(object(
            json!({
                "index": integer("Item index from clan_menu_open/clan_menu_state.", None),
                "value": integer(
                    "Submenu option value; required for the Notification Settings row.",
                    None,
                ),
            }),
            &["index"],
        )),
        "list_categories" => Arc::new(object(
            json!({ "clan_id": id("Clan snowflake id from list_clans.") }),
            &["clan_id"],
        )),
        "create_category" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id from list_clans."),
                "name": { "type": "string", "description": "Category name (letters, digits, space, - or _; must not start with a separator)." },
            }),
            &["clan_id", "name"],
        )),
        "channel_menu_open" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id from list_clans."),
                "channel_id": id("Channel snowflake id from list_channels."),
                "x": integer("Menu anchor x in window points (default 0).", Some(0)),
                "y": integer("Menu anchor y in window points (default 0).", Some(0)),
                "in_favorites": { "type": "boolean", "description": "Right-click the row inside the Favorites section instead of its own category (default false); the Favorites row drops Mark As Read." },
            }),
            &["clan_id", "channel_id"],
        )),
        "channel_menu_pick" => Arc::new(object(
            json!({
                "index": integer("Item index from channel_menu_open/channel_menu_state.", None),
                "value": integer(
                    "Submenu option value; required for the Mute and Notification rows.",
                    None,
                ),
            }),
            &["index"],
        )),
        "category_menu_open" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id from list_clans."),
                "category_id": { "type": "string", "description": "Category id from list_channels." },
                "x": integer("Menu anchor x in window points (default 0).", Some(0)),
                "y": integer("Menu anchor y in window points (default 0).", Some(0)),
            }),
            &["clan_id", "category_id"],
        )),
        "category_menu_pick" => Arc::new(object(
            json!({
                "index": integer("Item index from category_menu_open/category_menu_state.", None),
                "value": integer(
                    "Submenu option value; required for the Mute and Notification Settings rows.",
                    None,
                ),
            }),
            &["index"],
        )),
        "create_clan" => Arc::new(object(
            json!({
                "name": { "type": "string", "description": "Clan name (letters, digits, space, - or _; must not start with a separator)." },
                "logo": { "type": "string", "description": "Optional logo URL; empty for none." },
            }),
            &["name"],
        )),
        "list_channels" => Arc::new(object(
            json!({ "clan_id": id("Clan snowflake id to list channels for.") }),
            &["clan_id"],
        )),
        "list_channel_members" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Optional channel id. When omitted, returns all clan members."),
                "channel_type": integer("Channel type when channel_id is set. Default 0.", Some(0)),
            }),
            &["clan_id"],
        )),
        "list_threads" | "list_pinned_messages" | "mark_as_read" => Arc::new(clan_channel()),
        "pin_message" => Arc::new(clan_channel_message()),
        "unpin_message" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Pinned message id."),
                "pin_id": id("Pin entry id from list_pinned_messages."),
            }),
            &["clan_id", "channel_id", "message_id", "pin_id"],
        )),
        "create_poll" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
                "channel_id": id("Channel snowflake id."),
                "question": string("Poll question."),
                "answers": {
                    "type": "array",
                    "description": "Answer options, at least two.",
                    "items": { "type": "string" }
                },
                "expire_hours": integer("Hours until the poll closes. Default 24.", Some(24)),
                "poll_type": integer("0 = single choice (default), 1 = multiple choice.", Some(0)),
            }),
            &["clan_id", "channel_id", "question", "answers"],
        )),
        "vote_poll" => Arc::new(object(
            json!({
                "poll_id": id("Poll snowflake id."),
                "message_id": id("Id of the message carrying the poll."),
                "channel_id": id("Channel snowflake id."),
                "answers": {
                    "type": "array",
                    "description": "Zero-based answer indices.",
                    "items": { "type": "integer" }
                },
            }),
            &["poll_id", "message_id", "channel_id", "answers"],
        )),
        "load_more_messages" => Arc::new(object(
            json!({
                "direction": string("\"older\" (default) walks back through history, \"newer\" walks forward."),
            }),
            &[],
        )),
        "list_messages" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Anchor message id. Use 0 to start from latest."),
                "direction": integer("0 = around anchor, 1 = older, 2 = newer.", Some(0)),
                "limit": integer("Max messages to return (default 50).", Some(50)),
            }),
            &["clan_id", "channel_id"],
        )),
        "get_message" => Arc::new(clan_channel_message()),
        "search_messages" => Arc::new(object(
            json!({
                "query": string("Search text."),
                "size": integer("Max hits to return (default 20).", Some(20)),
            }),
            &["query"],
        )),
        "get_sticker" => Arc::new(object(
            json!({
                "id": id("Sticker id."),
                "name": string("Sticker shortname (case-insensitive)."),
                "shortname": string("Alias for name."),
            }),
            &[],
        )),
        "get_image" => Arc::new(object(
            json!({
                "url": string("Direct image URL to download."),
                "clan_id": id("Clan id when resolving a message attachment."),
                "channel_id": id("Channel id when resolving a message attachment."),
                "message_id": id("Message id when resolving a message attachment."),
                "attachment_url": string("Attachment URL (skips message lookup)."),
                "attachment_index": integer("Zero-based attachment index on the message.", Some(0)),
            }),
            &[],
        )),
        "navigate" => Arc::new(object(
            json!({
                "path": string("In-app route starting with /. Example: /chat/clans/{clan_id}/channels/{channel_id}"),
            }),
            &["path"],
        )),
        "open_channel" => Arc::new(clan_channel()),
        "open_dm" => Arc::new(object(
            json!({
                "channel_id": id("Direct message channel id."),
                "channel_type": integer("DM channel type (default 3).", Some(3)),
            }),
            &["channel_id"],
        )),
        "open_settings" => Arc::new(object(
            json!({
                "page": string("Settings page slug. Default advanced. Examples: advanced, language, appearance, account."),
            }),
            &[],
        )),
        "send_message" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id. Use 0 for the current direct message."),
                "channel_id": id("Channel snowflake id."),
                "content": string("Plain-text message body."),
                "emojis": {
                    "type": "array",
                    "description": "Custom emoji to render inside content. Each shortname must appear in content; every occurrence becomes a span. Look ids up with list_emojis.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "shortname": { "type": "string", "description": "Literal text in content, e.g. \":pepe_joy:\"." },
                            "emoji_id": { "type": "string", "description": "Emoji snowflake id from list_emojis." }
                        },
                        "required": ["shortname", "emoji_id"]
                    }
                },
            }),
            &["clan_id", "channel_id", "content"],
        )),
        "open_image_viewer" => Arc::new(object(
            json!({
                "message_id": id("Message carrying the attachment."),
                "attachment_index": integer("Zero-based attachment index. Default 0.", Some(0)),
            }),
            &["message_id"],
        )),
        "scroll_wheel" => Arc::new(object(
            json!({
                "delta_y": { "type": "number", "description": "Pixels per tick. Negative scrolls toward older messages. Default -120.", "default": -120 },
                "ticks": integer("How many wheel events to send, 1-500. Default 10.", Some(10)),
            }),
            &[],
        )),
        "scroll_messages" => Arc::new(object(
            json!({
                "to": string("\"top\" (default) or \"bottom\"."),
            }),
            &[],
        )),
        "open_panel" => Arc::new(object(
            json!({
                "kind": string("Which composer panel to show: \"emoji\", \"sticker\", \"gif\" or \"sound\"."),
            }),
            &["kind"],
        )),
        "list_emojis" => Arc::new(object(
            json!({
                "clan_id": id("Optional clan snowflake id to filter by."),
                "query": string("Optional case-insensitive substring match on shortname."),
                "limit": integer("Max entries to return. Default 100.", Some(100)),
            }),
            &[],
        )),
        "reply_to_message" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Parent message id to reply to."),
                "content": string("Reply text."),
            }),
            &["clan_id", "channel_id", "message_id", "content"],
        )),
        "react_to_message" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Message to react to."),
                "emoji": string("Emoji character or shortcode."),
                "remove": bool("When true, removes the reaction instead of adding it."),
                "message_sender_id": id("Optional message author id (resolved automatically when omitted)."),
                "topic_id": id("Optional topic id when reacting inside a discussion topic."),
            }),
            &["clan_id", "channel_id", "message_id", "emoji"],
        )),
        "click_message_button" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Message containing the button."),
                "button_id": string("Component id from get_message."),
                "button_label": string("Visible button label (used when button_id is omitted)."),
                "sender_id": id("Message sender id (defaults to message sender)."),
                "user_id": id("Clicking user id (defaults to signed-in user)."),
                "extra_data": string("JSON string passed to the button handler. Default {}."),
            }),
            &["clan_id", "channel_id", "message_id"],
        )),
        "select_message_option" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Message containing the dropdown."),
                "select_id": string("Dropdown component id from get_message."),
                "values": string_array("Selected option values before clicking a submit button."),
            }),
            &["clan_id", "channel_id", "message_id", "select_id"],
        )),
        "edit_message" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Message to edit."),
                "content": string("New message text."),
                "topic_id": id("Optional topic id when editing inside a discussion topic."),
                "is_update_msg_topic": bool("When true, sends the edit as a topic message update."),
            }),
            &["clan_id", "channel_id", "message_id", "content"],
        )),
        "delete_message" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id. Use 0 for direct messages."),
                "channel_id": id("Channel snowflake id."),
                "message_id": id("Target message snowflake id."),
                "topic_id": id("Optional topic id when deleting inside a discussion topic."),
            }),
            &["clan_id", "channel_id", "message_id"],
        )),
        "send_image" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id."),
                "content": string("Optional caption text."),
                "path": string("Local filesystem path to an image file."),
                "url": string("Remote image URL (alternative to path)."),
            }),
            &["clan_id", "channel_id"],
        )),
        "composer_type" => Arc::new(object(
            json!({ "text": string("Full composer text to set.") }),
            &["text"],
        )),
        "composer_pick" => Arc::new(object(
            json!({
                "index": json!({
                    "type": "integer",
                    "minimum": 0,
                    "description": "Index into the suggestion list (default 0)."
                }),
            }),
            &[],
        )),
        "edit_begin" => Arc::new(object(
            json!({ "message_id": id("Message id to edit.") }),
            &["message_id"],
        )),
        "edit_type" => Arc::new(object(
            json!({ "text": string("Full edit-box text to set.") }),
            &["text"],
        )),
        "edit_pick" => Arc::new(object(
            json!({
                "index": json!({
                    "type": "integer",
                    "minimum": 0,
                    "description": "Suggestion index (default 0)."
                }),
            }),
            &[],
        )),
        "composer_panel_send" => Arc::new(object(
            json!({
                "kind": string("sticker | gif | sound."),
                "url": string("Media url."),
                "filename": string("Filename for sticker/sound."),
                "width": json!({"type": "integer", "description": "GIF width."}),
                "height": json!({"type": "integer", "description": "GIF height."}),
            }),
            &["kind", "url"],
        )),
        "composer_drop_paths" => Arc::new(object(
            json!({
                "paths": json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Local file paths to drop on the composer."
                }),
            }),
            &["paths"],
        )),
        "send_buzz" => Arc::new(object(
            json!({ "text": string("Buzz text.") }),
            &[],
        )),
        "send_attachment" => Arc::new(object(
            json!({
                "path": string("Local filesystem path to one file to send."),
                "paths": json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Local filesystem paths for a multi-attachment (album) send."
                }),
                "content": string("Message text. Required when no path/paths is given."),
                "anonymous": json!({
                    "type": "boolean",
                    "description": "Send as Anonymous in the active clan (default false)."
                }),
                "reply_to": id("Message id to reply to; must be loaded in the active channel."),
            }),
            &[],
        )),
        "send_sticker" => Arc::new(object(
            json!({
                "clan_id": id("Clan snowflake id."),
                "channel_id": id("Channel snowflake id."),
                "url": string("Sticker image URL."),
                "name": string("Sticker shortname (alternative to url)."),
                "shortname": string("Alias for name."),
            }),
            &["clan_id", "channel_id"],
        )),
        "set_setting" => Arc::new(object(
            json!({
                "key": string("Allowlisted key: theme, language, zoom_factor, notifications_enabled, activity_tracking."),
                "value": json!({
                    "description": "Setting value. Type depends on key (string, number, or boolean)."
                }),
            }),
            &["key", "value"],
        )),
        "set_cli_enabled" => Arc::new(object(
            json!({
                "enabled": bool("When true, installs the mezon CLI shim into PATH."),
            }),
            &["enabled"],
        )),
        "list_loaded_messages" => Arc::new(object(
            json!({
                "limit": integer("Max rows from each end of the buffer. Default 50.", Some(50)),
            }),
            &[],
        )),
        "jump_to_message" => Arc::new(object(
            json!({
                "message_id": id("Message snowflake id to centre the window on."),
            }),
            &["message_id"],
        )),
        "set_user_status" => Arc::new(object(
            json!({
                "status": string("\"Online\", \"Idle\", \"Do Not Disturb\" or \"Invisible\"."),
                "minutes": integer("How long the status lasts. 0 (default) means indefinitely.", Some(0)),
                "until_turn_on": bool("Clear the status when the user turns it back on. Default false."),
            }),
            &["status"],
        )),
        _ => Arc::new(empty()),
    }
}
