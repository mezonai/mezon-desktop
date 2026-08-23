use crate::schemas;

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub write: bool,
}

pub const TOOL_SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "get_app_info",
        description: "\
Return Mezon desktop app metadata: version, platform, MCP server status, and auth summary.

Use on startup to confirm the app is reachable and the user is signed in.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "ping",
        description: "\
Health check through the Mezon control plane.

Returns { ok: true } when the desktop app is running and responding.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "get_connection_status",
        description: "\
Return realtime socket connection status for the signed-in session.

Use before sending messages or listing live data.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "list_clans",
        description: "\
List clans the signed-in user belongs to.

Each item includes ids and display names. Use clan_id from here for channel/message tools.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "list_channels",
        description: "\
List channels inside a clan.

Returns channel ids, names, types, and parent relationships. Pair with open_channel or list_messages.

Parameters:
- clan_id (required): clan snowflake id.",
        write: false,
    },
    ToolSpec {
        name: "list_dm_channels",
        description: "\
List direct message channels for the signed-in user.

Use channel_id with open_dm or send_message (clan_id=0).

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "list_friends",
        description: "\
List friends of the signed-in user.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "get_account",
        description: "\
Return the signed-in account profile (user id, username, display name, email, etc.).

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "list_channel_members",
        description: "\
List members in a clan or a specific channel.

When channel_id is omitted, returns up to 100 clan members. When channel_id is set, returns channel participants.

Parameters:
- clan_id (required)
- channel_id (optional)
- channel_type (optional, default 0)",
        write: false,
    },
    ToolSpec {
        name: "list_threads",
        description: "\
List threads in a channel.

Parameters:
- clan_id (required)
- channel_id (required)",
        write: false,
    },
    ToolSpec {
        name: "list_pinned_messages",
        description: "\
List pinned messages in a channel.

Parameters:
- clan_id (required)
- channel_id (required)",
        write: false,
    },
    ToolSpec {
        name: "list_messages",
        description: "\
Fetch messages from a channel with pagination.

Use message_id as an anchor (0 for latest) and direction to page older/newer messages.

Parameters:
- clan_id (required; use 0 for DMs)
- channel_id (required)
- message_id (optional, default 0)
- direction (optional: 0 around, 1 older, 2 newer)
- limit (optional, default 50)",
        write: false,
    },
    ToolSpec {
        name: "get_message",
        description: "\
Fetch one message with full detail: text, attachments, embeds, and interactive components (buttons, dropdowns).

Use before click_message_button or select_message_option.

Parameters:
- clan_id (required)
- channel_id (required)
- message_id (required)",
        write: false,
    },
    ToolSpec {
        name: "search_messages",
        description: "\
Search message content across accessible channels.

Parameters:
- query (required): search text
- size (optional, default 20): max results",
        write: false,
    },
    ToolSpec {
        name: "get_current_context",
        description: "\
Return the UI route and parsed context for the active screen.

Includes route, auth state, user_id, clan_id, and channel_id when viewing chat. Call this first to discover where the user is.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "get_scroll_state",
        description: "\
Report what the open channel's message list currently holds.

Returns loaded_count, has_more_top, has_more_bottom, loading, loading_more and the
active clan/channel ids. load_more_messages refuses while loading or loading_more is
true, so wait for both to clear rather than treating the refusal as end-of-history.
`list_messages` reads history straight from the API without moving the UI, so use this
to learn whether the on-screen list still has older or newer pages to pull in, and pair
it with load_more_messages before capture_chat if you need those rows rendered.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "load_more_messages",
        description: "\
Pull the next page of history into the open channel's message list.

This is the deterministic equivalent of the user scrolling to the top or bottom edge:
it drives the same store fetch, so the rows become visible to capture_chat. Returns
started=false when the list already holds that end of the history.

Check has_more_top / has_more_bottom from get_scroll_state first, and call repeatedly
to walk further back.

Parameters:
- direction (optional): \"older\" (default) or \"newer\".",
        write: false,
    },
    ToolSpec {
        name: "jump_to_message",
        description: "\
Move the open channel's message list to an AROUND window centred on one message.

This is the deterministic equivalent of clicking a search hit or a reply jump link. The
newest message stops being loaded, so get_scroll_state reports has_more_bottom=true
afterwards -- that is the \"tail detached\" state the send and live-append paths branch on.
Use jump_to_present to return to the tail.

Parameters:
- message_id (required)",
        write: true,
    },
    ToolSpec {
        name: "list_loaded_messages",
        description: "\
Return the rows the open channel's message list actually holds, oldest first.

This reads the UI buffer, not the API, so it is what the user can see. Compare it with
list_messages (which reads the server) to tell a delivery failure apart from a message
the server accepted but the list never rendered.

Parameters:
- limit (optional, default 50): return at most this many rows from each end of the buffer.
- topic (optional, default false): read the open topic panel's buffer instead. The topic is
  a bucket of its own and the parent channel stays active while it is open, so a reply sent
  into a topic only shows up here with this set.",
        write: false,
    },
    ToolSpec {
        name: "reply_begin",
        description: "\
Aim the composer at a message, the way the row's Reply action does. Nothing is sent.

Use this when the reply must carry something the composer holds — an attachment from
composer_drop_paths, a mention picked with composer_pick. `reply_to_message` posts a
text-only reply in one call and cannot carry either. Send with composer_submit;
composer_state reports the pending target under `reply_target`.

Parameters:
- message_id (required): must be in the open channel's loaded history.",
        write: true,
    },
    ToolSpec {
        name: "jump_to_present",
        description: "\
Return the open channel's message list to the live tail after a jump_to_message.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "get_user_status",
        description: "\
Return the signed-in user's own presence as the UI resolves it.

Reports the raw account status string, the parsed presence, the custom status, and the
`online` flag every self-aware view derives from it. Pair with get_member_list to check
that the member list agrees with the account.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "get_member_list",
        description: "\
Return the member-list panel's rows exactly as it computes them for the active screen.

Each section carries the header count the panel renders, and each row carries the
`online` flag driving its status dot. A row whose `online` contradicts its section is a
bucketing bug -- the sections come from the presence set while the self row's dot can be
overridden by the account status.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "list_banned_users",
        description: "\
Fetch the raw chat-ban list for a clan/channel straight from the server, bypassing the store cache.

Diagnostic for the Ban/Unban member action: shows exactly what ListBannedUsers returns
(channel_id, banned_id, banner_id, ban_time, reason).

Parameters:
- clan_id (required): clan snowflake id.
- channel_id (optional): channel snowflake id; 0 (default) asks clan-wide.",
        write: false,
    },
    ToolSpec {
        name: "close_modal",
        description: "\
Dismiss the modal the app currently shows (profile, confirmation, settings dialog).

Equivalent to pressing Escape. Returns closed:false when nothing was open.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "member_menu_state",
        description: "\
Return the member-list context menu that is currently open, if any.

Includes the target user, the resolved permission/relationship flags that decide which rows show
(is_friend, is_blocked, is_banned, show_ban, show_kick, show_remove_from_thread), and the item list
with the index to pass to member_menu_pick.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "member_menu_open",
        description: "\
Open the right-click context menu for one member of the visible member list.

The user must be listed by get_member_list. Returns the same shape as member_menu_state, so use the
returned item indexes with member_menu_pick.

Parameters:
- user_id (required): member snowflake id.
- x, y (optional): anchor point in window points, to exercise menu placement near a window edge.",
        write: false,
    },
    ToolSpec {
        name: "member_menu_close",
        description: "\
Dismiss the member-list context menu without running an action.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "member_menu_pick",
        description: "\
Run one row of the open member context menu (Profile, Message, Add Friend, Unblock, Remove Friend,
Ban, Unban, Kick, Remove from thread).

