use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, App, ClickEvent, Entity, FontWeight, MouseButton, ObjectFit, Pixels,
    SharedString, Transformation, Window, div, img, prelude::*, px, radians, rems, rgba,
};
use mezon_store::{
    AlbumLayout, AppConfig, ChannelType, ClanMembersStore, Message, MessageAttachment, MessageCode,
    MessageId, MessageReference, MessagesStore, PlatformStore, ProfileContext, Reaction,
    ThreadsStore, TopicsStore, ViewerMedia, resolve_avatar_url,
};
use smallvec::SmallVec;

use super::audio_player::{
    AudioActivation, audio_failed_pill, audio_pill, audio_sending_pill, audio_time_label,
};
use super::content::{SELECTION_BG, SelectableTextContext, profile_popover_trigger};
use super::context::{REPLY_USERNAME_COLOR, RowCtx};
use super::gif_video::GifVideoView;
use super::reaction_detail::{UserReactionPanel, emoji_error_fallback};
use super::selection::{SelectableRegion, TextSegment};
use super::time::format_message_time;
use super::video_player::{VideoActivation, VideoFullscreenMode, VideoLayout};
use crate::chat::user_profile_popover::{ClickableContainer, profile_popover_menu};
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size, Spinner};
use crate::theme::Theme;

const DELETED_REPLY_PREVIEW: &str = "Original message was deleted";
const FILE_NAME_COLOR: u32 = 0x3b_82_f6;

pub fn resolve_message_display_name(msg: &Message, ctx: &RowCtx, cx: &App) -> SharedString {
    let user_id = msg
        .sender_user_id
        .or_else(|| msg.sender_id.parse().ok().map(mezon_store::UserId));
    let clan_id = match ctx.profile_context {
        Some(ProfileContext::Clan(clan_id)) => Some(clan_id),
        _ => None,
    };
    if let Some(user_id) = user_id {
        let key = (clan_id, user_id);
        if let Some(cached) = ctx.row_memo.borrow().display_names.get(&key) {
            return cached.clone();
        }
        let resolved = if let Some(clan_id) = clan_id {
            ClanMembersStore::global(cx)
                .read(cx)
                .member(clan_id, user_id)
                .map(|member| member.name())
                .filter(|name| !name.is_empty())
                .map(SharedString::from)
                .unwrap_or_else(|| msg.sender_name.clone())
        } else {
            msg.sender_name.clone()
        };
        let mut memo = ctx.row_memo.borrow_mut();
        if memo.display_names.len() >= 4096 {
            memo.display_names.clear();
        }
        memo.display_names.insert(key, resolved.clone());
        return resolved;
    }
    msg.sender_name.clone()
}

pub fn avatar_element(msg: &Message, ctx: &RowCtx, cx: &App) -> AnyElement {
    let is_anonymous = AppConfig::try_global(cx)
        .map(|config| {
            !config.anonymous_user_id.is_empty() && msg.sender_id == config.anonymous_user_id
        })
        .unwrap_or(false);
    let (raw_url, proxied) = resolve_message_avatar_urls(msg, ctx, cx);
    let display_name = resolve_message_display_name(msg, ctx, cx);
    let mut avatar = Avatar::new()
        .name(display_name)
        .with_size(Size::Small)
        .anonymous(is_anonymous)
        .image_cache(ctx.avatar_cache.clone());
    if is_anonymous {
        return avatar.into_any_element();
    }
    if let Some(proxied) = proxied {
        avatar = avatar.src(proxied);
        if !raw_url.is_empty() {
            avatar = avatar.fallback_src(raw_url);
        }
    } else if !raw_url.is_empty() {
        avatar = avatar.src(raw_url);
    }
    avatar.into_any_element()
}

fn resolve_message_avatar_urls(
    msg: &Message,
    ctx: &RowCtx,
    cx: &App,
) -> (SharedString, Option<SharedString>) {
    if let Some(context) = ctx.profile_context
        && let Some(user_id) = msg.sender_user_id
    {
        let cached = ctx.row_memo.borrow().avatars.get(&user_id).cloned();
        let resolved = match cached {
            Some(resolved) => resolved,
            None => {
                let resolved = resolve_avatar_url(user_id, context, cx)
                    .filter(|url| !url.is_empty())
                    .map(|avatar_url| {
                        let proxied = if avatar_url.as_str() == msg.avatar_url.as_ref() {
                            msg.avatar_proxied.clone()
                        } else {
                            SharedString::from(crate::util::imgproxy::avatar_url(cx, &avatar_url))
                        };
                        (SharedString::from(avatar_url), proxied)
                    });
                ctx.row_memo
                    .borrow_mut()
                    .avatars
                    .insert(user_id, resolved.clone());
                resolved
            }
        };
        if let Some((raw_url, proxied)) = resolved {
            return (raw_url, Some(proxied));
        }
    }

    let proxied = msg.avatar_proxied.clone();
    if !proxied.is_empty() {
        return (msg.avatar_url.clone(), Some(proxied));
    }
    (msg.avatar_url.clone(), None)
}

