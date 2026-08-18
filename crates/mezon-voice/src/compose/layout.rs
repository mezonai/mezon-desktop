pub const GAP: f32 = 10.0;
pub const RADIUS: f32 = 12.0;

const FOCUS_STRIP_SCREEN_SHARE: f32 = 0.13;
const FOCUS_STRIP_CAMERA: f32 = 0.17;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct TileShape {
    pub focused: bool,
    pub contain: bool,
}

pub fn layout_tiles(width: f32, height: f32, tiles: &[TileShape]) -> Vec<Option<TileRect>> {
    let count = tiles.len();
    if count == 0 {
        return Vec::new();
    }

    let focus_index = tiles.iter().position(|tile| tile.focused);
    match focus_index {
        Some(focus) => layout_with_focus(width, height, tiles, focus),
        None => layout_grid(width, height, count),
    }
}

fn layout_with_focus(
    width: f32,
    height: f32,
    tiles: &[TileShape],
    focus: usize,
) -> Vec<Option<TileRect>> {
    let count = tiles.len();
    let mut rects: Vec<Option<TileRect>> = vec![None; count];
    let others: Vec<usize> = (0..count).filter(|index| *index != focus).collect();

    let focus_is_screen_share = tiles[focus].contain;
    let strip_height = if others.is_empty() {
        0.0
    } else {
        let ratio = if focus_is_screen_share {
            FOCUS_STRIP_SCREEN_SHARE
        } else {
            FOCUS_STRIP_CAMERA
        };
        (height * ratio).round()
    };

    rects[focus] = Some(if focus_is_screen_share {
        TileRect {
            x: 0.0,
            y: 0.0,
            w: width,
            h: height,
        }
    } else {
        TileRect {
            x: GAP,
            y: GAP,
            w: width - GAP * 2.0,
            h: height - strip_height - GAP * 2.0,
        }
    });

    if others.is_empty() {
        return rects;
    }

    let cell_height = strip_height - GAP;
    let cell_width = (cell_height * 16.0 / 9.0).round();
    let max_visible = (((width - GAP) / (cell_width + GAP)).floor() as isize).max(1) as usize;
    let visible = others.len().min(max_visible);
    let strip_width = visible as f32 * cell_width + (visible as f32 - 1.0) * GAP;
    let strip_x = if focus_is_screen_share {
        width - strip_width - GAP
    } else {
        ((width - strip_width) / 2.0).round()
    };
    let strip_y = height - strip_height;

    for (position, index) in others.iter().enumerate() {
        rects[*index] = (position < visible).then_some(TileRect {
            x: strip_x + position as f32 * (cell_width + GAP),
            y: strip_y,
            w: cell_width,
            h: cell_height,
        });
    }

    rects
}

