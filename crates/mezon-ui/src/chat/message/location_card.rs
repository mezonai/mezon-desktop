use gpui::{
    AnyElement, App, Bounds, Corners, Entity, FontWeight, ImageCache, MouseButton, Pixels,
    Resource, Window, canvas, div, point, prelude::*, px, rgba, size,
};
use mezon_client::transport::{location_map_preview, mezon_location_tile_base};
use mezon_store::{AppConfig, Message, MessageLocation};

use super::content::{SelectableSectionCursor, SelectableTextContext, open_message_link};
use super::context::RowCtx;
use crate::components::primitives::Avatar;
use crate::image_cache::LruImageCache;

const MAP_HEIGHT: f32 = 150.;
const INFO_BAR_HEIGHT: f32 = 40.;
const AVATAR_SIZE: f32 = 30.;
const AVATAR_RING: f32 = 34.;
const CARD_RADIUS: f32 = 10.;
const CARD_MAX_WIDTH: f32 = 520.;
const MAP_TILE_PX: f32 = 256.;
const AVATAR_BORDER: u32 = 0xff_ff_ff_ff;
const MAP_CREDIT_URL: &str = "https://www.openstreetmap.org/copyright";

pub fn render_location_card(
    msg: &Message,
    location: &MessageLocation,
    is_me: bool,
    ctx: &RowCtx,
    base: usize,
    selection_context: &SelectableTextContext,
) -> AnyElement {
    let theme = ctx.theme;
    let title = location_title(msg, is_me, ctx.locale);
    let mut cursor = SelectableSectionCursor::new(base);
    let title_range = cursor.section(&title);

    let card_width = if ctx.content_width > 0. {
        ctx.content_width.min(CARD_MAX_WIDTH)
    } else {
        CARD_MAX_WIDTH
    };

    let tile_base = mezon_location_tile_base(
        AppConfig::try_global(ctx.app)
            .map(|cfg| cfg.mezon_cdn_url.as_str())
            .unwrap_or("https://cdn.mezon.ai"),
    );
    let map_preview = location_map_preview(
        location.latitude,
        location.longitude,
        card_width,
        MAP_HEIGHT,
        &tile_base,
    );
    let map_origin_left = map_preview.origin_left_px;
    let map_origin_top = map_preview.origin_top_px;
    let map_tiles = map_preview.tile_urls;
    let map_cache = ctx.location_map_cache.clone();

    let mut avatar_view = Avatar::new()
        .size_px(px(AVATAR_SIZE))
        .border_color(rgba(AVATAR_BORDER))
        .image_cache(ctx.avatar_cache.clone());
    if !msg.avatar_proxied.is_empty() {
        avatar_view = avatar_view
            .src(msg.avatar_proxied.clone())
            .fallback_src(msg.avatar_url.to_string());
    } else if !msg.avatar_url.is_empty() {
        let proxied = crate::util::imgproxy::avatar_url(ctx.app, &msg.avatar_url);
        avatar_view = avatar_view
            .src(proxied)
            .fallback_src(msg.avatar_url.to_string());
    }

    let maps_url = location.maps_url.clone();
    let selection = ctx.selection.clone();
    let card_bg = theme.tokens.bg_tertiary;
    let map_placeholder_bg = theme.tokens.border_primary;

    div()
        .id(("msg-location", msg.row_anchor_id.0 as usize))
        .w(px(card_width))
        .mt_1()
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(card_bg)
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            if selection.borrow().has_selection() {
                return;
            }
            open_message_link(maps_url.to_string(), cx);
        })
        .child(
            div()
                .relative()
                .w_full()
                .h(px(MAP_HEIGHT))
                .overflow_hidden()
                .rounded_tl(px(CARD_RADIUS))
                .rounded_tr(px(CARD_RADIUS))
                .bg(map_placeholder_bg)
                .image_cache(ctx.location_map_cache.clone())
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| {
                            paint_location_map(
                                bounds,
                                map_origin_left,
                                map_origin_top,
                                &map_tiles,
                                &map_cache,
                                window,
                                cx,
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(map_credit(ctx.locale))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(AVATAR_RING))
                                .rounded_full()
                                .bg(theme.tokens.bg_active_member_channel)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(avatar_view),
                        ),
                ),
        )
        .child(
            div()
                .w_full()
                .h(px(INFO_BAR_HEIGHT))
                .px(px(10.))
                .flex()
                .items_center()
                .bg(card_bg)
                .child(
                    div()
                        .truncate()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.tokens.text_theme_message)
                        .cursor(gpui::CursorStyle::IBeam)
                        .when_some(title_range, |div, range| {
                            div.child(selection_context.text_node(&title, range))
                        }),
                ),
        )
        .into_any_element()
}