pub fn render_head(msg: &Message, ctx: &RowCtx, name_color: u32) -> AnyElement {
    let theme = ctx.theme;
    let time_label = {
        let mut memo = ctx.row_memo.borrow_mut();
        match memo.time_labels.get(&msg.id) {
            Some(label) if !label.is_empty() || msg.time_hhmm.is_empty() => label.clone(),
            _ => {
                let label =
                    format_message_time(&msg.time_hhmm, msg.local_date, ctx.locale, ctx.now);
                if !label.is_empty() {
                    if memo.time_labels.len() >= 4096 {
                        memo.time_labels.clear();
                    }
                    memo.time_labels.insert(msg.id, label.clone());
                }
                label
            }
        }
    };
    let display_name = resolve_message_display_name(msg, ctx, ctx.app);
    let name = div()
        .max_w_full()
        .min_w_0()
        .text_size(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(gpui::rgb(name_color))
        .child(display_name);
    div()
        .flex()
        .flex_row()
        .w_full()
        .min_w_0()
        .items_baseline()
        .gap_2()
        .child(profile_name_trigger(msg, ctx, name))
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child(time_label),
        )
        .into_any_element()
}

fn profile_name_trigger(msg: &Message, ctx: &RowCtx, name: gpui::Div) -> AnyElement {
    let (Some(profile_ctx), Some(user_id)) = (ctx.profile_context, msg.sender_user_id) else {
        return name.into_any_element();
    };
    let key = user_id.get() as usize;
    profile_popover_trigger(
        ("msg-head-trigger", key),
        user_id,
        profile_ctx,
        ctx,
        name.into_any_element(),
    )
    .max_w_full()
    .min_w_0()
    .into_any_element()
}

pub fn render_reply(reference: &MessageReference, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    if reference.message_ref_id.is_zero() {
        return div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(24.))
            .pl(px(super::context::REPLY_INSET))
            .pr(px(super::context::CONTENT_RIGHT_PAD))
            .text_size(px(14.))
            .child(
                Icon::new(IconName::ReplyCorner)
                    .size_4()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme.tokens.bg_active_member_channel)
                    .child(
                        Icon::new(IconName::IconReplyMessDeletedWeb)
                            .size_4()
                            .text_color(theme.tokens.text_secondary),
                    ),
            )
            .child(
                div()
                    .italic()
                    .text_size(px(13.))
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(ctx.locale, "message.messageDeleteReply")),
            )
            .into_any_element();
    }

    let has_attachment_ref = reference.has_attachment || reference.has_embed;
    let is_deleted = reference.content == DELETED_REPLY_PREVIEW;
    let avatar = if reference.sender_avatar.is_empty() {
        Avatar::new()
            .name(reference.sender_name.clone())
            .size_px(px(20.))
            .image_cache(ctx.avatar_cache.clone())
    } else {
        Avatar::new()
            .name(reference.sender_name.clone())
            .src(reference.sender_avatar.clone())
            .size_px(px(20.))
            .image_cache(ctx.avatar_cache.clone())
    };

    let jump_target = reference.message_ref_id;
    div()
        .id(("reply", reference.message_ref_id.0 as usize))
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(24.))
        .pl(px(super::context::REPLY_INSET))
        .pr(px(super::context::CONTENT_RIGHT_PAD))
        .text_size(px(14.))
        .cursor_pointer()
        .when(!jump_target.is_zero(), |d| {
            d.on_click(move |_, _, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.jump_to_message(jump_target, cx));
            })
        })
        .child(
            Icon::new(IconName::ReplyCorner)
                .size_4()
                .text_color(theme.text_muted),
        )
        .child(reply_avatar_trigger(
            reference,
            avatar.into_any_element(),
            ctx,
        ))
        .child(
            div()
                .id(("reply-name", reference.message_ref_id.0 as usize))
                .flex_none()
                .whitespace_nowrap()
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::rgb(REPLY_USERNAME_COLOR))
                .hover(|s| s.underline())
                .child(reference.sender_name.clone()),
        )
        .child(if has_attachment_ref {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_color(theme.tokens.text_theme_primary)
                .child(
                    div()
                        .italic()
                        .child(mezon_i18n::t(ctx.locale, "chat.clickToSeeAttachment")),
                )
                .child(
                    Icon::new(IconName::ImageThumbnail)
                        .size_4()
                        .text_color(theme.tokens.text_theme_primary),
                )
                .into_any_element()
        } else if reference.is_poll {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_color(theme.tokens.text_theme_message)
                .child("📊")
                .child(mezon_i18n::t(ctx.locale, "message.poll.pollLabel"))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.tokens.text_theme_message)
                .when(is_deleted, |d| d.italic())
                .child(reference.content_preview.clone())
                .into_any_element()
        })
        .into_any_element()
}

fn reply_avatar_trigger(
    reference: &MessageReference,
    avatar: AnyElement,
    ctx: &RowCtx,
) -> AnyElement {
    let Some(profile_ctx) = ctx.profile_context else {
        return avatar;
    };
    if reference.sender_id.is_zero() {
        return avatar;
    }
    let user_id = reference.sender_id;
    let key = user_id.get() as usize;
    let menu = profile_popover_menu(
        ("reply-avatar-popover", key),
        user_id,
        profile_ctx,
        ctx.settings.clone(),
        ctx.avatar_cache.clone(),
    )
    .anchor(Anchor::TopLeft)
    .attach(Anchor::TopRight)
    .trigger(
        ClickableContainer::new(("reply-avatar-trigger", key))
            .flex_none()
            .cursor_pointer()
            .child(avatar),
    );
    div()
        .id(("reply-avatar-stop", key))
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(menu)
        .into_any_element()
}

