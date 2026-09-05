use futures::StreamExt as _;
use gpui::{App, AppContext as _, AsyncApp};
use mezon_client::AppApi;
use mezon_mcp::{
    CaptureTarget, McpCommand, McpController, McpStartParams, ToolCallParams, is_write_tool,
    list_tools_json,
};
use mezon_native::control::{ControlHandler, ControlRequest, ControlResponse, ControlServer};
use mezon_store::{AuthState, LoginStore, Settings};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct McpRuntime {
    controller: Arc<McpController>,
    _control_server: Option<ControlServer>,
}

impl McpRuntime {
    pub fn controller(&self) -> Arc<McpController> {
        self.controller.clone()
    }

    pub fn start(
        api: Arc<AppApi>,
        mcp_cmd_tx: futures::channel::mpsc::UnboundedSender<McpCommand>,
    ) -> anyhow::Result<Self> {
        let controller = Arc::new(McpController::new());
        let controller_for_handler = controller.clone();
        let runtime_handle = mezon_client::transport_runtime::handle();
        let runtime_for_handler = runtime_handle.clone();

        let handler: ControlHandler = Arc::new(move |request| {
            let controller = controller_for_handler.clone();
            let request_id = request.id;
            match runtime_for_handler.block_on(handle_control_request(controller, request)) {
                Ok(response) => response,
                Err(error) => ControlResponse::err(request_id, error.to_string()),
            }
        });

        let control_server = if mezon_native::control::control_server_enabled() {
            let server = ControlServer::bind(handler)?;
            server.run_in_background();
            Some(server)
        } else {
            None
        };

        let controller_for_backend = controller.clone();
        let api_for_backend = api.clone();
        let ui_tx_for_backend = mcp_cmd_tx.clone();
        runtime_handle.spawn(async move {
            controller_for_backend
                .set_backend(api_for_backend, ui_tx_for_backend)
                .await;
        });

        Ok(Self {
            controller,
            _control_server: control_server,
        })
    }

