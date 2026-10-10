use crate::chat::notification_setting_modal::radio_dot;
use crate::components::primitives::{Label, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};
use gpui::{Context, Entity, FontWeight, Hsla, Subscription, Window, div, prelude::*, px};
use mezon_store::{ConnectionStore, RealtimeServer, Settings};

pub struct ServerPage {
    settings: Entity<Settings>,
    _subscriptions: Vec<Subscription>,
}

impl ServerPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe(&settings, |_, _, cx| cx.notify())];
        if let Some(connection) = ConnectionStore::try_global(cx) {
            subscriptions.push(cx.observe(&connection, |_, _, cx| cx.notify()));
        }
        Self {
            settings,
            _subscriptions: subscriptions,
        }
    }
}

fn region_label(host: &str) -> String {
    RealtimeServer::region_of_host(host)
        .map(str::to_string)
        .unwrap_or_else(|| host.to_string())
}

fn server_title(server: RealtimeServer, locale: &str) -> String {
    server
        .region_name()
        .map(str::to_string)
        .unwrap_or_else(|| mezon_i18n::t(locale, "setting.server.auto").to_string())
}

fn server_subtitle(
    server: RealtimeServer,
    chosen: RealtimeServer,
    region_in_use: Option<&str>,
    locale: &str,
) -> String {
    match server {
        RealtimeServer::Auto => match region_in_use {
            Some(region) if chosen == RealtimeServer::Auto => {
                mezon_i18n::t(locale, "setting.server.inUse").replace("{{region}}", region)
            }
            _ => mezon_i18n::t(locale, "setting.server.autoHint").to_string(),
        },
        RealtimeServer::Vn1 | RealtimeServer::Vn2 => {
            mezon_i18n::t(locale, "setting.server.vietnam").to_string()
        }
        RealtimeServer::Us => mezon_i18n::t(locale, "setting.server.unitedStates").to_string(),
    }
}

fn server_row(
    server: RealtimeServer,
    chosen: RealtimeServer,
    region_in_use: Option<&str>,
    locale: &str,
    theme: &Theme,
) -> impl IntoElement {
    let selected = server == chosen;
    let border: Hsla = theme.tokens.border_primary.into();
    let fill: Hsla = theme.brand.into();
    h_flex()
        .id(("realtime-server", server as usize))
        .items_center()
        .gap(px(12.))
        .p(px(12.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.tokens.bg_item_hover))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    Label::new(server_title(server, locale))
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary),
                )
                .child(
                    Label::new(server_subtitle(server, chosen, region_in_use, locale))
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary),
                ),
        )
        .child(radio_dot(selected, border, fill))
        .on_click(move |_, _, cx| {
            ConnectionStore::global(cx)
                .update(cx, |store, cx| store.select_realtime_server(server, cx));
        })
}

fn connection_line(
    host: Option<&str>,
    connected: bool,
    locale: &str,
    theme: &Theme,
) -> Option<impl IntoElement> {
    let region = region_label(host?);
    let (key, color) = if connected {
        ("setting.server.connectedTo", theme.status_online)
    } else {
        ("setting.server.connectingTo", theme.status_idle)
    };
    Some(
        h_flex()
            .items_center()
            .gap(px(8.))
            .child(div().size(px(8.)).rounded_full().bg(color))
            .child(
                Label::new(mezon_i18n::t(locale, key).replace("{{region}}", &region))
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary),
            ),
    )
}

impl Render for ServerPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let settings = self.settings.read(cx);
        let locale = settings.language.clone();
        let chosen = settings.realtime_server;
        let (host, connected) = ConnectionStore::try_global(cx)
            .map(|store| {
                let store = store.read(cx);
                (
                    store.realtime_host().map(str::to_string),
                    store.is_realtime_connected(),
                )
            })
            .unwrap_or_default();
        let region_in_use = host.as_deref().map(region_label);

        let mut options = v_flex()
            .p(px(8.))
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border);
        for (index, server) in RealtimeServer::ALL.into_iter().enumerate() {
            if index > 0 {
                options = options.child(div().h(px(1.)).mx(px(-8.)).bg(theme.border));
            }
            options = options.child(server_row(
                server,
                chosen,
                region_in_use.as_deref(),
                &locale,
                &theme,
            ));
        }

        v_flex()
            .gap_3()
            .children(connection_line(host.as_deref(), connected, &locale, &theme))
            .child(options)
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.server.footer"))
                    .text_sm()
                    .px_1()
                    .text_color(theme.tokens.text_theme_primary),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_names_the_node_in_use_only_while_auto_is_chosen() {
        assert_eq!(
            server_subtitle(
                RealtimeServer::Auto,
                RealtimeServer::Auto,
                Some("VN2"),
                "en"
            ),
            "Using VN2"
        );
        assert_eq!(
            server_subtitle(RealtimeServer::Auto, RealtimeServer::Us, Some("US"), "en"),
            "Picks a server for you"
        );
        assert_eq!(
            server_subtitle(RealtimeServer::Auto, RealtimeServer::Auto, None, "en"),
            "Picks a server for you"
        );
    }

    #[test]
    fn pinned_servers_show_their_region() {
        assert_eq!(server_title(RealtimeServer::Vn2, "en"), "VN2");
        assert_eq!(server_title(RealtimeServer::Auto, "vi"), "Tự động");
        assert_eq!(
            server_subtitle(RealtimeServer::Us, RealtimeServer::Auto, None, "vi"),
            "Hoa Kỳ"
        );
        assert_eq!(region_label("sock3.mezon.ai"), "VN2");
        assert_eq!(region_label("other.example.com"), "other.example.com");
    }
}