pub fn render_attachments(
    msg: &Message,
    ctx: &RowCtx,
    range: Option<std::ops::Range<usize>>,
    selection_context: &SelectableTextContext,
) -> Option<AnyElement> {
    if msg.attachments.is_empty() {
        return None;
    }
    let theme = ctx.theme;
    let mut videos: SmallVec<[&MessageAttachment; 2]> = SmallVec::new();
    let mut audios: SmallVec<[&MessageAttachment; 2]> = SmallVec::new();
    let mut images: SmallVec<[(usize, &MessageAttachment); 4]> = SmallVec::new();
    let mut documents: SmallVec<[&MessageAttachment; 2]> = SmallVec::new();
    for (idx, att) in msg.attachments.iter().enumerate() {
        if att.is_unsupported_media() {
            documents.push(att);
        } else if att.is_video() {
            videos.push(att);
        } else if att.is_audio() {
            audios.push(att);
        } else if att.is_image() {
            images.push((idx, att));
        } else {
            documents.push(att);
        }
    }

    let uploader = Uploader {
        name: msg.sender_name.clone(),
        avatar: if msg.avatar_proxied.is_empty() {
            msg.avatar_url.clone()
        } else {
            msg.avatar_proxied.clone()
        },
    };

    let mut col = div()
        .id(("msg-attachments", msg.id.0 as usize))
        .flex()
        .flex_col()
        .gap_2()
        .mt_1()
        .w_full();
    for (i, att) in videos.iter().enumerate() {
        col = col.child(render_video(msg.id, i, att, ctx, att.uploading));
    }
    for (i, att) in audios.iter().enumerate() {
        col = col.child(render_audio(msg.id, i, att, ctx, att.uploading));
    }
    if images.len() >= 2
        && let Some(layout) = msg.album_layout.as_ref()
    {
        col = col.child(render_album(
            &images,
            layout,
            &msg.viewer_media,
            theme,
            &uploader,
            msg,
            ctx,
        ));
    } else if let Some(&(att_index, att)) = images.first() {
        let gif_player = att
            .tenor_mp4
            .as_ref()
            .and_then(|_| ctx.gif_videos.get(&(msg.id, att_index)).cloned());
        col = col.child(render_photo(
            0,
            att,
            msg,
            ctx,
            &msg.viewer_media,
            &uploader,
            gif_player,
            att.uploading,
        ));
    }
    for (i, att) in documents.iter().enumerate() {
        col = col.child(render_file_box(i, att, msg, ctx));
    }
    let bounds = Rc::new(Cell::new(None));
    let selected = range
        .as_ref()
        .is_some_and(|range| selection_context.is_selected(range));
    if let Some(range) = range {
        selection_context.push_segment(TextSegment::block(range, bounds.clone()));
    }
    Some(
        SelectableRegion::new(
            col.into_any_element(),
            bounds,
            selected.then(|| rgba(SELECTION_BG)),
        )
        .into_any_element(),
    )
}

#[allow(dead_code)]
struct Uploader {
    name: SharedString,
    avatar: SharedString,
}

fn attachment_spinner(size: Pixels, theme: &Theme) -> impl IntoElement {
    div()
        .size(size)
        .rounded_full()
        .border_2()
        .border_color(theme.text_secondary)
}

fn attachment_sending_overlay(theme: &Theme) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            Spinner::new()
                .with_size(Size::Large)
                .color(theme.text_secondary.into()),
        )
}

fn attachment_failed_overlay(theme: &Theme) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(40.))
                .rounded_full()
                .bg(theme.bg_floating)
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(24.))
                        .text_color(theme.status_dnd),
                ),
        )
}

fn render_audio(
    msg_id: MessageId,
    index: usize,
    att: &MessageAttachment,
    ctx: &RowCtx,
    sending: bool,
) -> AnyElement {
    let duration = att.duration.max(0) as f64;
    if att.upload_failed {
        return audio_failed_pill(duration);
    }
    if sending {
        return audio_sending_pill(duration);
    }
    if let Some(view) = ctx.active_audios.get(&(msg_id, index)) {
        return div().w_full().child(view.clone()).into_any_element();
    }
    let url = SharedString::from(att.url.clone());
    let host = ctx.video_host.clone();
    let download_url = url.clone();
    let download_name = if att.filename.is_empty() {
        SharedString::from("audio")
    } else {
        SharedString::from(att.filename.clone())
    };
    let activate_download_url = download_url.clone();
    let activate_download_name = download_name.clone();
    let activate_selection = ctx.selection.clone();
    let download_selection = ctx.selection.clone();
    audio_pill(
        ("audio-play", index),
        ("audio-dl", index),
        false,
        audio_time_label(0.0, duration),
        move |_, _, cx| {
            if activate_selection.borrow().has_selection() {
                return;
            }
            let activation = AudioActivation {
                url: url.clone(),
                duration,
                download_url: activate_download_url.clone(),
                download_name: activate_download_name.clone(),
            };
            let _ = host.update(cx, |this, cx| {
                this.activate_audio((msg_id, index), activation, cx);
            });
        },
        move |_, _, cx| {
            if download_selection.borrow().has_selection() {
                return;
            }
            crate::util::download::save_with_progress_toast(
                download_url.clone(),
                download_name.clone(),
                cx,
            )
        },
    )
}

