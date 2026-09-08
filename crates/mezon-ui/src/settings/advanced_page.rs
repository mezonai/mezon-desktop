use crate::components::primitives::{
    Icon, IconName, Label, Switch, TextArea, TextAreaEvent, TextAreaField, h_flex, v_flex,
};
use crate::theme::ActiveTheme;
use gpui::{
    ClipboardItem, Context, Entity, Focusable as _, Subscription, Window, div, prelude::*, px,
};
use mezon_store::{McpServerStatus, PlatformStore, Settings};
use std::sync::OnceLock;
use std::time::Duration;
use ui::Tooltip;

const MCP_STDIO_CONFIG: &str = r#"{
  "mcpServers": {
    "mezon": {
      "type": "stdio",
      "command": {stdio_command},
      "args": {stdio_args}
    }
  }
}"#;

const MCP_HTTP_CONFIG: &str = r#"{
  "mcpServers": {
    "mezon-http": {
      "type": "http",
      "url": "http://127.0.0.1:{port}/mcp"
    }
  }
}"#;

const QUOTED_CLI_NAME: &str = "\"mezon\"";

static STDIO_COMMAND_PATH: OnceLock<String> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum McpCopyTarget {
    Url,
    HttpConfig,
    StdioConfig,
}

pub struct AdvancedPage {
    settings: Entity<Settings>,
    cli_busy: bool,
    mcp_busy: bool,
    mcp_status: McpServerStatus,
    mcp_copy_feedback: Option<McpCopyTarget>,
    mcp_port_input: Option<Entity<TextArea>>,
    _subscriptions: Vec<Subscription>,
}

