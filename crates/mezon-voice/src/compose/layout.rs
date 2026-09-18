pub const GAP: f32 = 8.0;
pub const PADDING: f32 = 8.0;
pub const RADIUS: f32 = 8.0;

const STRIP_MAX_HEIGHT: f32 = 93.0;
const STRIP_MIN_TILE_WIDTH: f32 = 140.0;
const STRIP_ASPECT_RATIO: f32 = 16.0 / 10.0;
const FOCUS_MAIN_SHARE: f32 = 5.0 / 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TileShape {
    pub focused: bool,
    pub contain: bool,
    pub fullscreen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub tile: usize,
    pub rect: TileRect,
    pub thumbnail: bool,
}

enum GridOrientation {
    Portrait,
    Landscape,
}

struct GridLayoutDef {
    columns: usize,
    rows: usize,
    min_width: f32,
    orientation: Option<GridOrientation>,
}

impl GridLayoutDef {
    const fn new(columns: usize, rows: usize, min_width: f32) -> Self {
        Self {
            columns,
            rows,
            min_width,
            orientation: None,
        }
    }

    const fn oriented(columns: usize, rows: usize, orientation: GridOrientation) -> Self {
        Self {
            columns,
            rows,
            min_width: 0.,
            orientation: Some(orientation),
        }
    }

    fn max_tiles(&self) -> usize {
        self.columns * self.rows
    }

    fn fits_orientation(&self, landscape: bool) -> bool {
        match self.orientation {
            None => true,
            Some(GridOrientation::Landscape) => landscape,
            Some(GridOrientation::Portrait) => !landscape,
        }
    }
}

const GRID_LAYOUTS: &[GridLayoutDef] = &[
    GridLayoutDef::new(1, 1, 0.),
    GridLayoutDef::oriented(1, 2, GridOrientation::Portrait),
    GridLayoutDef::oriented(2, 1, GridOrientation::Landscape),
    GridLayoutDef::new(2, 2, 560.),
    GridLayoutDef::new(3, 3, 700.),
    GridLayoutDef::new(4, 4, 960.),
    GridLayoutDef::new(5, 5, 1100.),
];

fn select_grid_layout(
    layouts: &[GridLayoutDef],
    participant_count: usize,
    width: f32,
    height: f32,
) -> (usize, usize) {
    if width <= 0. || height <= 0. {
        return (layouts[0].columns, layouts[0].rows);
    }
    let landscape = width / height > 1.;
    let mut selected = None;
    for (index, layout) in layouts.iter().enumerate() {
        let bigger_same_capacity = layouts[index + 1..]
            .iter()
            .any(|l| l.max_tiles() == layout.max_tiles() && l.fits_orientation(landscape));
        if layout.max_tiles() >= participant_count && !bigger_same_capacity {
            selected = Some(index);
            break;
        }
    }
    let index = selected.unwrap_or(layouts.len() - 1);
    let layout = &layouts[index];
    if width < layout.min_width && index > 0 {
        let smaller_count = layouts[index - 1].max_tiles();
        return select_grid_layout(&layouts[..index], smaller_count, width, height);
    }
    (layout.columns, layout.rows)
}

pub fn layout_tiles(width: f32, height: f32, tiles: &[TileShape]) -> Vec<Placement> {
    let count = tiles.len();
    if count == 0 || width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }

    if let Some(tile) = tiles.iter().position(|tile| tile.fullscreen) {
        return vec![Placement {
            tile,
            rect: TileRect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: height,
            },
            thumbnail: false,
        }];
    }

    match tiles.iter().position(|tile| tile.focused) {
        Some(focus) => layout_with_focus(width, height, tiles, focus),
        None => layout_grid(width, height, count),
    }
}

