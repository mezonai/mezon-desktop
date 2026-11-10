use std::borrow::Cow;

use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

pub const AVATAR_GROUP: &str = "images/avatar-group.png";
pub const MEZON_LOGO: &str = "images/logoflashsceenmezon.png";
pub const APP_ICON: &str = "images/mezon-app-icon.png";
pub const STREAM_THUMBNAIL: &str = "images/flahstream.png";
pub const MEZON_COMMUNITY: &str = "images/mezon-community.png";
pub const CHANNEL_SETTING_LOGO_LIGHT: &str = "images/channel_setting_logo_light.svg";
pub const CHANNEL_SETTING_LOGO_DARK: &str = "images/channel_setting_logo_dark.svg";
pub const ONBOARDING: &str = "images/onboarding.png";
pub const EMPTY_WEBHOOK: &str = "images/empty-webhook.svg";
pub const AGE_RESTRICTED_WARNING: &str = "images/warning.svg";

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
#[include = "images/**/*.png"]
#[include = "images/**/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_asset_is_embedded() {
        for path in [
            AVATAR_GROUP,
            MEZON_LOGO,
            APP_ICON,
            STREAM_THUMBNAIL,
            MEZON_COMMUNITY,
            CHANNEL_SETTING_LOGO_LIGHT,
            CHANNEL_SETTING_LOGO_DARK,
            ONBOARDING,
            EMPTY_WEBHOOK,
            AGE_RESTRICTED_WARNING,
            "icons/gift.svg",
        ] {
            let loaded = Assets
                .load(path)
                .unwrap_or_else(|e| panic!("{path} is declared but not embedded: {e}"));
            assert!(
                loaded.is_some_and(|bytes| !bytes.is_empty()),
                "{path} embedded but empty"
            );
        }
    }

    #[test]
    fn every_motion_catalog_icon_is_embedded_and_renders() {
        let catalog = include_str!("../../../mezon-widgets/assets/icon-motion.rs");
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(Assets));
        for path in catalog
            .split('"')
            .filter(|part| part.starts_with("icons/") && part.ends_with(".svg"))
        {
            let bytes = Assets
                .load(path)
                .unwrap_or_else(|error| panic!("{path}: {error}"))
                .expect("catalog icon is embedded");
            let frame = renderer
                .render_single_frame(&bytes, 1.0)
                .unwrap_or_else(|error| panic!("{path}: {error}"));
            assert!(frame.size(0).width.0 > 0, "{path}");
            assert!(frame.size(0).height.0 > 0, "{path}");
        }
    }

    #[test]
    fn callout_carets_keep_rounded_tips_and_flush_bases_at_native_size() {
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(Assets));
        for (direction, width, height) in [
            ("up", 12, 6),
            ("down", 12, 6),
            ("left", 6, 12),
            ("right", 6, 12),
        ] {
            let path = format!("icons/caret-{direction}.svg");
            let bytes = Assets.load(&path).unwrap().unwrap();
            let caret_svg = std::str::from_utf8(&bytes).unwrap();
            let padded_svg = caret_svg
                .replacen(
                    &format!("width=\"{width}\""),
                    &format!("width=\"{}\"", width + 2),
                    1,
                )
                .replacen(
                    &format!("height=\"{height}\""),
                    &format!("height=\"{}\"", height + 2),
                    1,
                )
                .replacen(
                    &format!("viewBox=\"0 0 {width} {height}\""),
                    &format!("viewBox=\"-1 -1 {} {}\"", width + 2, height + 2),
                    1,
                );
            let padded = renderer
                .render_single_frame(padded_svg.as_bytes(), 8.0)
                .unwrap();
            let frame = renderer.render_single_frame(&bytes, 8.0).unwrap();
            let rendered = frame.size(0);
            assert_eq!(
                rendered.width.0 * height,
                rendered.height.0 * width,
                "{path}"
            );
            let scale = rendered.width.0 / width;
            let (width, height) = (rendered.width.0, rendered.height.0);
            let pixels = frame.as_bytes(0).unwrap();
            let alpha = |x: i32, y: i32| pixels[((y * width + x) * 4 + 3) as usize];
            let padding_x = (padded.size(0).width.0 - width) / 2;
            let padding_y = (padded.size(0).height.0 - height) / 2;
            let padded_width = padded.size(0).width.0;
            let padded_pixels = padded.as_bytes(0).unwrap();
            for y in 0..padded.size(0).height.0 {
                for x in 0..padded_width {
                    let padded_alpha = padded_pixels[((y * padded_width + x) * 4 + 3) as usize];
                    if x < padding_x
                        || x >= padding_x + width
                        || y < padding_y
                        || y >= padding_y + height
                    {
                        assert!(
                            padded_alpha <= 1,
                            "{path}: geometry extends beyond the viewport"
                        );
                    }
                }
            }

            for (x, y) in [
                (0, 0),
                (width - 1, 0),
                (0, height - 1),
                (width - 1, height - 1),
            ] {
                assert!(
                    alpha(x, y) <= 1,
                    "{path}: clipped or sharp corner at {x},{y}"
                );
            }
            let mut min_x = width;
            let mut min_y = height;
            let mut max_x = 0;
            let mut max_y = 0;
            for y in 0..height {
                for x in 0..width {
                    if pixels[((y * width + x) * 4 + 3) as usize] > 0 {
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                    }
                }
            }
            let tip_gap = match direction {
                "up" => {
                    assert_eq!((min_x, max_x, max_y), (0, width - 1, height - 1));
                    min_y
                }
                "down" => {
                    assert_eq!((min_x, max_x, min_y), (0, width - 1, 0));
                    height - 1 - max_y
                }
                "left" => {
                    assert_eq!((min_y, max_y, max_x), (0, height - 1, width - 1));
                    min_x
                }
                "right" => {
                    assert_eq!((min_y, max_y, min_x), (0, height - 1, 0));
                    width - 1 - max_x
                }
                _ => unreachable!(),
            };
            assert!(
                tip_gap > 0 && tip_gap <= scale,
                "{path}: tip must have less than one logical pixel of raster clearance"
            );
            let tip_pixels = match direction {
                "up" => (0..width).filter(|&x| alpha(x, min_y) > 1).count(),
                "down" => (0..width).filter(|&x| alpha(x, max_y) > 1).count(),
                "left" => (0..height).filter(|&y| alpha(min_x, y) > 1).count(),
                "right" => (0..height).filter(|&y| alpha(max_x, y) > 1).count(),
                _ => unreachable!(),
            };
            assert!(
                tip_pixels > 0 && tip_pixels < (width.max(height) / 8) as usize,
                "{path}: flattened tip"
            );

            let native = renderer.render_single_frame(&bytes, 0.5).unwrap();
            let native_size = native.size(0);
            let native_pixels = native.as_bytes(0).unwrap();
            let native_alpha =
                |x: i32, y: i32| native_pixels[((y * native_size.width.0 + x) * 4 + 3) as usize];
            let edge_alpha = match direction {
                "up" => (0..native_size.width.0).map(|x| native_alpha(x, 0)).max(),
                "down" => (0..native_size.width.0)
                    .map(|x| native_alpha(x, native_size.height.0 - 1))
                    .max(),
                "left" => (0..native_size.height.0).map(|y| native_alpha(0, y)).max(),
                "right" => (0..native_size.height.0)
                    .map(|y| native_alpha(native_size.width.0 - 1, y))
                    .max(),
                _ => unreachable!(),
            };
            assert!(
                edge_alpha.unwrap() < 128,
                "{path}: native-size tip must not end in an opaque flat edge"
            );
        }
    }

    #[test]
    fn age_restricted_warning_renders_as_an_image() {
        let bytes = Assets
            .load(AGE_RESTRICTED_WARNING)
            .expect("warning illustration is embedded")
            .expect("warning illustration has bytes");
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(Assets));
        let frame = renderer
            .render_single_frame(&bytes, 1.0)
            .expect("warning illustration parses as svg");
        assert!(frame.size(0).width.0 > 0);
    }
}
