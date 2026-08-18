mod layout;
mod render;
mod text;

pub use layout::{GAP, RADIUS, TileRect, TileShape, layout_tiles};
pub use render::{DrawTile, Renderer, SourceImage, accent_for};
pub use text::TextPainter;

use std::sync::Arc;

use parking_lot::RwLock;

#[derive(Clone, Debug)]
pub struct AvatarImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct SceneTile {
    pub key: String,
    pub label: String,
    pub initial: String,
    pub frame_key: Option<u64>,
    pub avatar: Option<Arc<AvatarImage>>,
    pub is_screen_share: bool,
    pub focused: bool,
    pub speaking: bool,
}

#[derive(Clone, Default)]
pub struct Scene {
    tiles: Arc<RwLock<Vec<SceneTile>>>,
}

impl Scene {
    pub fn set(&self, tiles: Vec<SceneTile>) {
        *self.tiles.write() = tiles;
    }

    pub fn snapshot(&self) -> Vec<SceneTile> {
        self.tiles.read().clone()
    }
}