Call member_menu_open first and pick the index from its item list. Rows of kind \"submenu\" or
\"danger_submenu\" (Ban) additionally need value — one of the option values returned for that row
(seconds; 0 means until the ban is lifted).

Destructive: Kick, Ban and Remove-from-thread hit the server immediately; Remove Friend and Kick
open a confirmation modal instead.

Parameters:
- index (required): item index from member_menu_open/member_menu_state.
- value (optional): submenu option value. Omit on a submenu row to just open its flyout.",
        write: true,
    },
    ToolSpec {
        name: "clan_menu_state",
        description: "\
Return the clan-rail context menu that is currently open, if any.

Includes the target clan, whether the signed-in user owns it (owners get no Leave Clan row) and
whether it is the active clan, plus the item list with the index to pass to clan_menu_pick.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "clan_menu_open",
        description: "\
Open the right-click context menu for one clan in the left clan rail.

The clan must be listed by list_clans. Returns the same shape as clan_menu_state, so use the
returned item indexes with clan_menu_pick.

Parameters:
- clan_id (required): clan snowflake id.
- x, y (optional): anchor point in window points, to exercise menu placement near a window edge.",
        write: false,
    },
    ToolSpec {
        name: "clan_menu_close",
        description: "\
Dismiss the clan-rail context menu without running an action.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "clan_menu_pick",
        description: "\
Run one row of the open clan context menu (Mark As Read, Notification Settings, Edit Clan Profile,
Leave Clan).