fn render_album(
    images: &[(usize, &MessageAttachment)],
    layout: &AlbumLayout,
    _gallery: &Arc<[ViewerMedia]>,
    theme: &Theme,
    _uploader: &Uploader,
    msg: &Message,
    ctx: &RowCtx,
) -> AnyElement {
    let cfg = AppConfig::try_global(ctx.app);
    let mut container = div()
        .relative()
        .w(px(layout.container_width))
        .h(px(layout.container_height))
        .max_w(px(464.))
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_tertiary);
    for (index, (tile, image)) in layout.tiles.iter().zip(images.iter()).enumerate() {
        let att = image.1;
        let settings = ctx.settings.clone();
        let raw_url = SharedString::from(att.url.clone());
        let anchor = (msg.create_time + 86_400).max(0) as u32;
        let mut tile_element = div()
            .id(("msg-album", index))
            .absolute()
            .left(px(tile.x))
            .top(px(tile.y))
            .w(px(tile.width))
            .h(px(tile.height))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .bg(theme.bg_tertiary);
        if let Some(path) = att.local_source.clone() {
            let selection = ctx.selection.clone();
            tile_element = tile_element.when(
                !att.uploading && !att.upload_failed && !raw_url.is_empty(),
                |d| {
                    d.cursor_pointer().on_click(move |_, _window, cx| {
                        if !selection.borrow().has_selection() {
                            open_viewer_from_message(&settings, raw_url.clone(), anchor, cx);
                        }
                    })
                },
            );
            tile_element = tile_element.child(cover_tile_image(
                img(path),
                att.width,
                att.height,
                tile.width,
                tile.height,
            ));
        } else if att.presign_pending {
            tile_element = presign_child(tile_element, att, theme);
        } else {
            let selection = ctx.selection.clone();
            let src = album_tile_src(cfg, att, tile.width, tile.height);
            tile_element = tile_element
                .cursor_pointer()
                .when(!src.is_empty(), |d| {
                    d.child(
                        img(src)
                            .id(("msg-album-frames", index))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .on_click(move |_, _window, cx| {
                    if !selection.borrow().has_selection() {
                        open_viewer_from_message(&settings, raw_url.clone(), anchor, cx);
                    }
                });
        }
        if att.upload_failed {
            tile_element = tile_element.child(attachment_failed_overlay(theme));
        } else if att.uploading {
            tile_element = tile_element.child(attachment_sending_overlay(theme));
        }
        container = container.child(tile_element);
    }
    container.into_any_element()
}

fn album_tile_src(
    cfg: Option<&AppConfig>,
    att: &MessageAttachment,
    tile_width: f32,
    tile_height: f32,
) -> SharedString {
    let tile_w = (tile_width.round() as u32).max(1);
    let tile_h = (tile_height.round() as u32).max(1);
    let resize = if att.width < tile_w || att.height < tile_h {
        "fill-down"
    } else {
        "fill"
    };
    cfg.map(|c| c.imgproxy_url(&att.url, tile_w, tile_h, resize))
        .filter(|src| !src.is_empty())
        .map(SharedString::from)
        .unwrap_or_else(|| att.proxied_src.clone())
}

fn cover_tile_image(
    image: gpui::Img,
    natural_width: u32,
    natural_height: u32,
    tile_width: f32,
    tile_height: f32,
) -> AnyElement {
    if natural_width == 0 || natural_height == 0 {
        return image
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element();
    }
    let iw = natural_width as f32;
    let ih = natural_height as f32;
    let scale = (tile_width / iw).max(tile_height / ih);
    let rendered_w = iw * scale;
    let rendered_h = ih * scale;
    image
        .absolute()
        .left(px((tile_width - rendered_w) / 2.0))
        .top(px((tile_height - rendered_h) / 2.0))
        .w(px(rendered_w))
        .h(px(rendered_h))
        .into_any_element()
}

fn presign_child(
    parent: gpui::Stateful<gpui::Div>,
    att: &MessageAttachment,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    if att.thumbnail.is_empty() {
        parent.child(attachment_spinner(px(30.), theme))
    } else {
        parent.child(
            img(SharedString::from(att.thumbnail.clone()))
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
    }
}

fn open_external(url: &str, cx: &mut App) {
    if let Some(store) = PlatformStore::try_global(cx) {
        let _ = store.read(cx).open_url_external(url);
    }
}

fn render_photo(
    index: usize,
    att: &MessageAttachment,
    msg: &Message,
    ctx: &RowCtx,
    _gallery: &Arc<[ViewerMedia]>,
    _uploader: &Uploader,
    gif_player: Option<Entity<GifVideoView>>,
    sending: bool,
) -> AnyElement {
    if let Some(player) = gif_player {
        return div()
            .id(("msg-gif", index))
            .w(px(att.display_width))
            .h(px(att.display_height))
            .max_w_full()
            .child(player)
            .into_any_element();
    }
    let theme = ctx.theme;
    if let Some(path) = att.local_source.clone() {
        let settings = ctx.settings.clone();
        let raw_url = SharedString::from(att.url.clone());
        let anchor = (msg.create_time + 86_400).max(0) as u32;
        let fallback_bg = theme.bg_tertiary;
        let fallback_fg = theme.text_muted;
        let selection = ctx.selection.clone();
        let mut el = div()
            .id(("msg-img", index))
            .relative()
            .w(px(att.display_width))
            .h(px(att.display_height))
            .rounded_md()
            .overflow_hidden()
            .bg(theme.bg_tertiary);
        el = el.when(!sending && !att.upload_failed && !raw_url.is_empty(), |d| {
            d.cursor_pointer().on_click(move |_, _window, cx| {
                if !selection.borrow().has_selection() {
                    open_viewer_from_message(&settings, raw_url.clone(), anchor, cx);
                }
            })
        });
        el = el.child(
            img(path)
                .id(("msg-img-frames", msg.id.0 as usize))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(move || {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(fallback_bg)
                        .child(
                            Icon::new(IconName::ImageThumbnail)
                                .size(px(32.))
                                .text_color(fallback_fg),
                        )
                        .into_any_element()
                }),
        );
        if att.upload_failed {
            el = el.child(attachment_failed_overlay(theme));
        } else if sending {
            el = el.child(attachment_sending_overlay(theme));
        }
        return el.into_any_element();
    }
    if att.presign_pending {
        let mut placeholder = div()
            .id(("msg-img", index))
            .relative()
            .w(px(att.display_width))
            .h(px(att.display_height))
            .rounded_md()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.bg_tertiary);
        placeholder = presign_child(placeholder, att, theme);
        if att.upload_failed {
            placeholder = placeholder.child(attachment_failed_overlay(theme));
        } else if sending {
            placeholder = placeholder.child(attachment_sending_overlay(theme));
        }
        return placeholder.into_any_element();
    }
    let src = att.proxied_src.clone();
    if src.is_empty() {
        return attachment_box(att.filename.clone(), theme);
    }
    let object_fit = if is_gif(&att.url) {
        ObjectFit::Contain
    } else {
        ObjectFit::Cover
    };
    let fallback_bg = theme.bg_tertiary;
    let fallback_fg = theme.text_muted;
    let is_sticker = att.filetype == "sticker";
    let settings = ctx.settings.clone();
    let raw_url = SharedString::from(att.url.clone());
    let anchor = (msg.create_time + 86_400).max(0) as u32;
    let selection = ctx.selection.clone();
    let mut el = div()
        .id(("msg-img", index))
        .relative()
        .w(px(att.display_width))
        .h(px(att.display_height))
        .rounded_md()
        .overflow_hidden()
        .bg(theme.bg_tertiary);
    el = el.when(!is_sticker && !att.upload_failed, |d| {
        d.cursor_pointer().on_click(move |_, _window, cx| {
            if !selection.borrow().has_selection() {
                open_viewer_from_message(&settings, raw_url.clone(), anchor, cx);
            }
        })
    });
    el = el.child(
        img(src)
            .id(("msg-img-frames", msg.id.0 as usize))
            .size_full()
            .object_fit(object_fit)
            .with_fallback(move || {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(fallback_bg)
                    .child(
                        Icon::new(IconName::ImageThumbnail)
                            .size(px(32.))
                            .text_color(fallback_fg),
                    )
                    .into_any_element()
            }),
    );
    if att.upload_failed {
        el = el.child(attachment_failed_overlay(theme));
    } else if sending {
        el = el.child(attachment_sending_overlay(theme));
    }
    el.into_any_element()
}

fn render_video(
    msg_id: MessageId,
    index: usize,
    att: &MessageAttachment,
    ctx: &RowCtx,
    sending: bool,
) -> AnyElement {
    if !sending && let Some(view) = ctx.active_videos.get(&(msg_id, index)) {
        return div()
            .w(px(att.display_width))
            .h(px(att.display_height))
            .max_w_full()
            .child(view.clone())
            .into_any_element();
    }
    render_video_poster(msg_id, index, att, ctx, sending)
}

fn render_video_poster(
    msg_id: MessageId,
    index: usize,
    att: &MessageAttachment,
    ctx: &RowCtx,
    sending: bool,
) -> AnyElement {
    let theme = ctx.theme;
    let url = SharedString::from(att.url.clone());
    let filename = SharedString::from(att.filename.clone());
    let thumbnail = att.thumbnail_proxied.clone();
    let width = att.display_width;
    let height = att.display_height;
    let host = ctx.video_host.clone();
    let selection = ctx.selection.clone();
    let container = div()
        .id(("msg-video", index))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(att.display_width))
        .h(px(att.display_height))
        .max_w_full()
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_tertiary)
        .when(!thumbnail.is_empty(), |d| {
            d.child(
                img(thumbnail.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
        });
    if att.upload_failed {
        return container
            .child(attachment_failed_overlay(theme))
            .into_any_element();
    }
    if sending {
        return container
            .child(attachment_sending_overlay(theme))
            .into_any_element();
    }
    let overlay = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.3,
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(48.))
                .h(px(48.))
                .rounded_full()
                .bg(gpui::Rgba {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.5,
                })
                .child(
                    Icon::new(IconName::PlayButton)
                        .size(px(20.))
                        .text_color(gpui::white()),
                ),
        );
    container
        .cursor_pointer()
        .child(overlay)
        .on_click(move |_, window, cx| {
            if selection.borrow().has_selection() {
                return;
            }
            let activation = VideoActivation {
                url: url.clone(),
                filename: filename.clone(),
                poster: thumbnail.clone(),
                width,
                height,
                fullscreen_mode: VideoFullscreenMode::default(),
                layout: VideoLayout::default(),
                decode_max_size: None,
            };
            let _ = host.update(cx, |host, cx| {
                host.activate_video((msg_id, index), activation, window, cx);
            });
        })
        .into_any_element()
}