fn paint_location_map(
    viewport: Bounds<Pixels>,
    origin_left: f32,
    origin_top: f32,
    tiles: &[[String; 3]; 3],
    cache: &Entity<LruImageCache>,
    window: &mut Window,
    cx: &mut App,
) {
    let tile_px = px(MAP_TILE_PX);
    let radius = px(CARD_RADIUS);
    for (row, urls) in tiles.iter().enumerate() {
        for (col, url) in urls.iter().enumerate() {
            let tile_bounds = Bounds {
                origin: point(
                    viewport.origin.x + px(origin_left) + tile_px * col as f32,
                    viewport.origin.y + px(origin_top) + tile_px * row as f32,
                ),
                size: size(tile_px, tile_px),
            };
            let visible = viewport.intersect(&tile_bounds);
            if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
                continue;
            }
            let resource = Resource::Uri(url.clone().into());
            let Some(Ok(image)) = cache.update(cx, |cache, cx| {
                ImageCache::load(cache, &resource, window, cx)
            }) else {
                continue;
            };
            let mut radii = Corners::default();
            if px_near(visible.origin.x, viewport.origin.x)
                && px_near(visible.origin.y, viewport.origin.y)
            {
                radii.top_left = radius;
            }
            if px_near(
                visible.origin.x + visible.size.width,
                viewport.origin.x + viewport.size.width,
            ) && px_near(visible.origin.y, viewport.origin.y)
            {
                radii.top_right = radius;
            }
            if let Err(error) = window.paint_image(visible, tile_bounds, radii, image, 0, false) {
                tracing::warn!("location map tile paint failed: {error}");
            }
        }
    }
}

fn map_credit(locale: &str) -> AnyElement {
    div()
        .absolute()
        .bottom(px(4.))
        .left(px(6.))
        .px(px(4.))
        .py(px(1.))
        .rounded(px(3.))
        .bg(rgba(0x000000_99))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| open_message_link(MAP_CREDIT_URL.to_string(), cx))
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgba(0xffffff_e6))
                .child(mezon_i18n::t(locale, "message.mapView.mapCredit")),
        )
        .into_any_element()
}

fn px_near(left: Pixels, right: Pixels) -> bool {
    (f32::from(left) - f32::from(right)).abs() < 0.5
}

pub(crate) fn selectable_location_text(msg: &Message, is_me: bool, locale: &str) -> String {
    location_title(msg, is_me, locale)
}

fn location_title(msg: &Message, is_me: bool, locale: &str) -> String {
    if is_me {
        return mezon_i18n::t(locale, "message.mapView.yourLocation").to_string();
    }
    mezon_i18n::t(locale, "message.mapView.locationOf")
        .replace("{{name}}", msg.sender_name.as_ref())
}

#[cfg(test)]
mod tests {
    use mezon_client::transport::LOCATION_MAP_PREVIEW_ZOOM;

    #[test]
    fn preview_strip_uses_expected_zoom() {
        assert_eq!(LOCATION_MAP_PREVIEW_ZOOM, 15);
        let preview = mezon_client::transport::location_map_preview(
            18.6734,
            105.6923,
            400.,
            150.,
            mezon_client::transport::LOCATION_MAP_TILE_ROOT,
        );
        assert!(preview.tile_urls[1][1].starts_with(&format!(
            "{}/15/",
            mezon_client::transport::LOCATION_MAP_TILE_ROOT
        )));
        assert!(
            !preview
                .tile_urls
                .iter()
                .flatten()
                .any(|url| url.contains("openstreetmap.org"))
        );
        assert!(preview.origin_top_px + 256. * 3. >= 150.);
    }
}