fn layout_with_focus(width: f32, height: f32, tiles: &[TileShape], focus: usize) -> Vec<Placement> {
    let count = tiles.len();
    let inner_width = width - PADDING * 2.0;
    let inner_height = height - PADDING * 2.0;
    if inner_width <= 0.0 || inner_height <= 0.0 {
        return Vec::new();
    }

    let strip: Vec<usize> = if tiles[focus].contain {
        (0..count).collect()
    } else {
        (0..count).filter(|index| *index != focus).collect()
    };

    let main_full = Placement {
        tile: focus,
        rect: TileRect {
            x: PADDING,
            y: PADDING,
            w: inner_width,
            h: inner_height,
        },
        thumbnail: false,
    };
    if strip.is_empty() || strip == [focus] {
        return vec![main_full];
    }

    let free = inner_height - GAP;
    if free <= 0.0 {
        return vec![main_full];
    }
    let strip_height = (free * (1.0 - FOCUS_MAIN_SHARE)).min(STRIP_MAX_HEIGHT);
    let main_height = free - strip_height;

    let mut placements = vec![Placement {
        tile: focus,
        rect: TileRect {
            x: PADDING,
            y: PADDING,
            w: inner_width,
            h: main_height,
        },
        thumbnail: false,
    }];

    let tile_width = (strip_height * STRIP_ASPECT_RATIO).max(STRIP_MIN_TILE_WIDTH);
    let max_visible = (((inner_width + GAP) / (tile_width + GAP)).floor() as isize).max(1) as usize;
    let visible = strip.len().min(max_visible);
    let content_width = visible as f32 * tile_width + (visible as f32 - 1.0) * GAP;
    let strip_x = PADDING + ((inner_width - content_width) / 2.0).max(0.0);
    let strip_y = PADDING + main_height + GAP;

    for (position, tile) in strip.into_iter().take(visible).enumerate() {
        placements.push(Placement {
            tile,
            rect: TileRect {
                x: strip_x + position as f32 * (tile_width + GAP),
                y: strip_y,
                w: tile_width,
                h: strip_height,
            },
            thumbnail: true,
        });
    }

    placements
}

fn layout_grid(width: f32, height: f32, count: usize) -> Vec<Placement> {
    let (columns, rows) = grid_shape(width, height, count);
    let cell_width = (width - PADDING * 2.0 - GAP * (columns as f32 - 1.0)) / columns as f32;
    let cell_height = (height - PADDING * 2.0 - GAP * (rows as f32 - 1.0)) / rows as f32;
    if cell_width <= 0.0 || cell_height <= 0.0 {
        return Vec::new();
    }

    (0..count)
        .map(|index| {
            let row = index / columns;
            let column = index - row * columns;
            Placement {
                tile: index,
                rect: TileRect {
                    x: PADDING + column as f32 * (cell_width + GAP),
                    y: PADDING + row as f32 * (cell_height + GAP),
                    w: cell_width,
                    h: cell_height,
                },
                thumbnail: false,
            }
        })
        .collect()
}