impl AdvancedPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |this, _, cx| {
            this.refresh_mcp_status(cx);
            cx.notify();
        })
        .detach();
        let mcp_status = PlatformStore::try_global(cx)
            .map(|platform| platform.read(cx).mcp_server_status())
            .unwrap_or_default();
        Self {
            settings,
            cli_busy: false,
            mcp_busy: false,
            mcp_status,
            mcp_copy_feedback: None,
            mcp_port_input: None,
            _subscriptions: Vec::new(),
        }
    }

    fn init_mcp_port_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let port = self.settings.read(cx).mcp_port;
        let input = cx.new(|cx| {
            TextArea::new(window, cx)
                .single_line(true)
                .numeric(true)
                .max_length(5)
                .min_height(px(32.))
                .text_size(px(13.))
        });
        input.update(cx, |input, cx| input.set_value(port.to_string(), cx));

        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &TextAreaEvent, _, cx| {
                if *event == TextAreaEvent::PressEnter {
                    this.commit_mcp_port(cx);
                }
            },
        ));
        let focus_handle = input.read(cx).focus_handle(cx);
        self._subscriptions
            .push(cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.commit_mcp_port(cx);
            }));
        self.mcp_port_input = Some(input);
    }

    fn commit_mcp_port(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.mcp_port_input.clone() else {
            return;
        };
        let saved = self.settings.read(cx).mcp_port;
        let typed = input.read(cx).value().to_string();
        let plan = plan_port_commit(&typed, saved, self.mcp_busy, self.mcp_status.running);

        let PortCommit::Apply { port, restart } = plan else {
            if typed != saved.to_string() {
                input.update(cx, |input, cx| input.set_value(saved.to_string(), cx));
            }
            return;
        };
        if typed != port.to_string() {
            input.update(cx, |input, cx| input.set_value(port.to_string(), cx));
        }

        self.settings.update(cx, |settings, _| {
            settings.mcp_port = port;
        });
        mezon_store::schedule_settings_save(&self.settings, cx);

        if let Some(platform) = PlatformStore::try_global(cx)
            && let Some(set_port) = platform.read(cx).mcp_server_set_port_fn()
        {
            set_port(port);
        }

        if restart {
            self.restart_mcp_server(cx);
        } else {
            cx.notify();
        }
    }

    fn refresh_mcp_status(&mut self, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        let Some(platform) = PlatformStore::try_global(cx) else {
            return;
        };
        self.mcp_status = platform.read(cx).mcp_server_status();
    }

    fn resync_mcp_status(&mut self, cx: &mut Context<Self>) {
        let Some(platform) = PlatformStore::try_global(cx) else {
            return;
        };
        let status = platform.read(cx).mcp_server_status();
        self.remember_mcp_enabled(status.running, cx);
        self.mcp_status = status;
    }

    fn remember_mcp_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.settings.read(cx).mcp_enabled == enabled {
            return;
        }
        self.settings.update(cx, |settings, _| {
            settings.mcp_enabled = enabled;
        });
        mezon_store::schedule_settings_save(&self.settings, cx);
    }

    fn restart_mcp_server(&mut self, cx: &mut Context<Self>) {
        let Some(platform) = PlatformStore::try_global(cx) else {
            cx.notify();
            return;
        };
        let platform = platform.read(cx);
        let (Some(start), Some(stop)) = (
            platform.mcp_server_start_fn(),
            platform.mcp_server_stop_fn(),
        ) else {
            cx.notify();
            return;
        };
        let settings = self.settings.read(cx);
        let read_only = settings.mcp_read_only;
        let port = settings.mcp_port;

        self.mcp_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    stop()?;
                    start(read_only, Some(port))
                })
                .await;

            this.update(cx, |this, cx| {
                this.mcp_busy = false;
                match result {
                    Ok(status) => this.mcp_status = status,
                    Err(error) => {
                        tracing::warn!("MCP server restart failed: {error}");
                        this.resync_mcp_status(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn mark_mcp_copied(&mut self, target: McpCopyTarget, cx: &mut Context<Self>) {
        self.mcp_copy_feedback = Some(target);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                if this.mcp_copy_feedback == Some(target) {
                    this.mcp_copy_feedback = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy_mcp_url(&mut self, url: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()));
        self.mark_mcp_copied(McpCopyTarget::Url, cx);
    }

    fn copy_mcp_http_config(&mut self, cx: &mut Context<Self>) {
        let config = mcp_http_server(&self.mcp_status);
        cx.write_to_clipboard(ClipboardItem::new_string(config));
        self.mark_mcp_copied(McpCopyTarget::HttpConfig, cx);
    }

    fn copy_mcp_stdio_config(&mut self, read_only: bool, cx: &mut Context<Self>) {
        let config = mcp_stdio_server(read_only);
        cx.write_to_clipboard(ClipboardItem::new_string(config));
        self.mark_mcp_copied(McpCopyTarget::StdioConfig, cx);
    }

    fn toggle_cli_install(&mut self, cx: &mut Context<Self>) {
        if self.cli_busy {
            return;
        }
        let Some(platform) = PlatformStore::try_global(cx) else {
            return;
        };
        let Some(toggle) = platform.read(cx).cli_install_toggle_fn() else {
            return;
        };
        self.cli_busy = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { toggle() })
                .await;

            this.update(cx, |this, cx| {
                this.cli_busy = false;
                if let Err(error) = result {
                    tracing::warn!("CLI install toggle failed: {error}");
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_mcp_server(&mut self, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        let Some(platform) = PlatformStore::try_global(cx) else {
            return;
        };
        let platform = platform.read(cx);
        let running = self.mcp_status.running;
        let settings = self.settings.read(cx);
        let read_only = settings.mcp_read_only;
        let port = settings.mcp_port;
        let stop_fn = if running {
            platform.mcp_server_stop_fn()
        } else {
            None
        };
        let start_fn = if running {
            None
        } else {
            platform.mcp_server_start_fn()
        };
        if stop_fn.is_none() && start_fn.is_none() {
            return;
        }
        self.mcp_busy = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(stop) = stop_fn {
                        stop()
                    } else if let Some(start) = start_fn {
                        start(read_only, Some(port))
                    } else {
                        Err(anyhow::anyhow!("MCP server hooks unavailable"))
                    }
                })
                .await;

            this.update(cx, |this, cx| {
                this.mcp_busy = false;
                match result {
                    Ok(status) => {
                        this.remember_mcp_enabled(status.running, cx);
                        this.mcp_status = status;
                    }
                    Err(error) => {
                        tracing::warn!("MCP server toggle failed: {error}");
                        this.resync_mcp_status(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_mcp_read_only(&mut self, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        let was_running = self.mcp_status.running;
        let next_read_only = !self.settings.read(cx).mcp_read_only;
        self.settings.update(cx, |settings, _| {
            settings.mcp_read_only = next_read_only;
        });
        mezon_store::schedule_settings_save(&self.settings, cx);

        if !was_running {
            cx.notify();
            return;
        }

        self.restart_mcp_server(cx);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortCommit {
    Keep,
    Apply { port: u16, restart: bool },
}

fn plan_port_commit(typed: &str, saved: u16, busy: bool, running: bool) -> PortCommit {
    if busy {
        return PortCommit::Keep;
    }
    match parse_mcp_port(typed) {
        Some(port) if port != saved => PortCommit::Apply {
            port,
            restart: running,
        },
        _ => PortCommit::Keep,
    }
}

fn parse_mcp_port(typed: &str) -> Option<u16> {
    let typed = typed.trim();
    if typed.is_empty() || !typed.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    typed.parse::<u16>().ok()
}

fn mcp_server_url(status: &McpServerStatus) -> Option<String> {
    if !status.running {
        return None;
    }
    status.url.clone().or_else(|| {
        status
            .port
            .map(|port| format!("http://127.0.0.1:{port}/mcp"))
    })
}

fn mcp_stdio_command() -> &'static str {
    STDIO_COMMAND_PATH.get_or_init(|| {
        let Ok(exe) = std::env::current_exe() else {
            return QUOTED_CLI_NAME.to_string();
        };
        serde_json::to_string(&exe.display().to_string())
            .unwrap_or_else(|_| QUOTED_CLI_NAME.to_string())
    })
}

fn mcp_stdio_server(read_only: bool) -> String {
    let stdio_args = if read_only {
        "[\"mcp\", \"stdio\", \"--read-only\"]"
    } else {
        "[\"mcp\", \"stdio\"]"
    };
    MCP_STDIO_CONFIG
        .replace("{stdio_command}", mcp_stdio_command())
        .replace("{stdio_args}", stdio_args)
}

fn mcp_http_server(status: &McpServerStatus) -> String {
    let port = status
        .port
        .filter(|_| status.running)
        .map(|port| port.to_string())
        .unwrap_or_else(|| "{port}".to_string());
    MCP_HTTP_CONFIG.replace("{port}", &port)
}

fn render_mcp_copy_button(
    theme: &crate::theme::Theme,
    locale: &str,
    copy_id: &'static str,
    copied: bool,
    on_copy: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(copy_id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded_md()
        .cursor_pointer()
        .hover(|el| el.bg(theme.bg_hover))
        .tooltip(Tooltip::text(mezon_i18n::t(locale, "common.copy")))
        .child(
            Icon::new(if copied {
                IconName::Check
            } else {
                IconName::CopyIcon
            })
            .size(px(16.))
            .text_color(if copied {
                theme.status_online
            } else {
                theme.text_muted
            }),
        )
        .on_click(on_copy)
}

fn render_mcp_config_box(
    theme: &crate::theme::Theme,
    config: &str,
    locale: &str,
    copy_id: &'static str,
    copied: bool,
    on_copy: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let config_for_label = config.to_string();
    div().rounded_md().bg(theme.bg_secondary).p_3().child(
        h_flex()
            .items_start()
            .gap_2()
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(config_for_label)
                        .text_sm()
                        .text_color(theme.text_muted),
                ),
            )
            .child(render_mcp_copy_button(
                theme, locale, copy_id, copied, on_copy,
            )),
    )
}

fn render_mcp_url_box(
    theme: &crate::theme::Theme,
    url: &str,
    locale: &str,
    copied: bool,
    on_copy: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let url_for_label = url.to_string();
    h_flex()
        .items_center()
        .gap_2()
        .rounded_md()
        .bg(theme.bg_secondary)
        .p_3()
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                Label::new(url_for_label)
                    .text_sm()
                    .text_color(theme.text_primary),
            ),
        )
        .child(render_mcp_copy_button(
            theme,
            locale,
            "mcp-url-copy",
            copied,
            on_copy,
        ))
}

fn setting_group(
    theme: &crate::theme::Theme,
    title: &str,
    description: &str,
    body: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(Label::new(title).text_color(theme.text_primary))
        .child(
            Label::new(description)
                .text_sm()
                .text_color(theme.text_muted),
        )
        .child(
            v_flex()
                .rounded_lg()
                .bg(theme.bg_primary)
                .p_4()
                .gap_2()
                .child(body),
        )
}

fn toggle_setting_row(
    theme: &crate::theme::Theme,
    title: &str,
    description: &str,
    switch: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new(title).text_color(theme.text_primary))
                .child(switch),
        )
        .child(
            Label::new(description)
                .text_sm()
                .text_color(theme.text_muted),
        )
}

impl Render for AdvancedPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.mcp_port_input.is_none() {
            self.init_mcp_port_input(window, cx);
        }
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let hw_accel = self.settings.read(cx).hardware_acceleration;
        let mcp_read_only = self.settings.read(cx).mcp_read_only;
        let platform = PlatformStore::try_global(cx);
        let cli_visible = platform
            .as_ref()
            .is_some_and(|store| store.read(cx).cli_install_visible());
        let cli_installed = platform
            .as_ref()
            .is_some_and(|store| store.read(cx).cli_install_installed());
        let mcp_available = platform
            .as_ref()
            .is_some_and(|store| store.read(cx).mcp_server_available());
        let cli_busy = self.cli_busy;
        let mcp_busy = self.mcp_busy;
        let mcp_running = self.mcp_status.running;
        let mcp_server_url = mcp_server_url(&self.mcp_status);
        let url_copied = self.mcp_copy_feedback == Some(McpCopyTarget::Url);
        let http_config_copied = self.mcp_copy_feedback == Some(McpCopyTarget::HttpConfig);
        let stdio_config_copied = self.mcp_copy_feedback == Some(McpCopyTarget::StdioConfig);

        let mut page = v_flex().gap_6().child(toggle_setting_row(
            &theme,
            mezon_i18n::t(&locale, "setting.advanced.hardwareAcceleration"),
            mezon_i18n::t(&locale, "setting.advanced.hardwareAccelerationDesc"),
            Switch::new("hardware-acceleration")
                .checked(hw_accel)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.update(cx, |s, _| {
                        s.hardware_acceleration = !s.hardware_acceleration;
                    });
                    mezon_store::schedule_settings_save(&this.settings, cx);
                    cx.notify();
                })),
        ));

        if cli_visible {
            page = page.child(toggle_setting_row(
                &theme,
                mezon_i18n::t(&locale, "setting.advanced.cliInstall.title"),
                mezon_i18n::t(&locale, "setting.advanced.cliInstall.desc"),
                Switch::new("cli-install")
                    .checked(cli_installed)
                    .disabled(cli_busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_cli_install(cx);
                    })),
            ));
        }

        if mcp_available {
            let mut mcp_content = v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            Label::new(mezon_i18n::t(
                                &locale,
                                "setting.advanced.mcp.httpServer.title",
                            ))
                            .text_color(theme.text_primary),
                        )
                        .child(
                            Switch::new("mcp-http-server")
                                .checked(mcp_running)
                                .disabled(mcp_busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_mcp_server(cx);
                                })),
                        ),
                )
                .child(
                    Label::new(mezon_i18n::t(
                        &locale,
                        "setting.advanced.mcp.httpServer.desc",
                    ))
                    .text_sm()
                    .text_color(theme.text_muted),
                );

            if let Some(port_input) = &self.mcp_port_input {
                mcp_content = mcp_content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    Label::new(mezon_i18n::t(
                                        &locale,
                                        "setting.advanced.mcp.port.title",
                                    ))
                                    .text_color(theme.text_primary),
                                )
                                .child(
                                    TextAreaField::new(port_input)
                                        .w(px(96.))
                                        .h(px(32.))
                                        .rounded_md()
                                        .border_1()
                                        .border_color(theme.border),
                                ),
                        )
                        .child(
                            Label::new(mezon_i18n::t(&locale, "setting.advanced.mcp.port.desc"))
                                .text_sm()
                                .text_color(theme.text_muted),
                        ),
                );
            }

            if let Some(url) = mcp_server_url {
                let url_for_display = url.clone();
                let url_for_copy = url;
                mcp_content = mcp_content.child(render_mcp_url_box(
                    &theme,
                    &url_for_display,
                    &locale,
                    url_copied,
                    cx.listener(move |this, _, _, cx| {
                        this.copy_mcp_url(&url_for_copy, cx);
                    }),
                ));
            }

            mcp_content = mcp_content
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            Label::new(mezon_i18n::t(
                                &locale,
                                "setting.advanced.mcp.readOnly.title",
                            ))
                            .text_color(theme.text_primary),
                        )
                        .child(
                            Switch::new("mcp-read-only")
                                .checked(mcp_read_only)
                                .disabled(mcp_busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_mcp_read_only(cx);
                                })),
                        ),
                )
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.advanced.mcp.readOnly.desc"))
                        .text_sm()
                        .text_color(theme.text_muted),
                );

            if mcp_running {
                mcp_content = mcp_content
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.advanced.mcp.httpConfig.title",
                        ))
                        .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.advanced.mcp.httpConfig.desc",
                        ))
                        .text_sm()
                        .text_color(theme.text_muted),
                    )
                    .child(render_mcp_config_box(
                        &theme,
                        &mcp_http_server(&self.mcp_status),
                        &locale,
                        "mcp-http-config-template",
                        http_config_copied,
                        cx.listener(|this, _, _, cx| {
                            this.copy_mcp_http_config(cx);
                        }),
                    ));
            }

            mcp_content = mcp_content
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.advanced.mcp.stdio.title"))
                        .text_color(theme.text_primary),
                )
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.advanced.mcp.stdio.desc"))
                        .text_sm()
                        .text_color(theme.text_muted),
                )
                .child(render_mcp_config_box(
                    &theme,
                    &mcp_stdio_server(mcp_read_only),
                    &locale,
                    "mcp-stdio-config-template",
                    stdio_config_copied,
                    cx.listener(move |this, _, _, cx| {
                        this.copy_mcp_stdio_config(mcp_read_only, cx);
                    }),
                ));

            page = page.child(setting_group(
                &theme,
                mezon_i18n::t(&locale, "setting.advanced.mcp.title"),
                mezon_i18n::t(&locale, "setting.advanced.mcp.desc"),
                mcp_content,
            ));
        }

        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_run_of_digits_is_accepted_as_a_port() {
        assert_eq!(parse_mcp_port("3179"), Some(3179));
        assert_eq!(parse_mcp_port(" 8080 "), Some(8080));
        assert_eq!(parse_mcp_port("0"), Some(0));
        assert_eq!(parse_mcp_port(""), None);
        assert_eq!(parse_mcp_port("abc"), None);
        assert_eq!(parse_mcp_port("70000"), None);
        assert_eq!(parse_mcp_port("80.80"), None);
        assert_eq!(parse_mcp_port("-1"), None);
    }

    #[test]
    fn a_stopped_server_still_applies_the_port_so_the_cli_start_agrees() {
        assert_eq!(
            plan_port_commit("4100", 3179, false, false),
            PortCommit::Apply {
                port: 4100,
                restart: false
            }
        );
    }

    #[test]
    fn a_running_server_is_rebound_onto_the_new_port() {
        assert_eq!(
            plan_port_commit("4100", 3179, false, true),
            PortCommit::Apply {
                port: 4100,
                restart: true
            }
        );
    }

    #[test]
    fn a_commit_is_ignored_while_a_start_or_stop_is_still_in_flight() {
        assert_eq!(plan_port_commit("4100", 3179, true, true), PortCommit::Keep);
    }

    #[test]
    fn an_unparsable_or_unchanged_port_is_left_alone() {
        assert_eq!(
            plan_port_commit("80.80", 3179, false, true),
            PortCommit::Keep
        );
        assert_eq!(plan_port_commit("", 3179, false, true), PortCommit::Keep);
        assert_eq!(
            plan_port_commit("3179", 3179, false, true),
            PortCommit::Keep
        );
    }

    #[test]
    fn both_config_snippets_are_valid_json() {
        for config in [
            mcp_stdio_server(false),
            mcp_stdio_server(true),
            mcp_http_server(&McpServerStatus {
                running: true,
                port: Some(3179),
                read_only: false,
                url: Some("http://127.0.0.1:3179/mcp".to_string()),
            }),
        ] {
            serde_json::from_str::<serde_json::Value>(&config)
                .unwrap_or_else(|e| panic!("{config}\n\nis not valid JSON: {e}"));
        }
    }

    #[test]
    fn the_stdio_command_is_an_absolute_path_quoted_as_valid_json() {
        let command = mcp_stdio_command();
        assert!(command.starts_with('"') && command.ends_with('"'));
        let path = &command[1..command.len() - 1];
        assert_ne!(
            path, "mezon",
            "the shim name would not resolve from a GUI host"
        );
        assert!(!path.contains('\\') || command.contains("\\\\"));
    }
}