Call clan_menu_open first and pick the index from its item list. The \"submenu\" row (Notification
Settings) additionally needs value — one of the option values returned for that row.

Mark As Read and Notification Settings hit the server immediately; Edit Clan Profile navigates to
that clan's profile settings; Leave Clan opens a confirmation modal instead of leaving straight
away.

Parameters:
- index (required): item index from clan_menu_open/clan_menu_state.
- value (optional): submenu option value. Omit on a submenu row to just open its flyout.",
        write: true,
    },
    ToolSpec {
        name: "list_categories",
        description: "\
List the categories of a clan as the channel sidebar sees them, including EMPTY ones.

list_channels only reveals categories that still hold a channel, so this is the only way to find an
empty category — which is exactly the case category_menu_pick offers Delete Category for. The clan
must already be loaded (open one of its channels first).

Parameters:
- clan_id (required): clan snowflake id.",
        write: false,
    },
    ToolSpec {
        name: "create_category",
        description: "\
Create a category in a clan through the same store path the Create Category modal uses.

Useful for making a throwaway empty category to exercise the category context menu rows
(Edit Category, Delete Category) without touching a real one.

Parameters:
- clan_id (required): clan snowflake id.
- name (required): category name (letters, digits, space, - or _; must not start with a separator).",
        write: true,
    },
    ToolSpec {
        name: "channel_menu_state",
        description: "\
Return the channel context menu that is currently open in the channel sidebar, if any.

Reports the target channel plus is_favorite (which decides the Mark/Unmark Favorite row) and
can_manage_channel, alongside the item list whose indexes channel_menu_pick takes.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "channel_menu_open",
        description: "\
Open the right-click context menu for one channel or thread in the channel sidebar.

The channel must belong to the clan and be listed by list_channels. Returns the same shape as
channel_menu_state.

Parameters:
- clan_id (required): clan snowflake id.
- channel_id (required): channel snowflake id.
- x, y (optional): anchor point in window points.
- in_favorites (optional): right-click the row inside the Favorites section instead of its own
  category; that row drops Mark As Read, exactly as React does.",
        write: false,
    },
    ToolSpec {
        name: "channel_menu_close",
        description: "\
Dismiss the channel context menu without running an action.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "channel_menu_pick",
        description: "\
Run one row of the open channel context menu (Mark As Read, Copy Link, Mute, Notification,
Mark/Unmark Favorite, Edit Channel, Delete Channel).

Mark/Unmark Favorite writes straight through to the server; Delete Channel opens a confirmation
modal instead of deleting immediately.

Parameters:
- index (required): item index from channel_menu_open/channel_menu_state.
- value (optional): submenu option value (mute duration seconds, notification level).",
        write: true,
    },
    ToolSpec {
        name: "category_menu_state",
        description: "\
Return the category context menu that is currently open in the channel sidebar, if any.

Reports collapsed, can_manage_category and category_is_empty (Delete Category only shows for an
empty category), plus the item list for category_menu_pick.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "category_menu_open",
        description: "\
Open the right-click context menu for one category header in the channel sidebar.

The favourites pseudo-category has no menu and is rejected.

Parameters:
- clan_id (required): clan snowflake id.
- category_id (required): category id string from list_channels.
- x, y (optional): anchor point in window points.",
        write: false,
    },
    ToolSpec {
        name: "category_menu_close",
        description: "\
Dismiss the category context menu without running an action.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "category_menu_pick",
        description: "\
Run one row of the open category context menu (Mark As Read, Collapse Category, Collapse All
Categories, Mute, Notification Settings, Edit Category, Delete Category).

