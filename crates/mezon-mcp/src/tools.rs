use crate::command::{CaptureTarget, McpCommand};
use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures::channel::mpsc::UnboundedSender;
use mezon_client::transport::{
    ApiComponentPayload, ApiEmbed, ApiMessage, ApiMessageComponent, OutgoingEmoji, OutgoingReply,
};
use mezon_client::{AppApi, ConnectionStatus, UploadFile, UrlAttachment};
use mezon_proto::api::SearchMessageDocument;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

const CHANNEL_TYPE_CHANNEL: i32 = 1;
const CHANNEL_TYPE_THREAD: i32 = 7;
const STREAM_MODE_CHANNEL: i32 = 2;
const STREAM_MODE_THREAD: i32 = 6;

#[derive(Clone)]
pub struct McpBackend {
    api: Arc<AppApi>,
    ui_tx: Option<UnboundedSender<McpCommand>>,
    read_only: bool,
}

impl McpBackend {
    pub fn new(
        api: Arc<AppApi>,
        ui_tx: Option<UnboundedSender<McpCommand>>,
        read_only: bool,
    ) -> Self {
        Self {
            api,
            ui_tx,
            read_only,
        }
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        match name {
            "get_app_info" => self.get_app_info().await,
            "ping" => Ok(serde_json::json!({ "ok": true })),
            "get_connection_status" => self.get_connection_status().await,
            "list_clans" => self.list_clans().await,
            "list_channels" => {
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                self.list_channels(clan_id).await
            }
            "list_dm_channels" => self.list_dm_channels().await,
            "list_friends" => self.list_friends().await,
            "get_account" => self.get_account().await,
            "list_channel_members" => self.list_channel_members(&arguments).await,
            "list_threads" => {
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                self.list_threads(clan_id, channel_id).await
            }
            "list_pinned_messages" => {
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                self.list_pinned_messages(clan_id, channel_id).await
            }
            "list_messages" => {
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                let message_id = arguments
                    .get("message_id")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let direction = arguments
                    .get("direction")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32;
                let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
                self.list_messages(clan_id, channel_id, message_id, direction, limit)
                    .await
            }
            "get_message" => {
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                let message_id = parse_i64_field(&arguments, "message_id")?;
                self.get_message(clan_id, channel_id, message_id).await
            }
            "search_messages" => {
                let query = arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let size = arguments.get("size").and_then(Value::as_i64).unwrap_or(20) as i32;
                self.search_messages(&query, size).await
            }
            "get_current_context" => self.get_current_context().await,
            "get_scroll_state" => self.get_scroll_state().await,
            "scroll_wheel" => {
                let delta_y = arguments
                    .get("delta_y")
                    .and_then(Value::as_f64)
                    .unwrap_or(-120.0) as f32;
                let ticks = arguments.get("ticks").and_then(Value::as_u64).unwrap_or(10) as u32;
                self.send_ui_result(|reply| McpCommand::ScrollWheel {
                    delta_y,
                    ticks,
                    reply,
                })
                .await
            }
            "scroll_messages" => {
                let to_top = arguments
                    .get("to")
                    .and_then(Value::as_str)
                    .map(|to| !to.eq_ignore_ascii_case("bottom"))
                    .unwrap_or(true);
                self.send_ui_result(|reply| McpCommand::ScrollMessages { to_top, reply })
                    .await
            }
            "open_panel" => {
                let kind = arguments
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("open_panel requires string field kind"))?
                    .to_string();
                self.send_ui_result(|reply| McpCommand::SetPanel {
                    kind: Some(kind),
                    reply,
                })
                .await
            }
            "open_image_viewer" => {
                let message_id = parse_i64_field(&arguments, "message_id")?;
                let attachment_index = arguments
                    .get("attachment_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                self.send_ui_result(|reply| McpCommand::OpenImageViewer {
                    message_id,
                    attachment_index,
                    reply,
                })
                .await
            }
            "close_panel" => {
                self.send_ui_result(|reply| McpCommand::SetPanel { kind: None, reply })
                    .await
            }
            "open_topic" => {
                let message_id = parse_i64_field(&arguments, "message_id")?;
                self.send_ui_result(|reply| McpCommand::OpenTopic { message_id, reply })
                    .await
            }
            "close_topic" => self.send_ui_result(|reply| McpCommand::CloseTopic { reply }).await,
            "topic_state" => self.send_ui_result(|reply| McpCommand::TopicState { reply }).await,
            "topic_type" => {
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("topic_type requires string field text"))?
                    .to_string();
                self.send_ui_result(|reply| McpCommand::TopicType { text, reply })
                    .await
            }
            "topic_submit" => {
                self.send_ui_result(|reply| McpCommand::TopicSubmit { reply })
                    .await
            }
            "topic_scroll_wheel" => {
                let delta_y = arguments
                    .get("delta_y")
                    .and_then(Value::as_f64)
                    .unwrap_or(-120.0) as f32;
                let ticks = arguments.get("ticks").and_then(Value::as_u64).unwrap_or(10) as u32;
                self.send_ui_result(|reply| McpCommand::TopicScrollWheel {
                    delta_y,
                    ticks,
                    reply,
                })
                .await
            }
            "list_emojis" => {
                let clan_id = arguments
                    .get("clan_id")
                    .and_then(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| value.as_i64().map(|id| id.to_string()))
                    })
                    .filter(|id| id != "0");
                let query = arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .clamp(1, 1000) as usize;
                self.send_ui_result(|reply| McpCommand::ListEmojis {
                    clan_id,
                    query,
                    limit,
                    reply,
                })
                .await
            }
            "load_more_messages" => {
                let older = arguments
                    .get("direction")
                    .and_then(Value::as_str)
                    .map(|d| !d.eq_ignore_ascii_case("newer"))
                    .unwrap_or(true);
                self.load_more_messages(older).await
            }
            "list_loaded_messages" => {
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(50)
                    .clamp(1, 500) as usize;
                self.send_ui_result(|reply| McpCommand::ListLoadedMessages { limit, reply })
                    .await
            }
            "jump_to_message" => {
                self.require_write_mode("jump_to_message")?;
                let message_id = parse_i64_field(&arguments, "message_id")?;
                self.jump_to_message(message_id).await
            }
            "jump_to_present" => {
                self.require_write_mode("jump_to_present")?;
                self.jump_to_present().await
            }
            "get_user_status" => self.get_user_status().await,
            "get_member_list" => self.get_member_list().await,
            "set_user_status" => {
                self.require_write_mode("set_user_status")?;
                self.set_user_status(&arguments).await
            }
            "get_settings" => self.get_settings().await,
            "get_voice_status" => self.get_voice_status().await,
            "list_stickers" => self.list_stickers().await,
            "get_sticker" => self.get_sticker(&arguments).await,
            "get_image" => self.get_image(&arguments).await,
            "capture_window" => self.capture(CaptureTarget::Window).await,
            "capture_chat" => self.capture(CaptureTarget::Chat).await,
            "navigate" => {
                self.require_write_mode("navigate")?;
                let path = arguments
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("navigate requires string field path"))?
                    .to_string();
                self.navigate(&path).await
            }
            "open_channel" => {
                self.require_write_mode("open_channel")?;
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                let path = format!("/chat/clans/{clan_id}/channels/{channel_id}");
                self.navigate(&path).await
            }
            "open_dm" => {
                self.require_write_mode("open_dm")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                let channel_type = arguments
                    .get("channel_type")
                    .and_then(Value::as_u64)
                    .unwrap_or(3) as u32;
                let path = format!("/chat/direct/message/{channel_id}/{channel_type}");
                self.navigate(&path).await
            }
            "open_settings" => {
                self.require_write_mode("open_settings")?;
                let page = arguments
                    .get("page")
                    .and_then(Value::as_str)
                    .unwrap_or("advanced");
                let path = format!("/settings/{page}");
                self.navigate(&path).await
            }
            "go_back" => {
                self.require_write_mode("go_back")?;
                self.send_ui_ok(|reply| McpCommand::GoBack { reply }).await
            }
            "go_forward" => {
                self.require_write_mode("go_forward")?;
                self.send_ui_ok(|reply| McpCommand::GoForward { reply })
                    .await
            }
            "show_window" => {
                self.require_write_mode("show_window")?;
                self.send_ui_ok(|reply| McpCommand::ShowWindow { reply })
                    .await
            }
            "send_message" => {
                self.require_write_mode("send_message")?;
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("send_message requires string field content"))?
                    .to_string();
                let emojis = build_emoji_spans(&content, arguments.get("emojis"))?;
                self.send_message(clan_id, channel_id, &content, emojis)
                    .await
            }
            "reply_to_message" => {
                self.require_write_mode("reply_to_message")?;
                self.reply_to_message(&arguments).await
            }
            "react_to_message" => {
                self.require_write_mode("react_to_message")?;
                self.react_to_message(&arguments).await
            }
            "pin_message" => {
                self.require_write_mode("pin_message")?;
                self.pin_message(&arguments).await
            }
            "unpin_message" => {
                self.require_write_mode("unpin_message")?;
                self.unpin_message(&arguments).await
            }
            "create_poll" => {
                self.require_write_mode("create_poll")?;
                self.create_poll(&arguments).await
            }
            "vote_poll" => {
                self.require_write_mode("vote_poll")?;
                self.vote_poll(&arguments).await
            }
            "click_message_button" => {
                self.require_write_mode("click_message_button")?;
                self.click_message_button(&arguments).await
            }
            "select_message_option" => {
                self.require_write_mode("select_message_option")?;
                self.select_message_option(&arguments).await
            }
            "edit_message" => {
                self.require_write_mode("edit_message")?;
                self.edit_message(&arguments).await
            }
            "delete_message" => {
                self.require_write_mode("delete_message")?;
                self.delete_message(&arguments).await
            }
            "mark_as_read" => {
                self.require_write_mode("mark_as_read")?;
                let clan_id = parse_i64_field(&arguments, "clan_id")?;
                let channel_id = parse_i64_field(&arguments, "channel_id")?;
                self.mark_as_read(clan_id, channel_id).await
            }
            "send_image" => {
                self.require_write_mode("send_image")?;
                self.send_image(&arguments).await
            }
            "composer_type" => {
                self.require_write_mode("composer_type")?;
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("composer_type requires string field text"))?
                    .to_string();
                self.send_ui_result(|reply| McpCommand::ComposerType { text, reply })
                    .await
            }
            "composer_state" => {
                self.send_ui_result(|reply| McpCommand::ComposerState { reply })
                    .await
            }
            "composer_pick" => {
                self.require_write_mode("composer_pick")?;
                let index = arguments
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                self.send_ui_result(|reply| McpCommand::ComposerPick { index, reply })
                    .await
            }
            "composer_submit" => {
                self.require_write_mode("composer_submit")?;
                self.send_ui_result(|reply| McpCommand::ComposerSubmit { reply })
                    .await
            }
            "edit_begin" => {
                self.require_write_mode("edit_begin")?;
                let message_id = parse_i64_field(&arguments, "message_id")?;
                self.send_ui_result(|reply| McpCommand::EditBegin { message_id, reply })
                    .await
            }
            "edit_type" => {
                self.require_write_mode("edit_type")?;
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("edit_type requires string field text"))?
                    .to_string();
                self.send_ui_result(|reply| McpCommand::EditType { text, reply })
                    .await
            }
            "edit_pick" => {
                self.require_write_mode("edit_pick")?;
                let index = arguments
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                self.send_ui_result(|reply| McpCommand::EditPick { index, reply })
                    .await
            }
            "edit_state" => {
                self.send_ui_result(|reply| McpCommand::EditState { reply })
                    .await
            }
            "edit_save" => {
                self.require_write_mode("edit_save")?;
                self.send_ui_result(|reply| McpCommand::EditSave { reply })
                    .await
            }
            "composer_panel_send" => {
                self.require_write_mode("composer_panel_send")?;
                let kind = arguments
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow::anyhow!("composer_panel_send requires string field kind")
                    })?
                    .to_string();
                let url = arguments
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("composer_panel_send requires string field url"))?
                    .to_string();
                let filename = arguments
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let width = arguments.get("width").and_then(Value::as_i64).unwrap_or(0) as i32;
                let height = arguments.get("height").and_then(Value::as_i64).unwrap_or(0) as i32;
                self.send_ui_result(|reply| McpCommand::ComposerPanelSend {
                    kind,
                    url,
                    filename,
                    width,
                    height,
                    reply,
                })
                .await
            }
            "composer_drop_paths" => {
                self.require_write_mode("composer_drop_paths")?;
                let paths: Vec<String> = arguments
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if paths.is_empty() {
                    anyhow::bail!("composer_drop_paths requires a non-empty paths array");
                }
                self.send_ui_result(|reply| McpCommand::ComposerDropPaths { paths, reply })
                    .await
            }
            "send_buzz" => {
                self.require_write_mode("send_buzz")?;
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.send_ui_result(|reply| McpCommand::SendBuzz { text, reply })
                    .await
            }
            "send_attachment" => {
                self.require_write_mode("send_attachment")?;
                let mut paths: Vec<String> = arguments
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(path) = arguments.get("path").and_then(Value::as_str) {
                    paths.push(path.to_string());
                }
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if paths.is_empty() && content.is_empty() {
                    anyhow::bail!("send_attachment requires path/paths or content");
                }
                let anonymous = arguments
                    .get("anonymous")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let reply_to = optional_i64_field(&arguments, "reply_to").unwrap_or(0);
                self.send_ui_result(|reply| McpCommand::SendAttachment {
                    paths,
                    content,
                    anonymous,
                    reply_to,
                    reply,
                })
                .await
            }
            "send_sticker" => {
                self.require_write_mode("send_sticker")?;
                self.send_sticker(&arguments).await
            }
            "set_setting" => {
                self.require_write_mode("set_setting")?;
                self.set_setting(&arguments).await
            }
            "set_cli_enabled" => {
                self.require_write_mode("set_cli_enabled")?;
                let enabled = arguments
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        anyhow::anyhow!("set_cli_enabled requires bool field enabled")
                    })?;
                self.set_cli_enabled(enabled).await
            }
            "logout" => {
                self.require_write_mode("logout")?;
                self.send_ui_ok(|reply| McpCommand::Logout { reply }).await
            }
            "refresh" => {
                self.require_write_mode("refresh")?;
                self.send_ui_ok(|reply| McpCommand::Refresh { reply }).await
            }
            "quit_app" => {
                self.require_write_mode("quit_app")?;
                self.send_ui_ok(|reply| McpCommand::Quit { reply }).await
            }
            other => anyhow::bail!("unknown tool: {other}"),
        }
    }

    fn require_write_mode(&self, tool: &str) -> anyhow::Result<()> {
        if self.read_only {
            anyhow::bail!("tool {tool} is disabled in read-only mode");
        }
        Ok(())
    }

    async fn send_ui_value<F>(&self, send: F) -> anyhow::Result<Value>
    where
        F: FnOnce(oneshot::Sender<Value>) -> McpCommand,
    {
        let Some(ui_tx) = &self.ui_tx else {
            anyhow::bail!("ui bridge unavailable");
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        ui_tx
            .unbounded_send(send(reply_tx))
            .map_err(|_| anyhow::anyhow!("ui bridge unavailable"))?;
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .context("ui command timed out")?
            .context("ui bridge dropped reply")
    }

    async fn send_ui_ok<F>(&self, send: F) -> anyhow::Result<Value>
    where
        F: FnOnce(oneshot::Sender<anyhow::Result<()>>) -> McpCommand,
    {
        let Some(ui_tx) = &self.ui_tx else {
            anyhow::bail!("ui bridge unavailable");
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        ui_tx
            .unbounded_send(send(reply_tx))
            .map_err(|_| anyhow::anyhow!("ui bridge unavailable"))?;
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .context("ui command timed out")?
            .context("ui bridge dropped reply")??;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn send_ui_result<F>(&self, send: F) -> anyhow::Result<Value>
    where
        F: FnOnce(oneshot::Sender<anyhow::Result<Value>>) -> McpCommand,
    {
        let Some(ui_tx) = &self.ui_tx else {
            anyhow::bail!("ui bridge unavailable");
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        ui_tx
            .unbounded_send(send(reply_tx))
            .map_err(|_| anyhow::anyhow!("ui bridge unavailable"))?;
        let value = tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .context("ui command timed out")?
            .context("ui bridge dropped reply")??;
        Ok(value)
    }

    async fn get_app_info(&self) -> anyhow::Result<Value> {
        let context = self.get_current_context().await.unwrap_or_else(|_| {
            serde_json::json!({
                "route": null,
                "auth": "unknown"
            })
        });
        Ok(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "mcp_status": null,
            "auth": context.get("auth").cloned().unwrap_or(Value::Null),
            "route": context.get("route").cloned().unwrap_or(Value::Null),
            "user_id": context.get("user_id").cloned().unwrap_or(Value::Null),
        }))
    }

    async fn get_connection_status(&self) -> anyhow::Result<Value> {
        let status = self.api.connection_status();
        let value = match status {
            ConnectionStatus::Disconnected => "disconnected",
            ConnectionStatus::Connecting => "connecting",
            ConnectionStatus::Connected => "connected",
        };
        Ok(serde_json::json!({ "status": value }))
    }

    async fn list_clans(&self) -> anyhow::Result<Value> {
        let clans = self.api.list_clan_descs().await?;
        #[derive(Serialize)]
        struct ClanSummary {
            id: i64,
            name: String,
        }
        let items: Vec<ClanSummary> = clans
            .into_iter()
            .map(|clan| ClanSummary {
                id: clan.clan_id,
                name: clan.clan_name,
            })
            .collect();
        to_json(&items)
    }

    async fn list_channels(&self, clan_id: i64) -> anyhow::Result<Value> {
        let channels = self.api.list_channel_descs(clan_id, 0).await?;
        #[derive(Serialize)]
        struct ChannelSummary {
            id: i64,
            label: String,
            channel_type: u32,
        }
        let items: Vec<ChannelSummary> = channels
            .into_iter()
            .map(|channel| ChannelSummary {
                id: channel.channel_id,
                label: channel.channel_label,
                channel_type: channel.channel_type,
            })
            .collect();
        to_json(&items)
    }

    async fn list_dm_channels(&self) -> anyhow::Result<Value> {
        let channels = self.api.list_dm_channels(1).await?;
        #[derive(Serialize)]
        struct DmSummary {
            channel_id: i64,
            label: String,
            channel_type: u32,
            member_count: i32,
            unread: i32,
        }
        let items: Vec<DmSummary> = channels
            .into_iter()
            .map(|channel| DmSummary {
                channel_id: channel.channel_id,
                label: channel.channel_label,
                channel_type: channel.channel_type,
                member_count: channel.member_count,
                unread: channel.count_mess_unread,
            })
            .collect();
        to_json(&items)
    }

    async fn list_friends(&self) -> anyhow::Result<Value> {
        let friends = self.api.list_friends().await?;
        #[derive(Serialize)]
        struct FriendSummary {
            user_id: i64,
            username: String,
            display_name: Option<String>,
            state: i32,
        }
        let items: Vec<FriendSummary> = friends
            .into_iter()
            .map(|friend| FriendSummary {
                user_id: friend.account.user_id,
                username: friend.account.username,
                display_name: friend.account.display_name,
                state: friend.state,
            })
            .collect();
        to_json(&items)
    }

    async fn get_account(&self) -> anyhow::Result<Value> {
        let account = self.api.get_account().await?;
        to_json(&account)
    }

    async fn list_channel_members(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        if let Some(channel_id) = optional_i64_field(arguments, "channel_id") {
            let channel_type = arguments
                .get("channel_type")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32;
            let users = self
                .api
                .list_channel_users(clan_id, channel_id, channel_type)
                .await?;
            #[derive(Serialize)]
            struct MemberSummary {
                user_id: i64,
                id: i64,
                clan_nick: String,
                role_ids: Vec<i64>,
            }
            let items: Vec<MemberSummary> = users
                .into_iter()
                .take(100)
                .map(|user| MemberSummary {
                    user_id: user.user_id,
                    id: user.id,
                    clan_nick: user.clan_nick,
                    role_ids: user.role_id,
                })
                .collect();
            to_json(&items)
        } else {
            let users = self.api.list_clan_users(clan_id).await?;
            #[derive(Serialize)]
            struct ClanMemberSummary {
                user_id: i64,
                username: String,
                display_name: String,
                clan_nick: String,
                role_ids: Vec<i64>,
            }
            let items: Vec<ClanMemberSummary> = users
                .into_iter()
                .take(100)
                .filter_map(|user| {
                    let profile = user.user?;
                    Some(ClanMemberSummary {
                        user_id: profile.id,
                        username: profile.username,
                        display_name: profile.display_name,
                        clan_nick: user.clan_nick,
                        role_ids: user.role_id,
                    })
                })
                .collect();
            to_json(&items)
        }
    }

    async fn list_threads(&self, clan_id: i64, channel_id: i64) -> anyhow::Result<Value> {
        let threads = self
            .api
            .list_thread_descs(&channel_id.to_string(), &clan_id.to_string(), 0)
            .await?;
        to_json(&threads)
    }

    async fn list_pinned_messages(&self, clan_id: i64, channel_id: i64) -> anyhow::Result<Value> {
        let pins = self
            .api
            .get_pin_messages_list(&channel_id.to_string(), &clan_id.to_string())
            .await?;
        to_json(&pins)
    }

    async fn list_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> anyhow::Result<Value> {
        let result = self
            .api
            .list_channel_messages(clan_id, channel_id, message_id, direction, limit)
            .await?;
        #[derive(Serialize)]
        struct MessageSummary {
            id: i64,
            content: String,
            sender_id: i64,
            create_time: i64,
        }
        let items: Vec<MessageSummary> = result
            .messages
            .into_iter()
            .map(|message| MessageSummary {
                id: message.message_id,
                content: message.content,
                sender_id: message.sender_id,
                create_time: message.create_time,
            })
            .collect();
        to_json(&items)
    }

    async fn get_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> anyhow::Result<Value> {
        let message = self
            .fetch_channel_message(clan_id, channel_id, message_id)
            .await?;
        to_json(&message_detail(&message))
    }

    async fn search_messages(&self, query: &str, size: i32) -> anyhow::Result<Value> {
        use mezon_client::search_message::{build_search_request, content_filter};
        let request = build_search_request(
            vec![content_filter(query)],
            0,
            size,
            vec![mezon_proto::api::SortParam {
                field_name: "create_time".into(),
                order: "DESC".into(),
            }],
        );
        let response = self.api.search_message(request).await?;
        let messages: Vec<SearchSummary> = response
            .messages
            .iter()
            .map(search_summary_from_document)
            .collect();
        Ok(serde_json::json!({
            "total": response.total,
            "messages": messages,
        }))
    }

    async fn get_current_context(&self) -> anyhow::Result<Value> {
        self.send_ui_value(|reply| McpCommand::GetContext { reply })
            .await
    }

    async fn get_settings(&self) -> anyhow::Result<Value> {
        self.send_ui_value(|reply| McpCommand::GetSettings { reply })
            .await
    }

    async fn get_voice_status(&self) -> anyhow::Result<Value> {
        let settings = self.get_settings().await?;
        Ok(settings.get("voice").cloned().unwrap_or(serde_json::json!({
            "in_call": false,
            "channel_label": null,
            "mic_enabled": null,
            "camera_enabled": null,
        })))
    }

    async fn list_stickers(&self) -> anyhow::Result<Value> {
        let stickers = self.api.list_stickers_by_user_id().await?;
        let items: Vec<StickerSummary> = stickers.iter().map(sticker_summary).collect();
        to_json(&items)
    }

    async fn get_sticker(&self, arguments: &Value) -> anyhow::Result<Value> {
        let stickers = self.api.list_stickers_by_user_id().await?;
        if let Some(id) = optional_i64_field(arguments, "id") {
            let sticker = stickers
                .into_iter()
                .find(|s| s.id == id)
                .ok_or_else(|| anyhow::anyhow!("sticker not found: {id}"))?;
            return to_json(&sticker_summary(&sticker));
        }
        let name = arguments
            .get("name")
            .or_else(|| arguments.get("shortname"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("get_sticker requires id or name"))?;
        let sticker = stickers
            .into_iter()
            .find(|s| s.shortname.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow::anyhow!("sticker not found: {name}"))?;
        to_json(&sticker_summary(&sticker))
    }

    async fn get_image(&self, arguments: &Value) -> anyhow::Result<Value> {
        let url = if let Some(url) = arguments.get("url").and_then(Value::as_str) {
            url.to_string()
        } else {
            let clan_id = parse_i64_field(arguments, "clan_id")?;
            let channel_id = parse_i64_field(arguments, "channel_id")?;
            let message_id = parse_i64_field(arguments, "message_id")?;
            self.resolve_attachment_url(clan_id, channel_id, message_id, arguments)
                .await?
        };
        let (bytes, content_type) = mezon_client::transport_runtime::fetch_bytes(&url).await?;
        let mime = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
        Ok(serde_json::json!({
            "url": url,
            "mime": mime,
            "size": bytes.len(),
            "data_base64": BASE64.encode(bytes),
        }))
    }

    async fn resolve_attachment_url(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        arguments: &Value,
    ) -> anyhow::Result<String> {
        if let Some(url) = arguments.get("attachment_url").and_then(Value::as_str) {
            return Ok(url.to_string());
        }
        let message = self
            .fetch_channel_message(clan_id, channel_id, message_id)
            .await?;
        let index = arguments
            .get("attachment_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        message
            .attachments
            .get(index)
            .map(|att| att.url.clone())
            .ok_or_else(|| anyhow::anyhow!("attachment index {index} not found"))
    }

    async fn fetch_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> anyhow::Result<ApiMessage> {
        for anchor in [message_id, 0] {
            let result = self
                .api
                .list_channel_messages(clan_id, channel_id, anchor, 0, 50)
                .await?;
            if let Some(message) = result
                .messages
                .into_iter()
                .find(|m| m.message_id == message_id)
            {
                return Ok(message);
            }
        }
        anyhow::bail!("message not found: {message_id}")
    }

    async fn capture(&self, target: CaptureTarget) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::Capture { target, reply })
            .await
    }

    async fn pin_message(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        self.api
            .create_pin_message(message_id, channel_id, clan_id)
            .await?;
        Ok(serde_json::json!({ "ok": true, "message_id": message_id.to_string() }))
    }

    async fn unpin_message(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let pin_id = parse_i64_field(arguments, "pin_id")?;
        self.api
            .delete_pin_message(
                &pin_id.to_string(),
                &message_id.to_string(),
                &channel_id.to_string(),
                &clan_id.to_string(),
            )
            .await?;
        Ok(serde_json::json!({ "ok": true, "pin_id": pin_id.to_string() }))
    }

    async fn create_poll(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let question = arguments
            .get("question")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("create_poll requires string field question"))?
            .to_string();
        let answers: Vec<String> = arguments
            .get("answers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if answers.len() < 2 {
            anyhow::bail!("create_poll requires at least two answers");
        }
        let expire_hours = arguments
            .get("expire_hours")
            .and_then(Value::as_i64)
            .unwrap_or(24) as i32;
        let poll_type = arguments
            .get("poll_type")
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32;
        let response = self
            .api
            .create_poll(
                channel_id,
                clan_id,
                question,
                answers,
                expire_hours,
                poll_type,
            )
            .await?;
        Ok(serde_json::json!({
            "ok": true,
            "poll_id": response.poll_id.to_string(),
            "message_id": response.message_id.to_string(),
        }))
    }

    async fn vote_poll(&self, arguments: &Value) -> anyhow::Result<Value> {
        let poll_id = parse_i64_field(arguments, "poll_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let answers: Vec<i32> = arguments
            .get("answers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_i64().map(|value| value as i32))
                    .collect()
            })
            .unwrap_or_default();
        if answers.is_empty() {
            anyhow::bail!("vote_poll requires at least one answer index");
        }
        self.api
            .vote_poll(poll_id, message_id, channel_id, answers)
            .await?;
        Ok(serde_json::json!({ "ok": true, "poll_id": poll_id.to_string() }))
    }

    async fn get_scroll_state(&self) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::GetScrollState { reply })
            .await
    }

    async fn load_more_messages(&self, older: bool) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::LoadMoreMessages { older, reply })
            .await
    }

    async fn jump_to_message(&self, message_id: i64) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::JumpToMessage { message_id, reply })
            .await
    }

    async fn jump_to_present(&self) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::JumpToPresent { reply })
            .await
    }

    async fn get_user_status(&self) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::GetUserStatus { reply })
            .await
    }

    async fn get_member_list(&self) -> anyhow::Result<Value> {
        self.send_ui_result(|reply| McpCommand::GetMemberList { reply })
            .await
    }

    async fn set_user_status(&self, arguments: &Value) -> anyhow::Result<Value> {
        let status = arguments
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("set_user_status requires string field status"))?
            .to_string();
        let minutes = arguments
            .get("minutes")
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32;
        let until_turn_on = arguments
            .get("until_turn_on")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.send_ui_result(|reply| McpCommand::SetUserStatus {
            status,
            minutes,
            until_turn_on,
            reply,
        })
        .await
    }

    async fn navigate(&self, path: &str) -> anyhow::Result<Value> {
        validate_navigate_path(path)?;
        let Some(ui_tx) = &self.ui_tx else {
            anyhow::bail!("ui bridge unavailable");
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        ui_tx
            .unbounded_send(McpCommand::Navigate {
                path: path.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("ui bridge unavailable"))?;
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .context("navigate timed out")?
            .context("ui bridge dropped reply")??;
        Ok(serde_json::json!({ "ok": true, "path": path }))
    }

    async fn send_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        emojis: Vec<OutgoingEmoji>,
    ) -> anyhow::Result<Value> {
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        let message = self
            .api
            .send_channel_message(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                Vec::new(),
                Vec::new(),
                emojis,
                None,
            )
            .await?;
        Ok(serde_json::json!({
            "message_id": message.message_id,
        }))
    }

    async fn reply_to_message(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let parent_id = parse_i64_field(arguments, "message_id")?;
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("reply_to_message requires string field content"))?;
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        let reply = self
            .fetch_reply_reference(clan_id, channel_id, parent_id)
            .await?;
        let message = self
            .api
            .send_channel_message_reply(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                reply,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
                Default::default(),
            )
            .await?;
        Ok(serde_json::json!({
            "message_id": message.message_id,
        }))
    }

    async fn react_to_message(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let emoji = arguments
            .get("emoji")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("react_to_message requires string field emoji"))?;
        let remove = arguments
            .get("remove")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        let topic_id = optional_i64_field(arguments, "topic_id").unwrap_or(0);
        let message_sender_id = if let Some(id) = optional_i64_field(arguments, "message_sender_id")
        {
            id
        } else {
            self.fetch_message_sender_id(clan_id, channel_id, message_id)
                .await?
        };
        self.api
            .react_channel_message(
                clan_id,
                channel_id,
                message_id,
                0,
                emoji,
                1,
                message_sender_id,
                mode,
                is_public,
                remove,
                topic_id,
            )
            .await?;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn click_message_button(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let message = self
            .fetch_channel_message(clan_id, channel_id, message_id)
            .await?;
        let button_id = resolve_button_id(&message, arguments)?;
        let sender_id = optional_i64_field(arguments, "sender_id").unwrap_or(message.sender_id);
        let user_id = if let Some(id) = optional_i64_field(arguments, "user_id") {
            id
        } else {
            self.api.get_account().await?.user_id
        };
        let extra_data = arguments
            .get("extra_data")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "{}".to_string());
        self.resolve_channel_mode(clan_id, channel_id).await?;
        self.api
            .message_button_click(
                message_id,
                channel_id,
                &button_id,
                sender_id,
                user_id,
                &extra_data,
            )
            .await?;
        Ok(serde_json::json!({
            "ok": true,
            "message_id": message_id,
            "button_id": button_id,
        }))
    }

    async fn select_message_option(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let select_id = arguments
            .get("select_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!("select_message_option requires string field select_id")
            })?
            .to_string();
        let values: Vec<String> = arguments
            .get("values")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        self.resolve_channel_mode(clan_id, channel_id).await?;
        self.api
            .dropdown_box_selected(message_id, channel_id, &select_id)
            .await?;
        Ok(serde_json::json!({
            "ok": true,
            "message_id": message_id,
            "select_id": select_id,
            "values": values,
        }))
    }

    async fn edit_message(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("edit_message requires string field content"))?;
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        let topic_id = optional_i64_field(arguments, "topic_id").unwrap_or(0);
        let is_update_msg_topic = arguments
            .get("is_update_msg_topic")
            .and_then(Value::as_bool)
            .unwrap_or(topic_id != 0);
        self.api
            .update_channel_message(
                clan_id,
                channel_id,
                message_id,
                content,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                mode,
                is_public,
                topic_id,
                is_update_msg_topic,
                false,
                0,
            )
            .await?;
        Ok(serde_json::json!({ "ok": true, "message_id": message_id }))
    }

    async fn delete_message(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let message_id = parse_i64_field(arguments, "message_id")?;
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        let topic_id = optional_i64_field(arguments, "topic_id").unwrap_or(0);
        let message = self
            .fetch_channel_message(clan_id, channel_id, message_id)
            .await?;
        let has_attachment = !message.attachments.is_empty();
        let has_mentions = !message.entity_mentions.is_empty();
        let has_references = !message.references.is_empty();
        self.api
            .delete_channel_message(
                clan_id,
                channel_id,
                message_id,
                mode,
                is_public,
                has_attachment,
                topic_id,
                has_mentions,
                has_references,
            )
            .await?;
        Ok(serde_json::json!({ "ok": true, "message_id": message_id }))
    }

    async fn mark_as_read(&self, clan_id: i64, channel_id: i64) -> anyhow::Result<Value> {
        self.api.mark_as_read(channel_id, 0, clan_id).await?;
        Ok(serde_json::json!({ "ok": true }))
    }

    async fn send_image(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("");
        let source = arguments
            .get("path")
            .or_else(|| arguments.get("url"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("send_image requires path or url"))?;
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        if is_http_url(source) {
            let message = self
                .api
                .send_message_with_media(
                    clan_id,
                    channel_id,
                    content,
                    is_public,
                    mode,
                    &[source.to_string()],
                )
                .await?;
            return Ok(serde_json::json!({ "message_id": message.message_id }));
        }
        self.send_local_image(clan_id, channel_id, content, source, is_public, mode)
            .await
    }

    async fn send_local_image(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        path: &str,
        is_public: bool,
        mode: i32,
    ) -> anyhow::Result<Value> {
        let path = PathBuf::from(path);
        let upload = build_upload_file(&path).await?;
        let presigned = self.api.presign_file(upload).await?;
        let key = normalize_presign_key(&presigned.attachment.url);
        let attachment = presigned.attachment.clone();
        let sent = self
            .api
            .send_presigned_message(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                vec![attachment],
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(Vec::new()),
                Default::default(),
            )
            .await?;
        self.api.execute_upload(presigned).await?;
        self.api
            .update_presign_finish(
                clan_id,
                channel_id,
                sent.message_id,
                content,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![key],
                sent.create_time.max(0) as u32,
                mode,
                is_public,
                0,
                false,
            )
            .await?;
        Ok(serde_json::json!({ "message_id": sent.message_id }))
    }

    async fn send_sticker(&self, arguments: &Value) -> anyhow::Result<Value> {
        let clan_id = parse_i64_field(arguments, "clan_id")?;
        let channel_id = parse_i64_field(arguments, "channel_id")?;
        let sticker_url = arguments
            .get("sticker_url")
            .or_else(|| arguments.get("url"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("send_sticker requires sticker_url"))?;
        let filename = arguments
            .get("filename")
            .or_else(|| arguments.get("name"))
            .or_else(|| arguments.get("shortname"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let (clan_id, is_public, mode) = self.resolve_channel_mode(clan_id, channel_id).await?;
        let message = self
            .api
            .send_message_with_attachment_urls(
                clan_id,
                channel_id,
                is_public,
                mode,
                vec![UrlAttachment {
                    url: sticker_url.to_string(),
                    filename,
                    filetype: sticker_attachment_filetype(sticker_url),
                    width: 0,
                    height: 0,
                }],
                Default::default(),
            )
            .await?;
        Ok(serde_json::json!({ "message_id": message.message_id }))
    }

    async fn set_setting(&self, arguments: &Value) -> anyhow::Result<Value> {
        let key = arguments
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("set_setting requires string field key"))?;
        match key {
            "theme" | "language" | "zoom_factor" | "notifications_enabled" => {}
            other => anyhow::bail!("unsupported setting key: {other}"),
        }
        let value = arguments
            .get("value")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("set_setting requires field value"))?;
        let key = key.to_string();
        self.send_ui_ok(|reply| McpCommand::SetSetting { key, value, reply })
            .await
    }

    async fn set_cli_enabled(&self, enabled: bool) -> anyhow::Result<Value> {
        let Some(ui_tx) = &self.ui_tx else {
            anyhow::bail!("ui bridge unavailable");
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        ui_tx
            .unbounded_send(McpCommand::SetCliEnabled {
                enabled,
                reply: reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("ui bridge unavailable"))?;
        let installed = tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .context("set_cli_enabled timed out")?
            .context("ui bridge dropped reply")??;
        Ok(serde_json::json!({ "enabled": installed }))
    }

    async fn fetch_reply_reference(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> anyhow::Result<OutgoingReply> {
        let parent = self
            .fetch_channel_message(clan_id, channel_id, message_id)
            .await?;
        Ok(OutgoingReply {
            message_ref_id: parent.message_id,
            content: parent.content,
            has_attachment: !parent.attachments.is_empty(),
            message_sender_id: parent.sender_id,
            message_sender_username: parent.sender_name.clone(),
            message_sender_avatar: parent.avatar,
            message_sender_clan_nick: String::new(),
            message_sender_display_name: parent.sender_name,
        })
    }

    async fn fetch_message_sender_id(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> anyhow::Result<i64> {
        Ok(self
            .fetch_channel_message(clan_id, channel_id, message_id)
            .await?
            .sender_id)
    }

    async fn resolve_channel_mode(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> anyhow::Result<(i64, bool, i32)> {
        if clan_id == 0 {
            let (channel_type, mode) = self.resolve_direct_join(channel_id).await?;
            self.api
                .join_chat(0, channel_id, channel_type, false)
                .await?;
            Ok((0, false, mode))
        } else {
            self.api.join_clan_chat(clan_id).await?;
            let (is_public, join_type, mode) =
                self.resolve_clan_channel_join(clan_id, channel_id).await?;
            self.api
                .join_chat(clan_id, channel_id, join_type, is_public)
                .await?;
            Ok((clan_id, is_public, mode))
        }
    }

    async fn resolve_clan_channel_join(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> anyhow::Result<(bool, i32, i32)> {
        let desc = self
            .api
            .list_channel_descs(clan_id, 0)
            .await?
            .into_iter()
            .find(|c| c.channel_id == channel_id);
        Ok(match desc {
            Some(channel) => channel_join_params(
                channel.channel_type,
                channel.parent_id,
                channel.channel_private != 0,
            ),
            None => (true, CHANNEL_TYPE_CHANNEL, STREAM_MODE_CHANNEL),
        })
    }

    async fn resolve_direct_join(&self, channel_id: i64) -> anyhow::Result<(i32, i32)> {
        let context = self.get_current_context().await?;
        let route = context
            .get("route")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(rest) = route.strip_prefix("/chat/direct/message/") {
            let mut parts = rest.split('/');
            let Some(route_channel_id) = parts.next().and_then(|id| id.parse::<i64>().ok()) else {
                anyhow::bail!("invalid direct message route: {route}");
            };
            if route_channel_id != channel_id {
                anyhow::bail!(
                    "channel_id {channel_id} does not match current direct message context {route_channel_id}"
                );
            }
            let channel_type = parts
                .next()
                .and_then(|ty| ty.parse::<i32>().ok())
                .unwrap_or(3);
            let mode = match channel_type {
                2 => 3,
                _ => 4,
            };
            return Ok((channel_type, mode));
        }
        anyhow::bail!(
            "send_message with clan_id=0 requires the current route to be a direct message"
        )
    }
}

#[derive(Serialize)]
struct SearchSummary {
    message_id: String,
    channel_id: String,
    clan_id: String,
    content: String,
    sender_id: String,
    channel_label: String,
    create_time: String,
}

fn search_summary_from_document(doc: &SearchMessageDocument) -> SearchSummary {
    let content = parse_search_content(&doc.content);
    SearchSummary {
        message_id: doc.message_id.clone(),
        channel_id: doc.channel_id.clone(),
        clan_id: doc.clan_id.clone(),
        content,
        sender_id: doc.sender_id.clone(),
        channel_label: doc.channel_label.clone(),
        create_time: doc.create_time.clone(),
    }
}

fn parse_search_content(raw: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(raw)
        && let Some(text) = value.get("t").and_then(Value::as_str)
    {
        return text.to_string();
    }
    raw.to_string()
}

#[derive(Serialize)]
struct MessageDetail {
    id: i64,
    content: String,
    sender_id: i64,
    sender_name: String,
    create_time: i64,
    has_attachments: bool,
    embeds: Vec<EmbedSummary>,
    components: Vec<ComponentRowSummary>,
}

#[derive(Serialize)]
struct EmbedSummary {
    title: Option<String>,
    description: Option<String>,
    color: Option<String>,
    url: Option<String>,
    fields: Vec<EmbedFieldSummary>,
    footer: Option<String>,
    image_url: Option<String>,
    thumbnail_url: Option<String>,
}

#[derive(Serialize)]
struct EmbedFieldSummary {
    name: String,
    value: String,
    inline: bool,
}

#[derive(Serialize)]
struct ComponentRowSummary {
    components: Vec<ComponentSummary>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ComponentSummary {
    Button {
        id: Option<String>,
        label: String,
        style: i32,
        url: Option<String>,
        disabled: bool,
        interactive: bool,
    },
    Select {
        id: Option<String>,
        placeholder: Option<String>,
        disabled: bool,
        options: Vec<SelectOptionSummary>,
    },
    Other {
        component_type: i32,
    },
}

#[derive(Serialize)]
struct SelectOptionSummary {
    label: String,
    value: String,
    description: Option<String>,
    default: bool,
}

fn message_detail(message: &ApiMessage) -> MessageDetail {
    MessageDetail {
        id: message.message_id,
        content: message.content.clone(),
        sender_id: message.sender_id,
        sender_name: message.sender_name.clone(),
        create_time: message.create_time,
        has_attachments: !message.attachments.is_empty(),
        embeds: message
            .content_tokens
            .embed
            .iter()
            .map(embed_summary)
            .collect(),
        components: message
            .content_tokens
            .components
            .iter()
            .map(component_row_summary)
            .collect(),
    }
}

fn embed_summary(embed: &ApiEmbed) -> EmbedSummary {
    EmbedSummary {
        title: embed.title.clone(),
        description: embed.description.clone(),
        color: embed.color.clone(),
        url: embed.url.clone(),
        fields: embed
            .fields
            .iter()
            .map(|field| EmbedFieldSummary {
                name: field.name.clone(),
                value: field.value.clone(),
                inline: field.inline,
            })
            .collect(),
        footer: embed.footer.as_ref().map(|footer| footer.text.clone()),
        image_url: embed.image.as_ref().map(|image| image.url.clone()),
        thumbnail_url: embed.thumbnail.as_ref().map(|thumb| thumb.url.clone()),
    }
}

fn component_row_summary(row: &mezon_client::transport::ApiActionRow) -> ComponentRowSummary {
    ComponentRowSummary {
        components: row.components.iter().map(component_summary).collect(),
    }
}

fn component_summary(component: &ApiMessageComponent) -> ComponentSummary {
    match &component.component {
        ApiComponentPayload::Button(button) => ComponentSummary::Button {
            id: component.id.clone(),
            label: button.label.clone(),
            style: button.style.unwrap_or(1),
            url: button.url.clone(),
            disabled: button.disable,
            interactive: button.url.is_none() && !button.disable && component.id.is_some(),
        },
        ApiComponentPayload::Select(select) => ComponentSummary::Select {
            id: component.id.clone(),
            placeholder: select.placeholder.clone(),
            disabled: select.disabled,
            options: select
                .options
                .iter()
                .map(|option| SelectOptionSummary {
                    label: option.label.clone(),
                    value: option.value.clone(),
                    description: option.description.clone(),
                    default: option.default,
                })
                .collect(),
        },
        ApiComponentPayload::Other(_) => ComponentSummary::Other {
            component_type: component.component_type,
        },
    }
}

fn resolve_button_id(message: &ApiMessage, arguments: &Value) -> anyhow::Result<String> {
    if let Some(id) = arguments.get("button_id").and_then(Value::as_str) {
        return Ok(id.to_string());
    }
    let label = arguments
        .get("button_label")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!("click_message_button requires button_id or button_label")
        })?;
    for row in &message.content_tokens.components {
        for component in &row.components {
            if let ApiComponentPayload::Button(button) = &component.component
                && button.label == label
            {
                return component
                    .id
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("button '{label}' has no id"));
            }
        }
    }
    anyhow::bail!("button not found with label: {label}")
}

fn build_emoji_spans(content: &str, emojis: Option<&Value>) -> anyhow::Result<Vec<OutgoingEmoji>> {
    let Some(items) = emojis else {
        return Ok(Vec::new());
    };
    let items = items
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("send_message field emojis must be an array"))?;
    let mut spans = Vec::new();
    for item in items {
        let shortname = item
            .get("shortname")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("each emojis entry requires a non-empty shortname"))?;
        let emoji_id = item
            .get("emoji_id")
            .and_then(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| value.as_i64().map(|id| id.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("each emojis entry requires emoji_id"))?;
        let before = spans.len();
        let mut from = 0usize;
        while let Some(offset) = content[from..].find(shortname) {
            let start = from + offset;
            spans.push(OutgoingEmoji {
                emoji_id: emoji_id.clone(),
                s: start as i32,
                e: (start + shortname.len()) as i32,
            });
            from = start + shortname.len();
        }
        if spans.len() == before {
            anyhow::bail!("shortname {shortname} does not appear in content");
        }
    }
    spans.sort_by_key(|span| span.s);
    Ok(spans)
}

fn parse_i64_field(arguments: &Value, field: &str) -> anyhow::Result<i64> {
    let raw = arguments
        .get(field)
        .ok_or_else(|| anyhow::anyhow!("missing field {field}"))?;
    if let Some(value) = raw.as_i64() {
        return Ok(value);
    }
    raw.as_str()
        .ok_or_else(|| anyhow::anyhow!("field {field} must be a number or string"))?
        .parse::<i64>()
        .with_context(|| format!("invalid {field}"))
}

fn optional_i64_field(arguments: &Value, field: &str) -> Option<i64> {
    arguments.get(field).and_then(|raw| {
        raw.as_i64()
            .or_else(|| raw.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

fn validate_navigate_path(path: &str) -> anyhow::Result<()> {
    let path = path.trim();
    if path.is_empty() {
        anyhow::bail!("path must not be empty");
    }
    if path.contains("://") {
        anyhow::bail!("path must be an in-app route, not a URL");
    }
    if !path.starts_with('/') {
        anyhow::bail!("path must start with /");
    }
    if path.contains("..") {
        anyhow::bail!("path must not contain ..");
    }
    Ok(())
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn normalize_presign_key(key: &str) -> String {
    let segment = key
        .split('?')
        .next()
        .unwrap_or(key)
        .split('/')
        .rfind(|s| !s.is_empty())
        .unwrap_or(key);
    match segment.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < segment.len() => segment[..dot].to_string(),
        _ => segment.to_string(),
    }
}

async fn build_upload_file(path: &Path) -> anyhow::Result<UploadFile> {
    if !path.is_file() {
        anyhow::bail!("file not found: {}", path.display());
    }
    let raw_filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let filetype = match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "png" => "image/png".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        other => format!("image/{other}"),
    };
    Ok(UploadFile {
        path: path.to_path_buf(),
        filename: raw_filename,
        filetype,
        width: 0,
        height: 0,
        duration: 0,
        thumbnail: None,
    })
}

fn channel_join_params(channel_type: u32, parent_id: i64, private: bool) -> (bool, i32, i32) {
    let is_thread = channel_type == CHANNEL_TYPE_THREAD as u32 || parent_id != 0;
    if is_thread {
        (false, CHANNEL_TYPE_THREAD, STREAM_MODE_THREAD)
    } else {
        (!private, CHANNEL_TYPE_CHANNEL, STREAM_MODE_CHANNEL)
    }
}

fn sticker_attachment_filetype(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".wav") || lower.ends_with(".mp3") || lower.ends_with(".ogg") {
        "audio/mpeg".to_string()
    } else {
        "sticker".to_string()
    }
}

fn sticker_summary(sticker: &mezon_proto::api::ClanSticker) -> StickerSummary {
    StickerSummary {
        id: sticker.id,
        shortname: sticker.shortname.clone(),
        source: sticker.source.clone(),
        clan_id: sticker.clan_id,
        clan_name: sticker.clan_name.clone(),
        media_type: sticker.media_type,
    }
}

#[derive(Serialize)]
struct StickerSummary {
    id: i64,
    shortname: String,
    source: String,
    clan_id: i64,
    clan_name: String,
    media_type: i32,
}

fn to_json<T: Serialize>(value: &T) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_navigate_path_rejects_urls() {
        assert!(validate_navigate_path("http://evil.com").is_err());
        assert!(validate_navigate_path("/chat/clans/1/channels/2").is_ok());
    }

    #[test]
    fn emoji_spans_cover_every_occurrence() {
        let spans = build_emoji_spans(
            "hi :joy: there :joy:",
            Some(&serde_json::json!([{ "shortname": ":joy:", "emoji_id": "12" }])),
        )
        .expect("spans");
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].s, spans[0].e), (3, 8));
        assert_eq!((spans[1].s, spans[1].e), (15, 20));
    }

    #[test]
    fn emoji_spans_use_byte_offsets_like_the_composer() {
        let content = "chào :joy:";
        let spans = build_emoji_spans(
            content,
            Some(&serde_json::json!([{ "shortname": ":joy:", "emoji_id": "12" }])),
        )
        .expect("spans");
        let span = spans.first().expect("one span");
        assert_eq!(
            &content[span.s as usize..span.e as usize],
            ":joy:",
            "the composer indexes the input by byte offset, so a multi-byte prefix must not \
             shift the span onto the wrong characters"
        );
    }

    #[test]
    fn emoji_spans_reject_a_missing_shortname_that_shares_an_id_with_a_present_one() {
        assert!(
            build_emoji_spans(
                "only :joy: here",
                Some(&serde_json::json!([
                    { "shortname": ":joy:", "emoji_id": "12" },
                    { "shortname": ":absent:", "emoji_id": "12" },
                ])),
            )
            .is_err(),
            "checking for any span carrying this emoji_id lets a second entry pass on the \
             strength of the first one's match, so a shortname that is not in the text is \
             silently dropped instead of rejected"
        );
    }

    #[test]
    fn emoji_spans_reject_a_shortname_missing_from_content() {
        assert!(
            build_emoji_spans(
                "no emoji here",
                Some(&serde_json::json!([{ "shortname": ":joy:", "emoji_id": "12" }])),
            )
            .is_err(),
            "a span pointing outside the text would render as a stray emoji at offset 0"
        );
    }

    #[test]
    fn emoji_spans_default_to_empty() {
        assert!(
            build_emoji_spans("plain text", None)
                .expect("spans")
                .is_empty()
        );
    }
}
