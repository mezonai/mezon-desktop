use gpui::App;
use mezon_canvas::{CanvasNavigationHooks, set_navigation};

use crate::app::shell::Shell;
use crate::router::{Route, Router, navigate};

pub fn init(cx: &mut App) {
    set_navigation(
        CanvasNavigationHooks {
            navigate_to_canvas: |route, cx| {
                navigate(
                    cx,
                    Route::Canvas {
                        clan_id: route.clan_id,
                        channel_id: route.channel_id,
                        canvas_id: route.canvas_id,
                    },
                );
            },
            navigate_to_channel: |clan_id, channel_id, cx| {
                navigate(
                    cx,
                    Route::Channel {
                        clan_id,
                        channel_id,
                    },
                );
            },
            active_canvas_id: |cx| match Router::global(cx).read(cx).route() {
                Route::Canvas { canvas_id, .. } => Some(canvas_id.to_string()),
                _ => None,
            },
            confirm_delete_canvas: |canvas_id,
                                    canvas_title,
                                    clan_id,
                                    channel_id,
                                    locale,
                                    window,
                                    cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.confirm_delete_canvas(
                        canvas_id,
                        canvas_title,
                        clan_id,
                        channel_id,
                        locale,
                        window,
                        cx,
                    );
                });
            },
        },
        cx,
    );
}