fn file_icon_for(filetype: &str) -> IconName {
    if filetype.starts_with("image/") {
        IconName::ImageThumbnail
    } else {
        IconName::FileIcon
    }
}

fn file_box_action(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded_md()
        .bg(theme.tokens.bg_theme_contexify)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .cursor_pointer()
        .hover(|s| s.opacity(0.8))
        .on_click(on_click)
        .child(
            Icon::new(icon)
                .size(px(16.))
                .text_color(theme.tokens.text_theme_primary),
        )
}

fn render_file_box(
    index: usize,
    att: &MessageAttachment,
    msg: &Message,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let sending = att.uploading;
    let failed = att.upload_failed;
    let is_owner = ctx.current_user_id == msg.sender_id.as_str();
    let filename = if att.filename.is_empty() {
        SharedString::from("Attachment")
    } else {
        SharedString::from(att.filename.clone())
    };
    let is_pdf =
        att.filetype == "application/pdf" || att.filename.to_ascii_lowercase().ends_with(".pdf");
    let url = SharedString::from(att.url.clone());
    let size_line = SharedString::from(format!("size: {}", att.size_label));
    let group_name = SharedString::from(format!("file-box-{}-{}", msg.id.0, index));

    let download_url = url.clone();
    let download_name = filename.clone();
    let body_url = url.clone();
    let pdf_url = url.clone();
    let body_selection = ctx.selection.clone();
    let download_selection = ctx.selection.clone();
    let remove_selection = ctx.selection.clone();
    let pdf_selection = ctx.selection.clone();

    div()
        .id((
            "file-box",
            (msg.id.0 as usize).wrapping_mul(31).wrapping_add(index),
        ))
        .group(group_name.clone())
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .w_full()
        .max_w_full()
        .mt(px(10.))
        .p_3()
        .rounded_lg()
        .bg(theme.tokens.bg_item_theme_hover)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .child(
            div()
                .relative()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(40.))
                .when(!sending && !failed, |d| {
                    d.child(
                        Icon::new(file_icon_for(&att.filetype))
                            .size(px(30.))
                            .text_color(theme.tokens.text_theme_primary),
                    )
                })
                .when(sending, |d| {
                    d.child(
                        Spinner::new()
                            .with_size(Size::Medium)
                            .color(theme.tokens.text_theme_primary.into()),
                    )
                })
                .when(failed, |d| {
                    d.child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(28.))
                            .text_color(theme.status_dnd),
                    )
                }),
        )
        .child(
            div()
                .id((
                    "file-box-body",
                    (msg.id.0 as usize).wrapping_mul(31).wrapping_add(index),
                ))
                .flex_1()
                .min_w_0()
                .when(!sending && !failed, |d| {
                    d.cursor_pointer().on_click(move |_, _, cx| {
                        if !body_selection.borrow().has_selection() {
                            open_external(&body_url, cx);
                        }
                    })
                })
                .child(
                    div()
                        .truncate()
                        .text_size(px(16.))
                        .text_color(gpui::rgb(FILE_NAME_COLOR))
                        .when(!sending && !failed, |d| d.hover(|s| s.underline()))
                        .child(filename),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.tokens.text_theme_primary)
                        .child(size_line),
                ),
        )
        .when(!sending && !failed, |row| {
            row.child(
                div()
                    .absolute()
                    .right(px(16.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .opacity(0.)
                    .group_hover(group_name, |s| s.opacity(1.))
                    .child(file_box_action(
                        ("file-dl", index),
                        IconName::Download,
                        theme,
                        move |_, _, cx| {
                            if download_selection.borrow().has_selection() {
                                return;
                            }
                            crate::util::download::save_with_progress_toast(
                                download_url.clone(),
                                download_name.clone(),
                                cx,
                            )
                        },
                    ))
                    .when(is_owner, |d| {
                        let remove_msg_id = msg.id;
                        d.child(file_box_action(
                            ("file-rm", index),
                            IconName::TrashIcon,
                            theme,
                            move |_, _, cx| {
                                if remove_selection.borrow().has_selection() {
                                    return;
                                }
                                mezon_store::MessagesStore::global(cx).update(cx, |store, cx| {
                                    store.remove_attachment(remove_msg_id, index, cx);
                                });
                            },
                        ))
                    })
                    .when(is_pdf, |d| {
                        d.child(file_box_action(
                            ("file-pdf", index),
                            IconName::FileIcon,
                            theme,
                            move |_, _, cx| {
                                if !pdf_selection.borrow().has_selection() {
                                    open_external(&pdf_url, cx);
                                }
                            },
                        ))
                    }),
            )
        })
        .into_any_element()
}

fn is_gif(url: &str) -> bool {
    url.contains(".gif")
}

fn attachment_box(label: String, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(240.))
        .h(px(120.))
        .rounded_md()
        .bg(theme.bg_tertiary)
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.text_muted)
        .child(if label.is_empty() {
            "image".to_string()
        } else {
            label
        })
        .into_any_element()
}

