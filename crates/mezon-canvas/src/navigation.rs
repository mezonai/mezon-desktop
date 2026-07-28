use gpui::{App, Global, Window};
use mezon_store::{ChannelId, ClanId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanvasRoute {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
    pub canvas_id: ChannelId,
}

pub struct CanvasNavigationHooks {
    pub navigate_to_canvas: fn(CanvasRoute, &mut App),
    pub navigate_to_channel: fn(ClanId, ChannelId, &mut App),
    pub active_canvas_id: fn(&App) -> Option<String>,
    pub confirm_delete_canvas: fn(String, String, ClanId, ChannelId, &str, &mut Window, &mut App),
}

struct GlobalCanvasNavigation(CanvasNavigationHooks);
impl Global for GlobalCanvasNavigation {}

pub fn set_navigation(hooks: CanvasNavigationHooks, cx: &mut App) {
    cx.set_global(GlobalCanvasNavigation(hooks));
}

fn hooks(cx: &App) -> Option<&CanvasNavigationHooks> {
    cx.try_global::<GlobalCanvasNavigation>()
        .map(|global| &global.0)
}

pub fn navigate_to_canvas(route: CanvasRoute, cx: &mut App) {
    if let Some(hooks) = hooks(cx) {
        (hooks.navigate_to_canvas)(route, cx);
    }
}

pub fn navigate_to_channel(clan_id: ClanId, channel_id: ChannelId, cx: &mut App) {
    if let Some(hooks) = hooks(cx) {
        (hooks.navigate_to_channel)(clan_id, channel_id, cx);
    }
}

pub fn active_canvas_id(cx: &App) -> Option<String> {
    hooks(cx).and_then(|hooks| (hooks.active_canvas_id)(cx))
}

pub fn confirm_delete_canvas(
    canvas_id: String,
    canvas_title: String,
    clan_id: ClanId,
    channel_id: ChannelId,
    locale: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(hooks) = hooks(cx) {
        (hooks.confirm_delete_canvas)(
            canvas_id,
            canvas_title,
            clan_id,
            channel_id,
            locale,
            window,
            cx,
        );
    }
}