    pub fn spawn_ui_bridge(
        cx: &mut App,
        auth_state: gpui::Entity<AuthState>,
        settings: gpui::Entity<Settings>,
        mut mcp_cmd_rx: futures::channel::mpsc::UnboundedReceiver<McpCommand>,
    ) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(command) = mcp_cmd_rx.next().await {
                match command {
                    McpCommand::GetContext { reply } => {
                        let value = cx.update(|cx| build_context(cx, &auth_state));
                        let _ = reply.send(value);
                    }
                    McpCommand::Navigate { path, reply } => {
                        let result = cx.update(|cx| navigate_path(cx, &path));
                        let _ = reply.send(result);
                    }
                    #[cfg(debug_assertions)]
                    McpCommand::SetChannelAgeRestricted {
                        clan_id,
                        channel_id,
                        on,
                        reply,
                    } => {
                        let clan_id = mezon_store::ClanId(clan_id);
                        let channel_id = mezon_store::ChannelId(channel_id);
                        let prepared = cx.update(|cx| {
                            let store = mezon_store::ChannelList::global(cx);
                            let channel = store.read(cx).channel(clan_id, channel_id).cloned();
                            channel.map(|channel| {
                                store.update(cx, |store, cx| {
                                    store.update_channel_overview(
                                        clan_id,
                                        channel_id,
                                        channel.name.to_string(),
                                        channel.topic.clone(),
                                        i32::from(on),
                                        cx,
                                    )
                                })
                            })
                        });
                        let result = match prepared {
                            Some(task) => match task.await {
                                Ok(()) => {
                                    Ok(serde_json::json!({ "ok": true, "age_restricted": on }))
                                }
                                Err(e) => Err(anyhow::anyhow!("update failed: {e:?}")),
                            },
                            None => Err(anyhow::anyhow!("channel not loaded")),
                        };
                        let _ = reply.send(result);
                    }
                    #[cfg(debug_assertions)]
                    McpCommand::SetLocalDob { seconds, reply } => {
                        let result = cx.update(|cx| {
                            mezon_store::AccountStore::global(cx).update(cx, |store, cx| {
                                match store.account.as_mut() {
                                    Some(account) => {
                                        account.dob_seconds = seconds;
                                        cx.notify();
                                        Ok(serde_json::json!({ "ok": true, "dob_seconds": seconds }))
                                    }
                                    None => Err(anyhow::anyhow!("account not loaded")),
                                }
                            })
                        });
                        let _ = reply.send(result);
                    }
                    #[cfg(debug_assertions)]
                    McpCommand::InjectPreviewMessage {
                        content,
                        sender_name,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_store::MessagesStore::global(cx).update(cx, |store, cx| {
                                store.inject_preview_message(content, sender_name, cx).map(
                                    |message_id| serde_json::json!({ "message_id": message_id }),
                                )
                            })
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::Logout { reply } => {
                        let result = cx.update(|cx| {
                            LoginStore::global(cx).update(cx, |store, cx| store.logout(cx));
                            Ok(())
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::Refresh { reply } => {
                        let result = cx.update(refresh_app);
                        let _ = reply.send(result);
                    }
                    McpCommand::Quit { reply } => {
                        let result = cx.update(|cx| {
                            cx.quit();
                            Ok(())
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ShowWindow { reply } => {
                        let result = cx.update(mezon_ui::app::capture::show_main_window);
                        let _ = reply.send(result);
                    }
                    McpCommand::Capture { target, reply } => {
                        let result = cx.update(|cx| match target {
                            CaptureTarget::Window => mezon_ui::app::capture::capture_png(cx, false),
                            CaptureTarget::Chat => mezon_ui::app::capture::capture_png(cx, true),
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::GoBack { reply } => {
                        let result = cx.update(mezon_ui::app::capture::go_back);
                        let _ = reply.send(result);
                    }
                    McpCommand::GoForward { reply } => {
                        let result = cx.update(mezon_ui::app::capture::go_forward);
                        let _ = reply.send(result);
                    }
                    McpCommand::GetSettings { reply } => {
                        let value = cx.update(|cx| build_settings(cx, &settings, &auth_state));
                        let _ = reply.send(value);
                    }
                    McpCommand::SetSetting { key, value, reply } => {
                        let result = cx.update(|cx| set_setting(cx, &settings, &key, value));
                        let _ = reply.send(result);
                    }
                    McpCommand::SetCliEnabled { enabled, reply } => {
                        let result = cx.update(|_| set_cli_enabled(enabled));
                        let _ = reply.send(result);
                    }
                    McpCommand::JoinVoice {
                        channel_id,
                        clan_id,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::join_voice(channel_id, clan_id, cx)
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::LeaveVoice { reply } => {
                        let result = cx.update(mezon_ui::app::capture::leave_voice);
                        let _ = reply.send(result);
                    }
                    McpCommand::GetRecordingState { reply } => {
                        let value = cx.update(mezon_ui::app::capture::recording_state);
                        let _ = reply.send(value);
                    }
                    McpCommand::StartRecording { path, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::start_recording(path, cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::StopRecording { reply } => {
                        let result = cx.update(mezon_ui::app::capture::stop_recording);
                        let _ = reply.send(result);
                    }
                    McpCommand::GetScrollState { reply } => {
                        let result = cx.update(scroll_state);
                        let _ = reply.send(result);
                    }
                    McpCommand::TourState { reply } => {
                        let result = cx.update(tour_state);
                        let _ = reply.send(result);
                    }
                    McpCommand::TourStart { track, reply } => {
                        let result = cx.update(|cx| tour_start(track.as_deref(), cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::TourAdvance { forward, reply } => {
                        let result = cx.update(|cx| tour_advance(forward, cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::SetPanel { kind, reply } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::set_composer_panel(cx, kind.as_deref())
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::OpenImageViewer {
                        message_id,
                        attachment_index,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::open_message_image_viewer(
                                &settings,
                                message_id,
                                attachment_index,
                                cx,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::OpenPdfViewer {
                        message_id,
                        attachment_index,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::open_message_pdf_viewer(
                                &settings,
                                message_id,
                                attachment_index,
                                cx,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ScrollWheel {
                        delta_y,
                        ticks,
                        reply,
                    } => {
                        let result = scroll_wheel(cx, delta_y, ticks).await;
                        let _ = reply.send(result);
                    }
                    McpCommand::ScrollMessages { to_top, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::scroll_messages(cx, to_top));
                        let _ = reply.send(result);
                    }
                    McpCommand::ComposerType { text, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::composer_type(cx, &text));
                        let _ = reply.send(result);
                    }
                    McpCommand::ComposerState { reply } => {
                        let result = cx.update(mezon_ui::app::capture::composer_state);
                        let _ = reply.send(result);
                    }
                    McpCommand::ComposerPick { index, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::composer_pick(cx, index));
                        let _ = reply.send(result);
                    }
                    McpCommand::ComposerSubmit { reply } => {
                        let result = cx.update(mezon_ui::app::capture::composer_submit);
                        let _ = reply.send(result);
                    }
                    McpCommand::OpenTopic { message_id, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::open_topic(cx, message_id));
                        let _ = reply.send(result);
                    }
                    McpCommand::CloseTopic { reply } => {
                        let result = cx.update(mezon_ui::app::capture::close_topic);
                        let _ = reply.send(result);
                    }
                    McpCommand::TopicState { reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::topic_state(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::TopicType { text, reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::topic_type(cx, &text));
                        let _ = reply.send(result);
                    }
                    McpCommand::TopicPick { index, reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::topic_pick(cx, index));
                        let _ = reply.send(result);
                    }
                    McpCommand::TopicDropPaths { paths, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::topic_drop_paths(cx, paths));
                        let _ = reply.send(result);
                    }
                    McpCommand::TopicSubmit { reply } => {
                        let result = cx.update(mezon_ui::app::capture::topic_submit);
                        let _ = reply.send(result);
                    }
                    McpCommand::TopicScrollWheel {
                        delta_y,
                        ticks,
                        reply,
                    } => {
                        let result = topic_scroll_wheel(cx, delta_y, ticks).await;
                        let _ = reply.send(result);
                    }
                    McpCommand::EditBegin { message_id, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::edit_begin(cx, message_id));
                        let _ = reply.send(result);
                    }
                    McpCommand::EditType { text, reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::edit_type(cx, &text));
                        let _ = reply.send(result);
                    }
                    McpCommand::EditPick { index, reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::edit_pick(cx, index));
                        let _ = reply.send(result);
                    }
                    McpCommand::EditState { reply } => {
                        let result = cx.update(mezon_ui::app::capture::edit_state);
                        let _ = reply.send(result);
                    }
                    McpCommand::EditSave { reply } => {
                        let result = cx.update(mezon_ui::app::capture::edit_save);
                        let _ = reply.send(result);
                    }
                    McpCommand::ComposerPanelSend {
                        kind,
                        url,
                        filename,
                        width,
                        height,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::composer_panel_send(
                                cx, &kind, url, filename, width, height,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ComposerDropPaths { paths, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::composer_drop_paths(cx, paths));
                        let _ = reply.send(result);
                    }
                    McpCommand::SendBuzz { text, reply } => {
                        let result = cx.update(|cx| send_buzz(cx, &text));
                        let _ = reply.send(result);
                    }
                    McpCommand::SendAttachment {
                        paths,
                        content,
                        anonymous,
                        reply_to,
                        reply,
                    } => {
                        let prepared = cx
                            .background_spawn(async move { outgoing_attachments(&paths) })
                            .await;
                        let result = match prepared {
                            Ok(attachments) => cx.update(|cx| {
                                send_attachment(
                                    cx,
                                    &auth_state,
                                    attachments,
                                    &content,
                                    anonymous,
                                    reply_to,
                                )
                            }),
                            Err(error) => Err(error),
                        };
                        let _ = reply.send(result);
                    }
                    McpCommand::ListEmojis {
                        clan_id,
                        query,
                        limit,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            list_emojis(cx, clan_id.as_deref(), query.as_deref(), limit)
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::LoadMoreMessages { older, reply } => {
                        let result = cx.update(|cx| load_more_messages(cx, older));
                        let _ = reply.send(result);
                    }
                    McpCommand::ListLoadedMessages {
                        limit,
                        topic,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::list_loaded_messages(cx, limit, topic)
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ReplyBegin { message_id, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::reply_begin(cx, message_id));
                        let _ = reply.send(result);
                    }
                    McpCommand::JumpToMessage { message_id, reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::jump_to_message(cx, message_id));
                        let _ = reply.send(result);
                    }
                    McpCommand::JumpToPresent { reply } => {
                        let result = cx.update(mezon_ui::app::capture::jump_to_present);
                        let _ = reply.send(result);
                    }
                    McpCommand::GetUserStatus { reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::user_status_snapshot(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::GetMemberList { reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::member_list_snapshot(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::ListBannedUsers {
                        clan_id,
                        channel_id,
                        reply,
                    } => {
                        let task = cx.update(|cx| {
                            mezon_ui::app::capture::banned_users_task(cx, clan_id, channel_id)
                        });
                        let _ = reply.send(task.await);
                    }
                    McpCommand::CloseModal { reply } => {
                        let result = cx.update(mezon_ui::app::capture::close_modal);
                        let _ = reply.send(result);
                    }
                    McpCommand::MemberMenuState { reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::member_menu_state(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::MemberMenuOpen {
                        user_id,
                        x,
                        y,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::member_menu_open(
                                cx,
                                mezon_store::UserId(user_id),
                                x,
                                y,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::MemberMenuClose { reply } => {
                        let result = cx.update(mezon_ui::app::capture::member_menu_close);
                        let _ = reply.send(result);
                    }
                    McpCommand::MemberMenuPick {
                        index,
                        value,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::member_menu_pick(cx, index, value)
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ClanMenuState { reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::clan_menu_state(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::ClanMenuOpen {
                        clan_id,
                        x,
                        y,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::clan_menu_open(
                                cx,
                                mezon_store::ClanId(clan_id),
                                x,
                                y,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ClanMenuClose { reply } => {
                        let result = cx.update(mezon_ui::app::capture::clan_menu_close);
                        let _ = reply.send(result);
                    }
                    McpCommand::ClanMenuPick {
                        index,
                        value,
                        reply,
                    } => {
                        let result = cx
                            .update(|cx| mezon_ui::app::capture::clan_menu_pick(cx, index, value));
                        let _ = reply.send(result);
                    }
                    McpCommand::ListCategories { clan_id, reply } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::list_categories(
                                cx,
                                mezon_store::ClanId(clan_id),
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::CreateCategory {
                        clan_id,
                        name,
                        reply,
                    } => {
                        let task = cx.update(|cx| {
                            mezon_ui::app::capture::create_category_task(
                                cx,
                                mezon_store::ClanId(clan_id),
                                name,
                            )
                        });
                        let _ = reply.send(task.await);
                    }
                    McpCommand::ChannelMenuState { reply } => {
                        let result = cx.update(|cx| mezon_ui::app::capture::channel_menu_state(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::ChannelMenuOpen {
                        clan_id,
                        channel_id,
                        x,
                        y,
                        in_favorites,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::channel_menu_open(
                                cx,
                                mezon_store::ClanId(clan_id),
                                mezon_store::ChannelId(channel_id),
                                x,
                                y,
                                in_favorites,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::ChannelMenuClose { reply } => {
                        let result = cx.update(mezon_ui::app::capture::channel_menu_close);
                        let _ = reply.send(result);
                    }
                    McpCommand::ChannelMenuPick {
                        index,
                        value,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::channel_menu_pick(cx, index, value)
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::CategoryMenuState { reply } => {
                        let result =
                            cx.update(|cx| mezon_ui::app::capture::category_menu_state(cx));
                        let _ = reply.send(result);
                    }
                    McpCommand::CategoryMenuOpen {
                        clan_id,
                        category_id,
                        x,
                        y,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::category_menu_open(
                                cx,
                                mezon_store::ClanId(clan_id),
                                category_id,
                                x,
                                y,
                            )
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::CategoryMenuClose { reply } => {
                        let result = cx.update(mezon_ui::app::capture::category_menu_close);
                        let _ = reply.send(result);
                    }
                    McpCommand::CategoryMenuPick {
                        index,
                        value,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::category_menu_pick(cx, index, value)
                        });
                        let _ = reply.send(result);
                    }
                    McpCommand::CreateClan { name, logo, reply } => {
                        let task = cx
                            .update(|cx| mezon_ui::app::capture::create_clan_task(cx, name, logo));
                        let _ = reply.send(task.await);
                    }
                    McpCommand::OpenCreateClanModal { reply } => {
                        let result = cx.update(mezon_ui::app::capture::open_create_clan_modal);
                        let _ = reply.send(result);
                    }
                    McpCommand::SetUserStatus {
                        status,
                        minutes,
                        until_turn_on,
                        reply,
                    } => {
                        let result = cx.update(|cx| {
                            mezon_ui::app::capture::set_user_status(
                                cx,
                                &status,
                                minutes,
                                until_turn_on,
                            )
                        });
                        let _ = reply.send(result);
                    }
                }
            }
        })
        .detach();
    }
}

async fn handle_control_request(
    controller: Arc<McpController>,
    request: ControlRequest,
) -> anyhow::Result<ControlResponse> {
    match request.method.as_str() {
        "mcp.start" => {
            let params: McpStartParams = serde_json::from_value(request.params)?;
            let result = controller.start(params.read_only, params.port).await?;
            Ok(ControlResponse::ok(
                request.id,
                serde_json::to_value(result)?,
            ))
        }
        "mcp.status" => {
            let status = controller.status().await;
            Ok(ControlResponse::ok(
                request.id,
                serde_json::to_value(status)?,
            ))
        }
        "mcp.stop" => {
            controller.stop().await?;
            Ok(ControlResponse::ok(request.id, Value::Null))
        }
        "tool.call" => {
            let params: ToolCallParams = serde_json::from_value(request.params)?;
            let read_only = controller.status().await.read_only;
            if read_only && is_write_tool(&params.name) {
                return Ok(ControlResponse::err(
                    request.id,
                    format!("Tool {} is disabled in read-only mode", params.name),
                ));
            }
            let result = controller.call_tool(&params.name, params.arguments).await?;
            Ok(ControlResponse::ok(request.id, result))
        }
        "tools.list" => {
            let read_only = request
                .params
                .get("read_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(ControlResponse::ok(request.id, list_tools_json(read_only)))
        }
        "app.logout" => {
            let result = controller.call_tool("logout", Value::Null).await?;
            Ok(ControlResponse::ok(request.id, result))
        }
        "app.refresh" => {
            let result = controller.call_tool("refresh", Value::Null).await?;
            Ok(ControlResponse::ok(request.id, result))
        }
        "app.quit" => {
            let result = controller.call_tool("quit_app", Value::Null).await?;
            Ok(ControlResponse::ok(request.id, result))
        }
        "app.status" => {
            let result = controller.call_tool("get_app_info", Value::Null).await?;
            Ok(ControlResponse::ok(request.id, result))
        }
        other => Ok(ControlResponse::err(
            request.id,
            format!("unknown control method: {other}"),
        )),
    }
}

fn list_emojis(
    cx: &mut App,
    clan_id: Option<&str>,
    query: Option<&str>,
    limit: usize,
) -> anyhow::Result<Value> {
    let store = mezon_store::EmojiStore::global(cx);
    let store = store.read(cx);
    let needle = query.map(str::to_lowercase);
    let items: Vec<Value> = store
        .all()
        .into_iter()
        .filter(|emoji| clan_id.is_none_or(|id| emoji.clan_id == id))
        .filter(|emoji| {
            needle
                .as_deref()
                .is_none_or(|needle| emoji.shortname.to_lowercase().contains(needle))
        })
        .take(limit)
        .map(|emoji| {
            json!({
                "emoji_id": emoji.id,
                "shortname": emoji.shortname,
                "category": emoji.category,
                "clan_id": emoji.clan_id,
            })
        })
        .collect();
    Ok(json!({ "count": items.len(), "emojis": items }))
}

async fn scroll_wheel(cx: &mut AsyncApp, delta_y: f32, ticks: u32) -> anyhow::Result<Value> {
    use mezon_ui::app::capture::{
        WHEEL_MAX_TICKS, WHEEL_TICK_INTERVAL, dispatch_wheel_tick, message_viewport_state,
        prime_wheel_pointer,
    };
    let ticks = ticks.clamp(1, WHEEL_MAX_TICKS);
    cx.update(prime_wheel_pointer)?;
    cx.background_executor().timer(WHEEL_TICK_INTERVAL).await;
    let before = cx.update(|cx| message_viewport_state(cx));
    let mut delivered = 0u32;
    let mut consumed = 0u32;
    for tick in 0..ticks {
        if cx.update(|cx| dispatch_wheel_tick(cx, delta_y))? {
            consumed += 1;
        }
        delivered += 1;
        if tick + 1 < ticks {
            cx.background_executor().timer(WHEEL_TICK_INTERVAL).await;
        }
    }
    let after = cx.update(|cx| message_viewport_state(cx));
    let (item_count, first_visible, at_bottom) = after.unwrap_or((0, 0, false));
    Ok(json!({
        "ok": true,
        "ticks": delivered,
        "consumed_ticks": consumed,
        "moved": before != after,
        "delta_y": delta_y,
        "item_count": item_count,
        "first_visible_index": first_visible,
        "at_bottom": at_bottom,
    }))
}

async fn topic_scroll_wheel(cx: &mut AsyncApp, delta_y: f32, ticks: u32) -> anyhow::Result<Value> {
    use mezon_ui::app::capture::{
        WHEEL_MAX_TICKS, WHEEL_TICK_INTERVAL, dispatch_topic_wheel_tick, prime_topic_wheel_pointer,
        topic_viewport_state,
    };
    let ticks = ticks.clamp(1, WHEEL_MAX_TICKS);
    cx.update(prime_topic_wheel_pointer)?;
    cx.background_executor().timer(WHEEL_TICK_INTERVAL).await;
    let before = cx.update(|cx| topic_viewport_state(cx));
    let mut delivered = 0u32;
    let mut consumed = 0u32;
    for tick in 0..ticks {
        if cx.update(|cx| dispatch_topic_wheel_tick(cx, delta_y))? {
            consumed += 1;
        }
        delivered += 1;
        if tick + 1 < ticks {
            cx.background_executor().timer(WHEEL_TICK_INTERVAL).await;
        }
    }
    let after = cx.update(|cx| topic_viewport_state(cx));
    let (item_count, first_visible, at_bottom) = after.unwrap_or((0, 0, false));
    Ok(json!({
        "ok": true,
        "ticks": delivered,
        "consumed_ticks": consumed,
        "moved": before != after,
        "delta_y": delta_y,
        "item_count": item_count,
        "first_visible_index": first_visible,
        "at_bottom": at_bottom,
    }))
}

fn tour_state(cx: &mut App) -> anyhow::Result<Value> {
    let Some(entity) = mezon_ui::tour::TourState::try_global(cx) else {
        return Ok(json!({ "active": false }));
    };
    let status = entity.read(cx).status(cx);
    Ok(match status {
        None => json!({ "active": false }),
        Some(status) => json!({
            "active": true,
            "resolving": status.resolving,
            "hole": status.hole.map(|(x, y, w, h)| json!([x, y, w, h])),
            "track": status.track,
            "index": status.index,
            "position": status.position,
            "total": status.total,
            "title_key": status.title_key,
            "anchor": status.anchor,
            "has_hole": status.has_hole,
        }),
    })
}

fn tour_start(track: Option<&str>, cx: &mut App) -> anyhow::Result<Value> {
    match mezon_ui::tour::mcp_start(track, cx)? {
        Some(id) => Ok(json!({ "ok": true, "track": id })),
        None => Ok(json!({
            "ok": false,
            "reason": "no track matched this route, it is already done, or a tour is already running",
        })),
    }
}

fn tour_advance(forward: bool, cx: &mut App) -> anyhow::Result<Value> {
    match mezon_ui::tour::mcp_advance(forward, cx)? {
        Some(advance) => Ok(json!({
            "ok": true,
            "moved": advance.moved,
            "active": advance.still_active,
        })),
        None => Ok(json!({
            "ok": false,
            "moved": false,
            "active": false,
            "reason": "no tour is running",
        })),
    }
}

fn scroll_state(cx: &mut App) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    let store = store.read(cx);
    Ok(json!({
        "channel_id": store.active_channel_id().map(|id| id.to_string()),
        "loaded_count": store.messages().len(),
        "has_more_top": store.has_more_top(),
        "has_more_bottom": store.has_more_bottom(),
        "loading": store.is_loading(),
        "loading_more": store.is_loading_more(),
    }))
}

fn load_more_messages(cx: &mut App, older: bool) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    let (started, before) = store.update(cx, |store, cx| {
        let before = store.messages().len();
        let started = if older {
            store.load_more(cx)
        } else {
            store.load_more_bottom(cx)
        };
        (started, before)
    });
    let store = store.read(cx);
    Ok(json!({
        "started": started,
        "direction": if older { "older" } else { "newer" },
        "loaded_count_before": before,
        "has_more_top": store.has_more_top(),
        "has_more_bottom": store.has_more_bottom(),
    }))
}

fn send_buzz(cx: &mut App, text: &str) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    if store.read(cx).is_anonymous_mode() {
        anyhow::bail!("buzz is blocked in anonymous mode");
    }
    store.update(cx, |store, cx| {
        store.send_buzz_message(text.to_string(), cx);
    });
    Ok(json!({ "ok": true, "text": text }))
}

fn outgoing_attachments(paths: &[String]) -> anyhow::Result<Vec<mezon_store::OutgoingAttachment>> {
    paths
        .iter()
        .map(|path| {
            let path = std::path::PathBuf::from(path);
            if !path.is_file() {
                anyhow::bail!("file not found: {}", path.display());
            }
            let pending = mezon_ui::chat::mention_input::build_pending(path.clone())
                .ok_or_else(|| anyhow::anyhow!("unreadable attachment: {}", path.display()))?;
            Ok(mezon_store::OutgoingAttachment {
                path: pending.path,
                filename: pending.filename,
                filetype: pending.filetype,
                width: i32::try_from(pending.width).unwrap_or(0),
                height: i32::try_from(pending.height).unwrap_or(0),
                duration: pending.duration,
                poster_jpeg: pending.poster_jpeg,
            })
        })
        .collect()
}

fn send_attachment(
    cx: &mut App,
    auth_state: &gpui::Entity<AuthState>,
    attachments: Vec<mezon_store::OutgoingAttachment>,
    content: &str,
    anonymous: bool,
    reply_to: i64,
) -> anyhow::Result<Value> {
    let filenames: Vec<String> = attachments.iter().map(|att| att.filename.clone()).collect();
    let (user_id, username) = match auth_state.read(cx) {
        AuthState::Authenticated(session) => (session.user_id.clone(), session.username.clone()),
        _ => anyhow::bail!("not signed in"),
    };
    let store = mezon_store::MessagesStore::global(cx);
    let reply_draft = (reply_to != 0)
        .then(|| {
            store
                .read(cx)
                .reply_draft_for(mezon_store::MessageId(reply_to))
        })
        .flatten();
    if reply_to != 0 && reply_draft.is_none() {
        anyhow::bail!("message {reply_to} is not loaded in the active channel");
    }
    store.update(cx, |store, cx| {
        if store.is_anonymous_mode() != anonymous {
            store.toggle_anonymous_mode(cx);
        }
        if store.is_anonymous_mode() != anonymous {
            anyhow::bail!("cannot set anonymous={anonymous} for the active channel");
        }
        store.send_message_with_payload(
            content.to_string(),
            user_id,
            username,
            mezon_store::OutgoingContent::default(),
            attachments,
            None,
            reply_draft,
            anonymous,
            0,
            cx,
        );
        Ok(json!({
            "ok": true,
            "attachments": filenames,
            "content": content,
            "anonymous": anonymous,
            "reply_to": reply_to,
        }))
    })
}

fn build_context(cx: &App, auth_state: &gpui::Entity<AuthState>) -> Value {
    let router = mezon_ui::router::Router::global(cx).read(cx);
    let route = router.current_path();
    let auth = match auth_state.read(cx) {
        AuthState::NotAuthenticated => "not_authenticated",
        AuthState::AwaitingCallback => "awaiting_callback",
        AuthState::Connecting(_) => "connecting",
        AuthState::Authenticated(session) => {
            return json!({
                "route": route,
                "auth": "authenticated",
                "user_id": session.user_id,
                "clan_id": parse_route_field(&route, "clan"),
                "channel_id": parse_route_field(&route, "channel").or_else(|| parse_route_field(&route, "direct")),
            });
        }
        AuthState::OtpRequested { .. } => "otp_requested",
    };
    json!({
        "route": route,
        "auth": auth,
        "clan_id": parse_route_field(&route, "clan"),
        "channel_id": parse_route_field(&route, "channel").or_else(|| parse_route_field(&route, "direct")),
    })
}

fn parse_route_field(route: &str, kind: &str) -> Option<String> {
    let segments: Vec<_> = route.split('/').filter(|s| !s.is_empty()).collect();
    match kind {
        "clan" => segments
            .windows(2)
            .find(|w| w[0] == "clans")
            .map(|w| w[1].to_string()),
        "channel" => segments
            .windows(2)
            .find(|w| w[0] == "channels")
            .map(|w| w[1].to_string()),
        "direct" => segments
            .windows(2)
            .find(|w| w[0] == "message")
            .map(|w| w[1].to_string()),
        _ => None,
    }
}

fn navigate_path(cx: &mut App, path: &str) -> anyhow::Result<()> {
    let route = mezon_ui::router::Route::from_path(path);
    mezon_ui::router::navigate(cx, route);
    Ok(())
}

fn refresh_app(cx: &mut App) -> anyhow::Result<()> {
    if let Some(store) = mezon_store::ClanList::try_global(cx) {
        store.update(cx, |store, cx| store.reload(cx));
    }
    if let Some(store) = mezon_store::DirectMessageStore::try_global(cx) {
        store.update(cx, |store, cx| store.refresh(cx));
    }
    if let Some(store) = mezon_store::MessagesStore::try_global(cx) {
        store.update(cx, |store, cx| store.refresh(cx));
    }
    Ok(())
}

fn build_settings(
    cx: &App,
    settings: &gpui::Entity<Settings>,
    auth_state: &gpui::Entity<AuthState>,
) -> Value {
    let s = settings.read(cx);
    let voice = mezon_store::VoiceStore::try_global(cx).map(|store| {
        let store = store.read(cx);
        json!({
            "in_call": store.connection().active_channel_id().is_some(),
            "channel_label": store.channel_label(),
            "mic_enabled": store.mic_enabled(),
            "camera_enabled": store.camera_enabled(),
        })
    });
    json!({
        "theme": s.theme,
        "language": s.language,
        "zoom_factor": s.zoom_factor,
        "notifications_enabled": s.notifications_enabled,
        "activity_tracking": s.activity_tracking,
        "hardware_acceleration": s.hardware_acceleration,
        "cli_enabled": mezon_native::cli_install::is_installed(),
        "cli_visible": mezon_native::cli_install::menu_visible(),
        "auth": match auth_state.read(cx) {
            AuthState::Authenticated(_) => "authenticated",
            _ => "not_authenticated",
        },
        "voice": voice,
    })
}

fn set_setting(
    cx: &mut App,
    settings: &gpui::Entity<Settings>,
    key: &str,
    value: Value,
) -> anyhow::Result<()> {
    match key {
        "theme" => {
            let Some(theme) = value.as_str() else {
                anyhow::bail!("theme must be a string");
            };
            let entity = settings.clone();
            settings.update(cx, |settings, cx| {
                settings.theme = theme.to_string();
                mezon_store::schedule_settings_save(&entity, cx);
                cx.notify();
            });
        }
        "language" => {
            let Some(language) = value.as_str() else {
                anyhow::bail!("language must be a string");
            };
            let entity = settings.clone();
            settings.update(cx, |settings, cx| {
                settings.language = language.to_string();
                mezon_store::schedule_settings_save(&entity, cx);
                cx.notify();
            });
        }
        "zoom_factor" => {
            let Some(zoom) = value.as_f64() else {
                anyhow::bail!("zoom_factor must be a number");
            };
            let entity = settings.clone();
            settings.update(cx, |settings, cx| {
                settings.zoom_factor = zoom as f32;
                mezon_store::schedule_settings_save(&entity, cx);
                cx.notify();
            });
        }
        "notifications_enabled" | "activity_tracking" => {
            let Some(enabled) = value.as_bool() else {
                anyhow::bail!("{key} must be a boolean");
            };
            let entity = settings.clone();
            settings.update(cx, |settings, cx| {
                if key == "notifications_enabled" {
                    settings.notifications_enabled = enabled;
                } else {
                    settings.activity_tracking = enabled;
                }
                mezon_store::schedule_settings_save(&entity, cx);
                cx.notify();
            });
        }
        other => anyhow::bail!("unsupported setting key: {other}"),
    }
    Ok(())
}

fn set_cli_enabled(enabled: bool) -> anyhow::Result<bool> {
    if enabled {
        mezon_native::cli_install::install()?;
        Ok(true)
    } else {
        mezon_native::cli_install::uninstall()?;
        Ok(false)
    }
}