pub fn render_reactions(msg: &Message, ctx: &RowCtx) -> Option<AnyElement> {
    if msg.reactions.is_empty() {
        return None;
    }
    let mut row = div().flex().flex_row().flex_wrap().gap_2().mt_1().w_full();
    for reaction in msg.reactions.iter() {
        row = row.child(reaction_pill(reaction, msg.id, ctx));
    }
    row = row.child(add_reaction_button(msg.id, ctx));
    Some(row.into_any_element())
}

fn add_reaction_button(message_id: MessageId, ctx: &RowCtx) -> AnyElement {
    let visible = !ctx.suppress_hover
        && ctx.context_menu_message != Some(message_id)
        && ctx.hovered_row == Some(message_id);
    let bg = ctx.theme.bg_tertiary;
    let bg_hover = ctx.theme.bg_hover;
    let icon_color = ctx.theme.text_muted;
    let host = ctx.video_host.clone();
    div()
        .id("add-reaction")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_md()
        .when(visible, move |d| {
            d.cursor_pointer()
                .bg(bg)
                .hover(move |s| s.bg(bg_hover))
                .child(
                    Icon::new(IconName::Smile)
                        .size(px(20.))
                        .text_color(icon_color),
                )
                .on_click(move |_, window, cx| {
                    let position = window.mouse_position();
                    let _ = host.update(cx, |this, cx| {
                        this.open_reaction_picker(message_id, position, window, cx);
                    });
                })
        })
        .into_any_element()
}