Edit Category opens a rename modal and Delete Category opens a confirmation modal; the rest apply
immediately.

Parameters:
- index (required): item index from category_menu_open/category_menu_state.
- value (optional): submenu option value (mute duration seconds, notification level).",
        write: true,
    },
    ToolSpec {
        name: "open_create_clan_modal",
        description: "\
Open the Create Clan modal on its template-picker step.

Renders over everything, so pair it with capture_window to inspect the modal chrome. Dismiss it
with close_modal. Use create_clan instead to actually create one without the UI.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "create_clan",
        description: "\
Create a clan without opening the modal, through the same store path the modal uses.

The name is validated server-side for duplicates and the new clan becomes the active one. Useful
for making a throwaway clan to exercise clan_menu_pick rows against.

Parameters:
- name (required): clan name.
- logo (optional): logo URL; omit for none.",
        write: true,
    },
    ToolSpec {
        name: "set_user_status",
        description: "\
Set the signed-in user's own presence.

Mirrors the footer profile popup. The request always reaches the server, including a
duration-only change (same status, new minutes). status_changed reports whether the
local status string moved, which is what drives the StatusUpdated event.

Parameters:
- status (required): \"Online\", \"Idle\", \"Do Not Disturb\" or \"Invisible\"
- minutes (optional, default 0): how long the status lasts, 0 for indefinite
- until_turn_on (optional, default false): clear the status when the user turns it back on",
        write: true,
    },
    ToolSpec {
        name: "open_panel",
        description: "\
Open one of the composer panels: emoji, sticker, gif or sound.

The panel renders over the chat, so pair it with capture_window to see it. It stays
open until close_panel or until the user dismisses it.

Parameters:
- kind (required): \"emoji\", \"sticker\", \"gif\" or \"sound\".",
        write: false,
    },
    ToolSpec {
        name: "open_topic",
        description: "\
Open the discussion-topic side panel on a message of the open channel.

The message must be in the channel's loaded history, so call open_channel (and
load_more_messages when it sits further back) first. A message that has no topic yet
gets one on the first topic_submit, so topic_id stays null until then.