fn grid_shape(width: f32, height: f32, count: usize) -> (usize, usize) {
    let (columns, rows) = select_grid_layout(GRID_LAYOUTS, count, width, height);
    if columns * rows >= count {
        return (columns, rows);
    }
    (columns, count.div_ceil(columns))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(count: usize) -> Vec<TileShape> {
        vec![TileShape::default(); count]
    }

    fn rect_of(placements: &[Placement], tile: usize) -> TileRect {
        placements
            .iter()
            .find(|p| p.tile == tile && !p.thumbnail)
            .expect("a main placement")
            .rect
    }

    fn thumbnails(placements: &[Placement]) -> Vec<&Placement> {
        placements.iter().filter(|p| p.thumbnail).collect()
    }

    #[test]
    fn an_empty_scene_lays_out_nothing() {
        assert!(layout_tiles(1280.0, 720.0, &[]).is_empty());
    }

    #[test]
    fn a_single_tile_fills_the_frame_inside_the_padding() {
        let placements = layout_tiles(1280.0, 720.0, &plain(1));
        assert_eq!(
            rect_of(&placements, 0),
            TileRect {
                x: PADDING,
                y: PADDING,
                w: 1280.0 - PADDING * 2.0,
                h: 720.0 - PADDING * 2.0,
            }
        );
    }

    #[test]
    fn a_landscape_pair_sits_side_by_side() {
        let placements = layout_tiles(1280.0, 720.0, &plain(2));
        let left = rect_of(&placements, 0);
        let right = rect_of(&placements, 1);
        assert_eq!(left.y, right.y);
        assert!((left.w - right.w).abs() < 0.01);
        assert!((right.x - (left.x + left.w + GAP)).abs() < 0.01);
    }

    #[test]
    fn a_short_last_row_stays_left_aligned_like_the_live_grid() {
        let placements = layout_tiles(1280.0, 720.0, &plain(3));
        let first = rect_of(&placements, 0);
        let third = rect_of(&placements, 2);
        assert_eq!(third.x, first.x);
        assert!(third.y > first.y);
    }

    #[test]
    fn tiles_never_leave_the_frame() {
        for count in 1..=12 {
            for placement in layout_tiles(1280.0, 720.0, &plain(count)) {
                let rect = placement.rect;
                assert!(rect.x >= -0.5, "count {count}");
                assert!(rect.y >= -0.5, "count {count}");
                assert!(rect.x + rect.w <= 1280.5, "count {count}");
                assert!(rect.y + rect.h <= 720.5, "count {count}");
            }
        }
    }

    #[test]
    fn a_grid_never_overlaps_two_tiles() {
        let placements = layout_tiles(1280.0, 720.0, &plain(5));
        for (i, a) in placements.iter().enumerate() {
            for b in placements.iter().skip(i + 1) {
                let (a, b) = (a.rect, b.rect);
                let separated = a.x + a.w <= b.x + 0.5
                    || b.x + b.w <= a.x + 0.5
                    || a.y + a.h <= b.y + 0.5
                    || b.y + b.h <= a.y + 0.5;
                assert!(separated, "tiles overlap: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn a_crowd_past_the_last_layout_grows_rows_instead_of_paging() {
        let placements = layout_tiles(1280.0, 720.0, &plain(30));
        assert_eq!(placements.len(), 30);
        assert!(placements.iter().all(|p| p.rect.y + p.rect.h <= 720.5));
    }

    #[test]
    fn a_fullscreen_share_is_the_only_thing_drawn() {
        let mut tiles = plain(4);
        tiles[1].contain = true;
        tiles[1].focused = true;
        tiles[1].fullscreen = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].tile, 1);
        assert_eq!(
            placements[0].rect,
            TileRect {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 720.0
            }
        );
    }

    #[test]
    fn a_focused_share_keeps_its_own_thumbnail_in_the_strip() {
        let mut tiles = plain(3);
        tiles[0].contain = true;
        tiles[0].focused = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        let strip = thumbnails(&placements);
        assert_eq!(strip.len(), 3, "the share is in the strip as well");
        assert!(strip.iter().any(|p| p.tile == 0));
    }

    #[test]
    fn a_focused_camera_is_left_out_of_the_strip() {
        let mut tiles = plain(3);
        tiles[0].focused = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        let strip = thumbnails(&placements);
        assert_eq!(strip.len(), 2);
        assert!(strip.iter().all(|p| p.tile != 0));
    }

    #[test]
    fn the_strip_is_centred_and_capped_in_height() {
        let mut tiles = plain(3);
        tiles[0].focused = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        let strip = thumbnails(&placements);
        let first = strip.first().expect("a thumbnail").rect;
        let last = strip.last().expect("a thumbnail").rect;
        assert!(first.h <= STRIP_MAX_HEIGHT + 0.01);
        assert!((first.w / first.h - STRIP_ASPECT_RATIO).abs() < 0.01);
        let lead = first.x - PADDING;
        let trail = (1280.0 - PADDING) - (last.x + last.w);
        assert!((lead - trail).abs() < 1.0, "centred: {lead} vs {trail}");
    }

    #[test]
    fn the_main_tile_sits_above_the_strip_with_one_gap() {
        let mut tiles = plain(2);
        tiles[0].focused = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        let main = rect_of(&placements, 0);
        let thumb = thumbnails(&placements)[0].rect;
        assert_eq!(main.x, PADDING);
        assert_eq!(main.y, PADDING);
        assert!((thumb.y - (main.y + main.h + GAP)).abs() < 0.01);
        assert!((thumb.y + thumb.h - (720.0 - PADDING)).abs() < 0.01);
    }

    #[test]
    fn a_lone_focused_share_gets_no_strip_of_itself() {
        let mut tiles = plain(1);
        tiles[0].focused = true;
        tiles[0].contain = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        assert_eq!(placements.len(), 1);
        assert!(!placements[0].thumbnail);
    }

    #[test]
    fn a_focused_tile_alone_uses_the_whole_frame() {
        let mut tiles = plain(1);
        tiles[0].focused = true;
        let placements = layout_tiles(1280.0, 720.0, &tiles);
        assert_eq!(placements.len(), 1);
        assert_eq!(
            placements[0].rect,
            TileRect {
                x: PADDING,
                y: PADDING,
                w: 1280.0 - PADDING * 2.0,
                h: 720.0 - PADDING * 2.0,
            }
        );
    }

    #[test]
    fn a_strip_that_cannot_fit_everyone_drops_the_overflow() {
        let mut tiles = plain(41);
        tiles[0].focused = true;
        tiles[0].contain = true;
        let strip = thumbnails(&layout_tiles(1280.0, 720.0, &tiles)).len();
        assert!(strip > 0 && strip < 41);
    }
}