const REACTION_EMOJI_PX: f32 = 16.;
const REACTION_EMOJI_SOURCE_PX: u32 = 32;

fn reaction_emoji_src(reaction: &Reaction, app: &gpui::App) -> SharedString {
    if reaction.emoji_id.is_empty() || reaction.emoji_id.as_ref() == "0" {
        return SharedString::default();
    }
    crate::util::imgproxy::emoji_url_sized(app, &reaction.emoji_id, REACTION_EMOJI_SOURCE_PX).into()
}

fn reaction_pill(reaction: &Reaction, message_id: MessageId, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let reacted = !ctx.current_user_id.is_empty() && reaction.has_sender(ctx.current_user_id);
    let count_label = reaction.count_label.clone();
    let src = reaction_emoji_src(reaction, ctx.app);

    let mut pill = div()
        .id(super::content::hashed_element_id(
            "reaction",
            &reaction.emoji_id,
        ))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .h(px(24.))
        .min_w(px(48.))
        .pl(px(28.))
        .pr(px(8.))
        .rounded_md()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.tokens.text_theme_primary);
    if !ctx.scroll_active {
        let add_emoji_id = reaction.emoji_id.clone();
        let add_emoji = reaction.emoji.clone();
        let panel_emoji_id = reaction.emoji_id.clone();
        let panel_emoji = reaction.emoji.clone();
        let avatar_cache = ctx.avatar_cache.clone();
        pill = pill
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.add_reaction(
                        message_id,
                        add_emoji_id.to_string(),
                        add_emoji.to_string(),
                        cx,
                    );
                });
            })
            .hoverable_tooltip(move |_window, cx| {
                cx.new(|cx| {
                    UserReactionPanel::new(
                        message_id,
                        panel_emoji_id.clone(),
                        panel_emoji.clone(),
                        avatar_cache.clone(),
                        cx,
                    )
                })
                .into()
            });
    }
    if reacted {
        pill = pill
            .bg(gpui::Rgba {
                a: 0.18,
                ..theme.brand
            })
            .border_1()
            .border_color(theme.brand);
    } else {
        pill = pill.bg(theme.bg_tertiary);
    }

    let glyph = reaction.emoji.clone();
    let emoji_el = if src.is_empty() {
        div()
            .absolute()
            .left(px(5.))
            .child(glyph)
            .into_any_element()
    } else {
        div()
            .absolute()
            .left(px(5.))
            .top(px(4.))
            .image_cache(ctx.icon_cache.clone())
            .child(
                img(src)
                    .size(px(REACTION_EMOJI_PX))
                    .object_fit(ObjectFit::ScaleDown)
                    .with_fallback(emoji_error_fallback(
                        px(REACTION_EMOJI_PX),
                        theme.text_muted,
                    )),
            )
            .into_any_element()
    };

    pill.child(emoji_el).child(count_label).into_any_element()
}