Parameters:
- message_id (required)",
        write: true,
    },
    ToolSpec {
        name: "close_topic",
        description: "\
Close the discussion-topic side panel.

Returns ok even when no panel was open.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "topic_state",
        description: "\
Report what the open topic panel holds.

Returns panel_open, topic_id, origin_message_id, loaded_count, has_more_top,
loading_more and the panel list's item_count / first_visible_index / at_bottom.
has_more_top drives topic paging the way get_scroll_state does for a channel.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "topic_type",
        description: "\
Replace the topic composer's text, the way typing into the panel does.

Parameters:
- text (required)",
        write: false,
    },
    ToolSpec {
        name: "topic_submit",
        description: "\
Send whatever the topic composer holds, the way pressing Enter in the panel does.

The first submit on a message without a topic creates the topic.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "topic_scroll_wheel",
        description: "\
Send wheel events to the topic panel's message list, the way a mouse wheel does.

The topic counterpart of scroll_wheel: events are aimed at the panel list's bounds and
paced one per frame, so reaching the top pages older replies in. Read topic_state
before and after to see loaded_count grow and has_more_top settle.

Parameters:
- delta_y (optional): pixels per tick. Positive scrolls toward older replies, negative
  toward the newest one. Default -120.
- ticks (optional): how many wheel events to send, 1-500. Default 10.",
        write: false,
    },
    ToolSpec {
        name: "close_panel",
        description: "\
Close the composer panel opened by open_panel.

Returns ok even when no panel was open.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "open_image_viewer",
        description: "\
Open the full-screen image viewer on a message attachment.

The message must be in the open channel's loaded history, so call open_channel first
and load_more_messages if it sits further back. The viewer fetches the rest of the
channel's media itself, so it can be paged once open.

Parameters:
- message_id (required): message carrying the attachment.
- attachment_index (optional): zero-based index on that message. Default 0.",
        write: false,
    },
    ToolSpec {
        name: "open_pdf_viewer",
        description: "\
Open the pdf viewer window on a message's pdf attachment.

Same reach as open_image_viewer: the message must be in the open channel's loaded
history (or the open topic's), so call open_channel and load_more_messages first.
The row only offers this for a pdf attachment, and so does this tool.

Parameters:
- message_id (required): message carrying the pdf attachment.
- attachment_index (optional): zero-based index on that message. Default 0.",
        write: false,
    },
    ToolSpec {
        name: "scroll_wheel",
        description: "\
Send wheel events to the message list, the way a mouse wheel does.

scroll_messages jumps the viewport, which cancels the wheel animation and never sets
the list's scroll-active flag. That flag matters: while it is set the chat suppresses
its image-cache sweep, so a jump measures the app under lighter memory pressure than
real scrolling does. Use this when the run has to reproduce scrolling behaviour, and
scroll_messages when you only need to land at an edge.

Events go through GPUI's own dispatch, not the OS, so they cannot land in another
application if the window is not frontmost. They are aimed at the message list's real
bounds and paced one per frame, so the list animates and loads history the way it does
under a hand on the wheel; a run of 500 ticks therefore takes about 8 seconds.

Read `moved` to tell a real scroll from a no-op: it compares the viewport before and
after. `consumed_ticks` counts events a handler stopped propagating, which the message
list does not do even when it scrolls, so it stays 0 on a working scroll and is only
useful for debugging.

Parameters:
- delta_y (optional): pixels per tick. Positive scrolls toward older messages (up the
  history), negative toward the live tail. Default -120.
- ticks (optional): how many wheel events to send, 1-500. Default 10.",
        write: false,
    },
    ToolSpec {
        name: "scroll_messages",
        description: "\
Move the open channel's message list to the top or the bottom.

load_more_messages prepends older rows and drops the newest ones to stay inside the
buffer cap, which is invisible when the reader is already at the top but pulls rows
out from under a reader sitting at the bottom. Scroll to the top first to walk back
through history the way a user does.

Returns item_count, first_visible_index and at_bottom.

Parameters:
- to (optional): \"top\" (default) or \"bottom\".",
        write: false,
    },
    ToolSpec {
        name: "list_emojis",
        description: "\
List custom emoji available to the signed-in user.

Emoji in a message body are spans, not text: sending the literal \":shortname:\" through
send_message renders as plain characters. Look the emoji up here, then pass its
shortname and emoji_id in send_message's `emojis` array so it renders.

Parameters:
- clan_id (optional): restrict to one clan's emoji.
- query (optional): case-insensitive substring match on shortname.
- limit (optional): max entries to return. Default 100.",
        write: false,
    },
    ToolSpec {
        name: "pin_message",
        description: "\
Pin a message to the channel.

The pin shows up for everyone in the channel; list_pinned_messages reads them back.

Parameters:
- clan_id (required): clan snowflake id. Use 0 for direct messages.
- channel_id (required): channel snowflake id.
- message_id (required): message to pin.",
        write: true,
    },
    ToolSpec {
        name: "unpin_message",
        description: "\
Remove a pinned message from the channel.

Call list_pinned_messages first: `pin_id` is the entry's own id, which is not the
message id.

Parameters:
- clan_id (required): clan snowflake id. Use 0 for direct messages.
- channel_id (required): channel snowflake id.
- message_id (required): the pinned message's id.
- pin_id (required): the pin entry id from list_pinned_messages.",
        write: true,
    },
    ToolSpec {
        name: "create_poll",
        description: "\
Post a poll to the channel.

Returns the created poll so its id can be passed to vote_poll.

Parameters:
- clan_id (required): clan snowflake id. Use 0 for direct messages.
- channel_id (required): channel snowflake id.
- question (required): poll question.
- answers (required): array of 2 or more answer strings.
- expire_hours (optional): hours until the poll closes. Default 24.
- poll_type (optional): 0 = single choice (default), 1 = multiple choice.",
        write: true,
    },
    ToolSpec {
        name: "vote_poll",
        description: "\
Cast a vote on an existing poll.

Parameters:
- poll_id (required): poll snowflake id.
- message_id (required): id of the message carrying the poll.
- channel_id (required): channel snowflake id.
- answers (required): array of zero-based answer indices. Single-choice polls take one.",
        write: true,
    },
    ToolSpec {
        name: "get_settings",
        description: "\
Read app settings: theme, language, zoom, notifications, voice state, and related flags.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "join_voice",
        description: "\
Join the voice call of a voice channel.

Parameters: clan_id (string), channel_id (string).",
        write: true,
    },
    ToolSpec {
        name: "leave_voice",
        description: "\
Leave the current voice call.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "get_recording_state",
        description: "\
Return call-recording state: state (idle|starting|recording|stopping), elapsed_seconds,
video_stalled, can_record, in_call.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "start_recording",
        description: "\
Start recording the current voice call, skipping the native save dialog. Requires being in a call.

Parameters: path (optional string) - absolute output path; defaults to a timestamped file in Downloads.",
        write: true,
    },
    ToolSpec {
        name: "stop_recording",
        description: "\
Stop the running call recording and finalize the file.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "get_voice_status",
        description: "\
Return voice call status derived from settings: in_call, channel label, mic/camera enabled.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "list_stickers",
        description: "\
List stickers available to the signed-in user.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "get_sticker",
        description: "\
Look up one sticker by id or shortname.

Parameters (provide one):
- id
- name or shortname",
        write: false,
    },
    ToolSpec {
        name: "get_image",
        description: "\
Download an image and return base64 bytes.

Provide either url directly, or clan_id + channel_id + message_id (+ optional attachment_index) to resolve a message attachment.

Parameters:
- url (optional)
- clan_id, channel_id, message_id, attachment_index (optional)
- attachment_url (optional shortcut)",
        write: false,
    },
    ToolSpec {
        name: "capture_window",
        description: "\
Capture a PNG screenshot of the entire Mezon main window using OS screen capture (scap).

Requires Screen Recording permission (macOS). The window must be visible on screen. Does not use GPUI render-to-texture.

Returns: { format: \"png\", width, height, region: \"window\", source: \"scap\", data_base64 }.

Tip: decode data_base64 to a file, then send_image with path.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "capture_chat",
        description: "\
Capture a PNG screenshot of the chat panel only (excludes the left sidebar) using OS screen capture (scap).

Requires Screen Recording permission (macOS). The window must be visible. Cropping uses the fixed sidebar width and current display scale factor.

Returns: { format: \"png\", width, height, region: \"chat\", source: \"scap\", data_base64 }.

Workflow: get_current_context → capture_chat → write PNG to disk → send_image.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "navigate",
        description: "\
Navigate the in-app router to a path.

Path must start with / and must not be an external URL. Examples: /settings/advanced, /chat/clans/{clan_id}/channels/{channel_id}.

Parameters:
- path (required)",
        write: true,
    },
    ToolSpec {
        name: "open_channel",
        description: "\
Open a clan channel in the UI.

Equivalent to navigating to /chat/clans/{clan_id}/channels/{channel_id}.

Parameters:
- clan_id (required)
- channel_id (required)",
        write: true,
    },
    ToolSpec {
        name: "open_dm",
        description: "\
Open a direct message channel in the UI.

Parameters:
- channel_id (required)
- channel_type (optional, default 3)",
        write: true,
    },
    ToolSpec {
        name: "open_settings",
        description: "\
Open an app settings page.

Parameters:
- page (optional, default advanced). Examples: advanced, language, appearance, account.",
        write: true,
    },
    ToolSpec {
        name: "go_back",
        description: "\
Navigate back in the in-app history stack.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "go_forward",
        description: "\
Navigate forward in the in-app history stack.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "show_window",
        description: "\
Bring the main Mezon window to the foreground.

Useful before capture_chat/capture_window so the window is visible.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "send_message",
        description: "\
Send a plain-text message to a channel.

For direct messages, use clan_id=0 and the DM channel_id. The app auto-joins the channel when needed.

Parameters:
- clan_id (required)
- channel_id (required)
- content (required)",
        write: true,
    },
    ToolSpec {
        name: "reply_to_message",
        description: "\
Reply to an existing message in a channel.

Parameters:
- clan_id (required)
- channel_id (required)
- message_id (required): parent message
- content (required)",
        write: true,
    },
    ToolSpec {
        name: "react_to_message",
        description: "\
Add or remove an emoji reaction on a message.

Parameters:
- clan_id (required)
- channel_id (required)
- message_id (required)
- emoji (required)
- remove (optional boolean)
- message_sender_id (optional)
- topic_id (optional): discussion topic id when reacting inside a topic",
        write: true,
    },
    ToolSpec {
        name: "click_message_button",
        description: "\
Click an interactive button attached to a message.

Fetch the message first with get_message to read button ids/labels.

Parameters:
- clan_id, channel_id, message_id (required)
- button_id or button_label (one required)
- sender_id, user_id, extra_data (optional)",
        write: true,
    },
    ToolSpec {
        name: "select_message_option",
        description: "\
Select value(s) on a dropdown component before submitting a message button.

Parameters:
- clan_id, channel_id, message_id, select_id (required)
- values (optional string array)",
        write: true,
    },
    ToolSpec {
        name: "edit_message",
        description: "\
Edit the text content of an existing message.

Parameters:
- clan_id (required)
- channel_id (required)
- message_id (required)
- content (required)
- topic_id (optional): discussion topic id when editing inside a topic
- is_update_msg_topic (optional boolean)",
        write: true,
    },
    ToolSpec {
        name: "delete_message",
        description: "\
Delete a message from a channel.

Parameters:
- clan_id (required)
- channel_id (required)
- message_id (required)
- topic_id (optional): discussion topic id when deleting inside a topic",
        write: true,
    },
    ToolSpec {
        name: "mark_as_read",
        description: "\
Mark a channel as read for the signed-in user.

Parameters:
- clan_id (required)
- channel_id (required)",
        write: true,
    },
    ToolSpec {
        name: "send_image",
        description: "\
Send an image to a channel from a local file path or remote URL.

For captures from capture_chat, write data_base64 to a .png file and pass path.

Parameters:
- clan_id (required)
- channel_id (required)
- path or url (one required)
- content (optional caption)",
        write: true,
    },
    ToolSpec {
        name: "composer_type",
        description: "\
Type text into the active channel's composer, exactly as the user would.

Drives the real MentionInput, so trigger characters open their popup: @ (member/role),
# (channel), : (emoji), / (slash command). Returns the composer state including the
suggestion list. Follow with composer_pick to accept one, then composer_submit to send.

Parameters:
- text (required): full composer text to set.",
        write: true,
    },
    ToolSpec {
        name: "composer_state",
        description: "\
Read the active composer: current text, whether a suggestion popup is open, the
suggestion labels, the selected index, and which panel (emoji/sticker/gif/sound) is open.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "composer_pick",
        description: "\
Accept a suggestion from the open composer popup, like clicking it or pressing Enter.

Parameters:
- index (optional, default 0): index into the list returned by composer_type/composer_state.",
        write: true,
    },
    ToolSpec {
        name: "composer_submit",
        description: "\
Press Enter on the composer to send whatever it currently holds (text, committed
mention/emoji tokens, pending attachments).

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "edit_begin",
        description: "\
Open the inline edit box on a message, exactly as the Edit action does.

The returned text is what the edit box was SEEDED with — that is the markdown source
reconstructed from the stored (marker-stripped) content, so it reveals whether code
fences / markers survive a round-trip. Follow with edit_type / edit_pick / edit_save.

Parameters:
- message_id (required): message to edit, loaded in the active channel.",
        write: true,
    },
    ToolSpec {
        name: "edit_type",
        description: "\
Replace the inline edit box text. Trigger characters open their popup just like the
composer.

Parameters:
- text (required).",
        write: true,
    },
    ToolSpec {
        name: "edit_pick",
        description: "\
Accept a suggestion from the edit box popup.

Parameters:
- index (optional, default 0).",
        write: true,
    },
    ToolSpec {
        name: "edit_state",
        description: "\
Read the inline edit box: target message id, text, popup state and suggestions.

Parameters: none.",
        write: false,
    },
    ToolSpec {
        name: "edit_save",
        description: "\
Save the inline edit, as pressing Enter does. Runs the real store edit path.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "composer_panel_send",
        description: "\
Send a sticker, GIF or sound exactly as picking it from the composer panel does.

Emits the same composer event the panel emits, so it exercises the real send pipeline.
Get candidates from list_stickers / list_emojis.

Parameters:
- kind (required): sticker | gif | sound.
- url (required): media url.
- filename (optional), width/height (optional, GIF only).",
        write: true,
    },
    ToolSpec {
        name: "topic_drop_paths",
        description: "\
Drop local files onto the TOPIC panel's composer; send them with topic_submit.

composer_drop_paths always targets the channel composer even while the topic
panel is open, so use this one for attachments meant for a topic. Call
open_topic first.

Staging is asynchronous: this returns as soon as the paths are handed over, so
poll topic_state until its `attachments` lists the files before topic_submit —
submitting earlier sends the reply without them.

Parameters:
- paths (required): array of local file paths.",
        write: true,
    },
    ToolSpec {
        name: "composer_drop_paths",
        description: "\
Drop local files onto the composer, like a drag-and-drop. They become pending
attachments; send them with composer_submit.

The file is read on a background task, so poll composer_state until its
`attachments` lists it before composer_submit.

Parameters:
- paths (required): array of local file paths.",
        write: true,
    },
    ToolSpec {
        name: "send_buzz",
        description: "\
Send a BUZZ message to the active channel. Refused while anonymous mode is on,
matching the composer's own guard.

Parameters:
- text (optional): buzz text.",
        write: true,
    },
    ToolSpec {
        name: "send_attachment",
        description: "\
Send a local file to the ACTIVE channel through the app's own composer pipeline.

Unlike send_image (which talks to the API directly), this drives MessagesStore::send_message,
so it exercises optimistic rows, the presign flow and anonymous mode exactly as the UI does.
Open the target channel first with open_channel or open_dm.

Parameters:
- path / paths (optional): one or several local file paths.
- content (optional): message text. Required when no file is given.
- anonymous (optional, default false): send as Anonymous in the active clan.
- reply_to (optional): message id to reply to, loaded in the active channel.",
        write: true,
    },
    ToolSpec {
        name: "send_sticker",
        description: "\
Send a sticker to a channel by URL or shortname.

Parameters:
- clan_id (required)
- channel_id (required)
- url or name/shortname (one required)",
        write: true,
    },
    ToolSpec {
        name: "set_setting",
        description: "\
Update an allowlisted app setting.

Allowed keys: theme, language, zoom_factor, notifications_enabled, activity_tracking.

Parameters:
- key (required)
- value (required; string, number, or boolean depending on key)",
        write: true,
    },
    ToolSpec {
        name: "set_cli_enabled",
        description: "\
Install or remove the mezon CLI shim in the user PATH.

Parameters:
- enabled (required boolean)",
        write: true,
    },
    ToolSpec {
        name: "logout",
        description: "\
Sign out of the current Mezon session.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "refresh",
        description: "\
Refresh clans, direct messages, and message lists in the UI.

Parameters: none.",
        write: true,
    },
    ToolSpec {
        name: "quit_app",
        description: "\
Quit the Mezon desktop application.

Parameters: none.",
        write: true,
    },
];

pub fn list_tools_json(read_only: bool) -> serde_json::Value {
    let tools: Vec<_> = TOOL_SPECS
        .iter()
        .filter(|tool| !read_only || !tool.write)
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "write": tool.write,
                "inputSchema": schemas::input_schema(tool.name),
            })
        })
        .collect();
    serde_json::json!({ "tools": tools })
}

pub fn is_write_tool(name: &str) -> bool {
    TOOL_SPECS
        .iter()
        .find(|tool| tool.name == name)
        .is_some_and(|tool| tool.write)
}

#[cfg(test)]
mod tests {
    use super::{TOOL_SPECS, list_tools_json};
    use crate::schemas;

    #[test]
    fn every_tool_has_description_and_schema() {
        for tool in TOOL_SPECS {
            assert!(!tool.description.is_empty(), "{}", tool.name);
            let schema = schemas::input_schema(tool.name);
            assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
        }
    }

    #[test]
    fn list_tools_json_includes_input_schema() {
        let value = list_tools_json(false);
        let tools = value
            .get("tools")
            .and_then(|v| v.as_array())
            .expect("tools array");
        assert_eq!(tools.len(), TOOL_SPECS.len());
        assert!(tools[0].get("inputSchema").is_some());
    }
}
