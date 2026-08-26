use std::borrow::Cow;

use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

pub const AVATAR_GROUP: &str = "images/avatar-group.png";
pub const MEZON_LOGO: &str = "images/logoflashsceenmezon.png";
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
            STREAM_THUMBNAIL,
            MEZON_COMMUNITY,
            CHANNEL_SETTING_LOGO_LIGHT,
            CHANNEL_SETTING_LOGO_DARK,
            ONBOARDING,
            EMPTY_WEBHOOK,
            AGE_RESTRICTED_WARNING,
            "icons/flower.svg",
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
