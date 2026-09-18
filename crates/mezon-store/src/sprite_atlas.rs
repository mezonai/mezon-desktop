use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use gpui::SharedString;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpriteFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Default, PartialEq)]
pub struct SpriteAtlas {
    pub frames: HashMap<SharedString, SpriteFrame>,
    pub sheet_width: f32,
    pub sheet_height: f32,
}

impl SpriteAtlas {
    pub fn frame(&self, name: &str) -> Option<SpriteFrame> {
        self.frames.get(name).copied()
    }
}

#[derive(Deserialize)]
struct RawRect {
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    w: f32,
    #[serde(default)]
    h: f32,
}

#[derive(Deserialize)]
struct RawFrame {
    #[serde(default)]
    filename: Option<String>,
    frame: RawRect,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawFrames {
    Map(HashMap<String, RawFrame>),
    List(Vec<RawFrame>),
}

#[derive(Deserialize)]
struct RawMeta {
    #[serde(default)]
    size: Option<RawRect>,
}

#[derive(Deserialize)]
struct RawAtlas {
    frames: RawFrames,
    #[serde(default)]
    meta: Option<RawMeta>,
}

pub fn parse_sprite_atlas(bytes: &[u8]) -> Result<SpriteAtlas> {
    let raw: RawAtlas = serde_json::from_slice(bytes)?;
    let mut frames = HashMap::new();
    match raw.frames {
        RawFrames::Map(map) => {
            for (name, frame) in map {
                frames.insert(SharedString::from(name), rect_to_frame(&frame.frame));
            }
        }
        RawFrames::List(list) => {
            for (index, frame) in list.iter().enumerate() {
                let name = frame
                    .filename
                    .clone()
                    .unwrap_or_else(|| format!("frame_{index}"));
                frames.insert(SharedString::from(name), rect_to_frame(&frame.frame));
            }
        }
    }
    if frames.is_empty() {
        anyhow::bail!("sprite atlas has no frames");
    }
    let (fallback_width, fallback_height) = frames.values().fold((0f32, 0f32), |acc, frame| {
        (
            acc.0.max(frame.x + frame.width),
            acc.1.max(frame.y + frame.height),
        )
    });
    let size = raw.meta.and_then(|meta| meta.size);
    let sheet_width = size
        .as_ref()
        .map(|s| s.w)
        .filter(|w| *w > 0.)
        .unwrap_or(fallback_width);
    let sheet_height = size
        .as_ref()
        .map(|s| s.h)
        .filter(|h| *h > 0.)
        .unwrap_or(fallback_height);
    if sheet_width <= 0. || sheet_height <= 0. {
        anyhow::bail!("sprite atlas has no usable size");
    }
    Ok(SpriteAtlas {
        frames,
        sheet_width,
        sheet_height,
    })
}

fn rect_to_frame(rect: &RawRect) -> SpriteFrame {
    SpriteFrame {
        x: rect.x,
        y: rect.y,
        width: rect.w,
        height: rect.h,
    }
}

pub async fn fetch_sprite_atlas(url: String) -> Result<Arc<SpriteAtlas>> {
    let (bytes, _) = mezon_client::transport_runtime::fetch_bytes(&url).await?;
    Ok(Arc::new(parse_sprite_atlas(&bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hash_layout_with_meta_size() {
        let json = br#"{
            "frames": {
                "a.png": { "frame": { "x": 0, "y": 0, "w": 10, "h": 20 } },
                "b.png": { "frame": { "x": 10, "y": 0, "w": 10, "h": 20 } }
            },
            "meta": { "size": { "w": 20, "h": 20 } }
        }"#;
        let atlas = parse_sprite_atlas(json).expect("atlas should parse");
        assert_eq!(atlas.sheet_width, 20.);
        assert_eq!(atlas.sheet_height, 20.);
        assert_eq!(
            atlas.frame("b.png"),
            Some(SpriteFrame {
                x: 10.,
                y: 0.,
                width: 10.,
                height: 20.
            })
        );
    }

    #[test]
    fn falls_back_to_frame_extent_without_meta() {
        let json = br#"{
            "frames": [ { "filename": "only", "frame": { "x": 4, "y": 6, "w": 8, "h": 10 } } ]
        }"#;
        let atlas = parse_sprite_atlas(json).expect("atlas should parse");
        assert_eq!(atlas.sheet_width, 12.);
        assert_eq!(atlas.sheet_height, 16.);
        assert!(atlas.frame("only").is_some());
    }

    #[test]
    fn rejects_an_empty_atlas() {
        assert!(parse_sprite_atlas(br#"{ "frames": {} }"#).is_err());
    }
}