fn layout_grid(width: f32, height: f32, count: usize) -> Vec<Option<TileRect>> {
    let mut best_cols = 1usize;
    let mut best_width = 0.0f32;
    let mut best_height = 0.0f32;

    for cols in 1..=count {
        let rows = count.div_ceil(cols);
        let avail_width = (width - GAP * (cols as f32 + 1.0)) / cols as f32;
        let avail_height = (height - GAP * (rows as f32 + 1.0)) / rows as f32;
        if avail_width <= 0.0 || avail_height <= 0.0 {
            continue;
        }
        let scale = (avail_width / 16.0).min(avail_height / 9.0);
        let tile_width = scale * 16.0;
        let tile_height = scale * 9.0;
        if tile_width * tile_height > best_width * best_height {
            best_cols = cols;
            best_width = tile_width;
            best_height = tile_height;
        }
    }

    let row_count = count.div_ceil(best_cols);
    let grid_top =
        (height - (row_count as f32 * best_height + (row_count as f32 - 1.0) * GAP)) / 2.0;

    (0..count)
        .map(|index| {
            let row = index / best_cols;
            let col = index - row * best_cols;
            let in_row = best_cols.min(count - row * best_cols);
            let row_left =
                (width - (in_row as f32 * best_width + (in_row as f32 - 1.0) * GAP)) / 2.0;
            Some(TileRect {
                x: row_left + col as f32 * (best_width + GAP),
                y: grid_top + row as f32 * (best_height + GAP),
                w: best_width,
                h: best_height,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(count: usize) -> Vec<TileShape> {
        vec![
            TileShape {
                focused: false,
                contain: false,
            };
            count
        ]
    }

    #[test]
    fn an_empty_scene_lays_out_nothing() {
        assert!(layout_tiles(1280.0, 720.0, &[]).is_empty());
    }

    #[test]
    fn a_single_tile_keeps_the_sixteen_by_nine_shape() {
        let rects = layout_tiles(1280.0, 720.0, &plain(1));
        let rect = rects[0].expect("one rect");
        assert!((rect.w / rect.h - 16.0 / 9.0).abs() < 0.01);
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
        assert!(rect.x + rect.w <= 1280.0 + 0.5);
    }

    #[test]
    fn tiles_never_leave_the_frame() {
        for count in 1..=12 {
            for rect in layout_tiles(1280.0, 720.0, &plain(count))
                .into_iter()
                .flatten()
            {
                assert!(rect.x >= -0.5, "count {count}");
                assert!(rect.y >= -0.5, "count {count}");
                assert!(rect.x + rect.w <= 1280.5, "count {count}");
                assert!(rect.y + rect.h <= 720.5, "count {count}");
            }
        }
    }

    #[test]
    fn a_grid_never_overlaps_two_tiles() {
        let rects: Vec<TileRect> = layout_tiles(1280.0, 720.0, &plain(5))
            .into_iter()
            .flatten()
            .collect();
        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                let separated = a.x + a.w <= b.x + 0.5
                    || b.x + b.w <= a.x + 0.5
                    || a.y + a.h <= b.y + 0.5
                    || b.y + b.h <= a.y + 0.5;
                assert!(separated, "tiles overlap: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn a_focused_screen_share_takes_the_whole_frame() {
        let tiles = vec![
            TileShape {
                focused: true,
                contain: true,
            },
            TileShape {
                focused: false,
                contain: false,
            },
        ];
        let rects = layout_tiles(1280.0, 720.0, &tiles);
        let focus = rects[0].expect("focus rect");
        assert_eq!(
            focus,
            TileRect {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 720.0
            }
        );
        let thumb = rects[1].expect("thumb rect");
        assert!(thumb.y > 720.0 * 0.8, "thumbnail sits in the bottom strip");
        assert!(thumb.x + thumb.w <= 1280.5);
    }

    #[test]
    fn a_focused_camera_leaves_room_for_the_strip() {
        let tiles = vec![
            TileShape {
                focused: true,
                contain: false,
            },
            TileShape {
                focused: false,
                contain: false,
            },
        ];
        let rects = layout_tiles(1280.0, 720.0, &tiles);
        let focus = rects[0].expect("focus rect");
        assert!(focus.x >= GAP - 0.01);
        assert!(focus.y + focus.h < 720.0 - 720.0 * FOCUS_STRIP_CAMERA + GAP);
    }

    #[test]
    fn a_focused_tile_alone_uses_no_strip() {
        let tiles = vec![TileShape {
            focused: true,
            contain: false,
        }];
        let rects = layout_tiles(1280.0, 720.0, &tiles);
        let focus = rects[0].expect("focus rect");
        assert!((focus.h - (720.0 - GAP * 2.0)).abs() < 0.01);
    }

    #[test]
    fn a_strip_that_cannot_fit_everyone_drops_the_overflow() {
        let mut tiles = vec![TileShape {
            focused: true,
            contain: true,
        }];
        tiles.extend(plain(40));
        let rects = layout_tiles(1280.0, 720.0, &tiles);
        assert!(rects.iter().skip(1).any(Option::is_none));
        assert!(rects.iter().skip(1).any(Option::is_some));
    }
}