pub fn render_hover_actions(
    msg: &Message,
    combined: bool,
    has_reply: bool,
    is_different_day: bool,
    ctx: &RowCtx,
) -> AnyElement {
    if ctx.suppress_hover
        || ctx.context_menu_message == Some(msg.id)
        || ctx.hovered_row != Some(msg.id)
    {
        return div().into_any_element();
    }
    let theme = ctx.theme;
    let bg_hover = theme.bg_hover;
    let action = move |id: &'static str, icon: IconName, size: f32| {
        let mut svg_icon = Icon::new(icon)
            .size(px(size))
            .text_color(theme.text_secondary);
        if matches!(icon, IconName::Reply) {
            // Mirrors React's `rotate-180` on the toolbar's reply icon (the same
            // asset is used un-rotated for the "···" menu's reply item).
            svg_icon =
                svg_icon.with_transformation(Transformation::rotate(radians(std::f32::consts::PI)));
        }
        div()
            .id(id)
            .p_1()
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .child(svg_icon)
    };

    let (top, margin_top) = if is_different_day {
        (-8., 4.)
    } else if combined || has_reply {
        (-8., 0.)
    } else {
        (16., 0.)
    };

    let reply_id = msg.id;
    let react_id = msg.id;
    let react_host = ctx.video_host.clone();

    let is_topic_msg = msg.code == MessageCode::Topic;
    let is_poll_msg = msg.code == MessageCode::Poll;
    let sender_is_real = !msg.sender_id.is_empty() && msg.sender_id != "0";
    let is_own_message = ctx.current_user_id == msg.sender_id.as_str();

    let show_topic = !ctx.is_topic_box
        && ctx.clan_id.is_some_and(|c| !c.is_zero())
        && !is_topic_msg
        && !is_poll_msg
        && TopicsStore::can_create_topic(ctx.app)
        && TopicsStore::message_allows_topic_discussion(msg);
    let show_edit = is_own_message
        && msg.code != MessageCode::SendToken
        && msg.code.is_user_timeline()
        && !is_poll_msg
        && !msg.is_forwarded;
    let show_thread = !ctx.is_topic_box
        && ctx.channel_top_level
        && ctx.is_clan_owner
        && !is_poll_msg
        && ctx.channel_type != Some(ChannelType::Stream)
        && ctx.channel_type != Some(ChannelType::App);
    let show_coffee = !is_own_message && sender_is_real;

    let msg_id = msg.id;
    let edit_host = ctx.video_host.clone();
    let option_host = ctx.video_host.clone();

    let recent_emoji = (!ctx.emoji_recent.is_empty()).then(|| {
        let mut row = div().flex().flex_row().items_center().gap_0p5();
        for emoji in ctx.emoji_recent {
            let emoji_id = emoji.id.clone();
            let shortname = emoji.shortname.clone();
            let src = crate::util::imgproxy::emoji_url(ctx.app, &emoji.id);
            let cell_id =
                SharedString::from(format!("recent-emoji-{}-{}", msg.row_anchor_id.0, emoji.id));
            let mut cell = div()
                .id(cell_id)
                .flex()
                .items_center()
                .justify_center()
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(bg_hover))
                .on_click(move |_, _, cx| {
                    MessagesStore::global(cx).update(cx, |store, cx| {
                        store.add_reaction(msg_id, emoji_id.clone(), shortname.clone(), cx);
                    });
                });
            if !src.is_empty() {
                cell = cell.child(
                    img(src)
                        .size(px(20.))
                        .object_fit(ObjectFit::ScaleDown)
                        .with_fallback(emoji_error_fallback(px(20.), theme.text_secondary)),
                );
            }
            row = row.child(cell);
        }
        row.child(
            div()
                .w(px(1.))
                .h(px(20.))
                .mx_1()
                .bg(theme.border)
                .opacity(0.5),
        )
    });

    div()
        .id(SharedString::from(format!(
            "hover-actions-{}",
            msg.row_anchor_id.0
        )))
        .absolute()
        .right(px(24.))
        .top(px(top))
        .mt(px(margin_top))
        .flex()
        .flex_row()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .bg(theme.tokens.bg_theme_contexify)
        .children(recent_emoji)
        .when(show_topic, |d| {
            let message_id = msg.id;
            d.child(
                action("topic", IconName::TopicIcon, 24.).on_click(move |_, _, cx| {
                    TopicsStore::global(cx).update(cx, |store, cx| {
                        store.start_create_for_message(message_id, cx);
                    });
                }),
            )
        })
        .child(
            action("react", IconName::Smile, 20.).on_click(move |_, window, cx| {
                let position = window.mouse_position();
                let _ = react_host.update(cx, |this, cx| {
                    this.open_reaction_picker(react_id, position, window, cx);
                });
            }),
        )
        .when(!is_own_message, |d| {
            let is_topic = ctx.is_topic_box;
            d.child(
                action("reply", IconName::Reply, 20.).on_click(move |_, _, cx| {
                    if is_topic {
                        TopicsStore::global(cx)
                            .update(cx, |store, cx| store.set_reply_to(reply_id, cx));
                    } else {
                        MessagesStore::global(cx)
                            .update(cx, |store, cx| store.set_reply_to(reply_id, cx));
                    }
                }),
            )
        })
        .when(show_edit, |d| {
            d.child(
                action("edit", IconName::PenEdit, 20.).on_click(move |_, window, cx| {
                    let _ = edit_host.update(cx, |this, cx| {
                        this.begin_edit(msg_id, window, cx);
                    });
                }),
            )
        })
        .when(show_thread, |d| {
            d.child(
                action("thread", IconName::ThreadIcon, 24.).on_click(move |_, _, cx| {
                    ThreadsStore::global(cx).update(cx, |store, cx| store.start_create(cx));
                }),
            )
        })
        .when(show_coffee, |d| {
            let message_id = msg.id;
            d.child(
                action("give-coffee", IconName::DollarIconRightClick, 20.).on_click(
                    move |_, _, cx| {
                        MessagesStore::global(cx).update(cx, |store, cx| {
                            store.give_coffee_reaction(message_id, cx);
                        });
                    },
                ),
            )
        })
        .child(
            action("option", IconName::ThreeDot, 20.).on_click(move |_, window, cx| {
                let position = window.mouse_position();
                let _ = option_host.update(cx, |this, cx| {
                    this.open_context_menu(msg_id, position, cx);
                });
            }),
        )
        .into_any_element()
}

fn open_viewer_from_message(
    settings: &Entity<mezon_store::Settings>,
    url: SharedString,
    anchor_before: u32,
    cx: &mut gpui::App,
) {
    use crate::image_viewer::{OpenViewerRequest, open_image_viewer, resolve_channel_label};
    use crate::router::{Route, Router};
    use mezon_store::ClanId;

    let (clan_id, channel_id) = match Router::global(cx).read(cx).route() {
        Route::Channel {
            clan_id,
            channel_id,
        }
        | Route::Thread {
            clan_id,
            channel_id,
            ..
        }
        | Route::Canvas {
            clan_id,
            channel_id,
            ..
        } => (clan_id, channel_id),
        Route::DirectMessage { direct_id, .. } => (ClanId(0), direct_id),
        _ => return,
    };

    open_image_viewer(
        OpenViewerRequest {
            clan_id,
            channel_id,
            channel_label: resolve_channel_label(clan_id, channel_id, SharedString::default(), cx),
            settings: settings.clone(),
            attachments: Vec::new(),
            selected_index: 0,
            selected_url: Some(url),
            anchor_before: Some(anchor_before),
        },
        cx,
    );
}

pub fn render_date_divider(theme: &Theme, label: &str) -> AnyElement {
    let line_color = theme.tokens.border_color_primary;
    div()
        .id(super::content::hashed_element_id("date-sep", label))
        .w_full()
        .mt_5()
        .mb_2()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .relative()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .flex()
                        .items_center()
                        .child(div().w_full().h(px(1.)).bg(line_color)),
                )
                .child(
                    div()
                        .relative()
                        .px_4()
                        .rounded_lg()
                        .bg(theme.tokens.bg_primary)
                        .text_xs()
                        .line_height(rems(1.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(label.to_string()),
                ),
        )
        .into_any_element()
}
