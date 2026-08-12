use gpui::{
    AnyElement, App, Context, Entity, FontWeight, ListHorizontalSizingBehavior, ScrollHandle,
    SharedString, Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, size,
    uniform_list,
};
use mezon_store::{
    BadgeService, ClanId, ClanMembersStore, ClanSettingsPermissions, PermissionEvent,
    PermissionStore, Settings, Sticker, StickerEvent, StickerStore, UserId,
};

use super::emoji_sticker_picker::{
    EmojiStickerPicker, EmojiStickerPickerEvent, EmoticonEditTarget, EmoticonKind,
};
use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Sizable, Size, h_flex, v_flex,
};
use crate::image_cache::{AVATAR_ENTRY_MAX_BYTES, LruImageCache};
use crate::theme::{ActiveTheme, Theme};

const MAX_STICKER_SLOTS: usize = 250;
const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 150.0;
const STICKER_IMAGE_SIZE: f32 = 72.0;
const STICKER_THUMB_PROXY_PX: u32 = 144;
const STICKER_CONTENT_MAX_WIDTH: f32 = 740.0;
const STICKER_GRID_GAP_X: f32 = 16.0;
const STICKER_GRID_GAP_Y: f32 = 16.0;
const STICKER_GRID_MIN_COLUMNS: u16 = 3;
const STICKER_GRID_MAX_COLUMNS: u16 = 5;
const STICKER_GRID_OVERFLOW_INSET: f32 = 8.0;
const STICKER_ROW_HEIGHT: f32 = CARD_HEIGHT + STICKER_GRID_GAP_Y;
const STICKER_LIST_CACHE_CAPACITY: usize = 512;
const STICKER_LIST_CACHE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone)]
struct StickerCardData {
    id: SharedString,
    shortname: SharedString,
    src: SharedString,
    creator_name: SharedString,
    creator_avatar: Option<SharedString>,
    can_manage: bool,
    is_for_sale: bool,
}

impl StickerCardData {
    fn same_as(&self, other: &Self) -> bool {
        self.id == other.id
            && self.shortname == other.shortname
            && self.src == other.src
            && self.creator_name == other.creator_name
            && self.creator_avatar == other.creator_avatar
            && self.can_manage == other.can_manage
            && self.is_for_sale == other.is_for_sale
    }
}

enum StickerGridCell {
    Sticker(StickerCardData),
    Add,
}

fn sticker_grid_columns() -> usize {
    let mut columns = STICKER_GRID_MIN_COLUMNS;
    for column_count in STICKER_GRID_MIN_COLUMNS..=STICKER_GRID_MAX_COLUMNS {
        let row_width =
            column_count as f32 * CARD_WIDTH + (column_count - 1) as f32 * STICKER_GRID_GAP_X;
        if row_width <= STICKER_CONTENT_MAX_WIDTH {
            columns = column_count;
        } else {
            break;
        }
    }
    usize::from(columns)
}

fn sticker_grid_gap_x() -> f32 {
    let columns = sticker_grid_columns() as f32;
    let gaps = (columns - 1.0).max(1.0);
    ((STICKER_CONTENT_MAX_WIDTH - columns * CARD_WIDTH) / gaps).max(STICKER_GRID_GAP_X)
}

fn section_heading_xs(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    let text = text.into().to_string().to_uppercase();
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text_primary)
        .mb(px(8.0))
        .child(text)
}

fn body_text(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .text_sm()
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.text_secondary)
        .child(text.into())
}

pub struct StickerSettingPage {
    clan_id: ClanId,
    clan_id_str: String,
    settings: Entity<Settings>,
    image_cache: Entity<LruImageCache>,
    scroll: ScrollHandle,
    grid_scroll: UniformListScrollHandle,
    grid_cells: Vec<StickerGridCell>,
    grid_columns: usize,
    last_permissions: Option<ClanSettingsPermissions>,
    _sticker_sub: Subscription,
    _members_sub: Subscription,
    _perm_sub: Subscription,
    _modal_sub: Option<Subscription>,
}

impl StickerSettingPage {
    pub fn new(clan_id: ClanId, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        StickerStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        PermissionStore::global(cx).update(cx, |store, cx| {
            store.load_clan_permissions(clan_id, cx);
        });

        let sticker_sub = cx.subscribe(
            &StickerStore::global(cx),
            |this, _, _: &StickerEvent, cx| {
                if this.rebuild_grid(cx) {
                    cx.notify();
                }
            },
        );
        let members_sub = cx.subscribe(&ClanMembersStore::global(cx), |this, _, event, cx| {
            if event.clan_id() == this.clan_id && this.rebuild_grid(cx) {
                cx.notify();
            }
        });
        let perm_sub = cx.subscribe(&PermissionStore::global(cx), |this, _, event, cx| {
            let PermissionEvent::Changed { clan_id } = event;
            if !clan_id.is_none_or(|id| id == this.clan_id) {
                return;
            }
            let perms = PermissionStore::global(cx)
                .read(cx)
                .clan_settings_permissions(this.clan_id, cx);
            if this.last_permissions == Some(perms) {
                return;
            }
            this.last_permissions = Some(perms);
            if this.rebuild_grid(cx) {
                cx.notify();
            }
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "clan-sticker-settings-thumbs",
                STICKER_LIST_CACHE_CAPACITY,
                STICKER_LIST_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });

        let clan_id_str = clan_id.get().to_string();
        let grid_columns = sticker_grid_columns();
        let mut this = Self {
            clan_id,
            clan_id_str,
            settings,
            image_cache,
            scroll: ScrollHandle::new(),
            grid_scroll: UniformListScrollHandle::new(),
            grid_cells: Vec::new(),
            grid_columns,
            last_permissions: None,
            _sticker_sub: sticker_sub,
            _members_sub: members_sub,
            _perm_sub: perm_sub,
            _modal_sub: None,
        };
        this.rebuild_grid(cx);
        this.last_permissions = Some(
            PermissionStore::global(cx)
                .read(cx)
                .clan_settings_permissions(clan_id, cx),
        );
        this
    }

    pub fn release(&mut self, _cx: &mut Context<Self>) {
        self._modal_sub.take();
    }

    fn sticker_count(&self) -> usize {
        self.grid_cells
            .iter()
            .filter(|cell| matches!(cell, StickerGridCell::Sticker(_)))
            .count()
    }

    fn rebuild_grid(&mut self, cx: &App) -> bool {
        let grid_columns = sticker_grid_columns();
        let stickers = StickerStore::global(cx)
            .read(cx)
            .for_clan(&self.clan_id_str);
        let mut next: Vec<StickerGridCell> = stickers
            .iter()
            .map(|sticker| StickerGridCell::Sticker(self.card_data(sticker, cx)))
            .collect();
        next.push(StickerGridCell::Add);

        if self.grid_columns == grid_columns
            && self.grid_cells.len() == next.len()
            && self
                .grid_cells
                .iter()
                .zip(&next)
                .all(|(left, right)| match (left, right) {
                    (StickerGridCell::Sticker(a), StickerGridCell::Sticker(b)) => a.same_as(b),
                    (StickerGridCell::Add, StickerGridCell::Add) => true,
                    _ => false,
                })
        {
            return false;
        }

        self.grid_columns = grid_columns;
        self.grid_cells = next;
        true
    }

    fn grid_row_count(&self) -> usize {
        self.grid_cells.len().div_ceil(self.grid_columns)
    }

    fn can_manage_sticker(&self, sticker: &Sticker, cx: &App) -> bool {
        if PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(self.clan_id, cx)
            .has_manage_clan
        {
            return true;
        }
        let Some(current) = BadgeService::global(cx).read(cx).current_user_id(cx) else {
            return false;
        };
        sticker
            .creator_id
            .parse::<i64>()
            .ok()
            .is_some_and(|id| id == current.get())
    }

    fn creator_display(&self, sticker: &Sticker, cx: &App) -> (SharedString, Option<SharedString>) {
        let creator_id = sticker.creator_id.parse::<UserId>().ok();
        if let Some(user_id) = creator_id
            && let Some(member) = ClanMembersStore::global(cx)
                .read(cx)
                .member(self.clan_id, user_id)
        {
            let name = member.name().to_string();
            let avatar = member.avatar();
            let avatar_src = if avatar.is_empty() {
                None
            } else {
                Some(SharedString::from(crate::util::imgproxy::avatar_url(
                    cx, avatar,
                )))
            };
            return (SharedString::from(name), avatar_src);
        }
        (SharedString::default(), None)
    }

    fn sticker_image_src(sticker: &Sticker, cx: &App) -> SharedString {
        if sticker.src.is_empty() {
            SharedString::default()
        } else {
            crate::util::imgproxy::proxied(
                cx,
                &sticker.src,
                STICKER_THUMB_PROXY_PX,
                STICKER_THUMB_PROXY_PX,
                "fit",
            )
            .into()
        }
    }

    fn card_data(&self, sticker: &Sticker, cx: &App) -> StickerCardData {
        let (creator_name, creator_avatar) = self.creator_display(sticker, cx);
        StickerCardData {
            id: SharedString::from(sticker.id.clone()),
            shortname: SharedString::from(sticker.shortname.clone()),
            src: Self::sticker_image_src(sticker, cx),
            creator_name,
            creator_avatar,
            can_manage: self.can_manage_sticker(sticker, cx),
            is_for_sale: sticker.is_for_sale,
        }
    }

    fn open_picker(
        &mut self,
        editing: Option<EmoticonEditTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.settings.clone();
        let clan_id = self.clan_id;
        let modal = cx.new(|cx| {
            EmojiStickerPicker::new(
                EmoticonKind::Sticker,
                clan_id,
                editing,
                settings,
                window,
                cx,
            )
        });
        self._modal_sub = Some(
            cx.subscribe(&modal, |this, _, _: &EmojiStickerPickerEvent, cx| {
                this._modal_sub = None;
                cx.notify();
            }),
        );
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
    }

    fn open_create_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(None, window, cx);
    }

    fn confirm_delete_sticker(
        &mut self,
        sticker_id: SharedString,
        shortname: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.settings.read(cx).language.clone();
        let clan_id = self.clan_id;
        Shell::global(cx).update(cx, |shell, cx| {
            shell.confirm_delete_sticker(clan_id, sticker_id, shortname, &locale, window, cx);
        });
    }

    fn render_upload_card(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.theme_setting_nav)
            .items_center()
            .gap_4()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(locale, "clanSettings.stickers.uploadHere")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(
                                locale,
                                "clanSettings.stickers.customizeMessage",
                            )),
                    ),
            )
            .child(
                Button::new("sticker-upload-card")
                    .label(mezon_i18n::t(locale, "clanStickerSetting.btn.upload"))
                    .primary()
                    .with_size(Size::Large)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_create_modal(window, cx);
                    })),
            )
    }
}

fn render_sticker_card(
    sticker: &StickerCardData,
    theme: &Theme,
    entity: Entity<StickerSettingPage>,
) -> impl IntoElement {
    let shortname = sticker.shortname.clone();
    let group_name = SharedString::from(format!("sticker-card-{}", sticker.id));

    let mut card = v_flex()
        .id(group_name.clone())
        .group(group_name.clone())
        .relative()
        .w(px(CARD_WIDTH))
        .h(px(CARD_HEIGHT))
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.tokens.bg_active_member_channel)
        .items_center()
        .justify_between()
        .child(
            div()
                .h(px(STICKER_IMAGE_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(sticker.src.clone())
                        .id(SharedString::from(format!("sticker-thumb-{}", sticker.id)))
                        .h(px(STICKER_IMAGE_SIZE))
                        .max_w(px(STICKER_IMAGE_SIZE))
                        .object_fit(gpui::ObjectFit::Contain),
                ),
        )
        .child(
            div()
                .w_full()
                .max_w(px(90.0))
                .text_center()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(shortname.clone()),
        )
        .child(
            h_flex()
                .w_full()
                .items_end()
                .justify_center()
                .gap_1()
                .child({
                    let mut avatar = Avatar::new()
                        .name(sticker.creator_name.clone())
                        .size_px(px(16.0));
                    if let Some(src) = sticker.creator_avatar.clone() {
                        avatar = avatar.src(src);
                    }
                    div().flex_shrink_0().child(avatar)
                })
                .child(
                    div()
                        .max_w(px(80.0))
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(sticker.creator_name.clone()),
                ),
        );

    if sticker.is_for_sale {
        card = card.child(
            div().absolute().top_1().left_1().child(
                Icon::new(IconName::MarketIcons)
                    .size(px(16.0))
                    .text_color(gpui::rgb(0xfacc15)),
            ),
        );
    }

    if sticker.can_manage {
        let sticker_id_for_delete = sticker.id.clone();
        let shortname_for_delete = shortname.clone();
        card = card.child(
            div()
                .absolute()
                .top(px(-8.0))
                .right(px(-8.0))
                .invisible()
                .group_hover(group_name, |s| s.visible())
                .child(
                    div()
                        .id(SharedString::from(format!("sticker-delete-{}", sticker.id)))
                        .size(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme.surfaces.input_primary)
                        .shadow_sm()
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.confirm_delete_sticker(
                                    sticker_id_for_delete.clone(),
                                    shortname_for_delete.clone(),
                                    window,
                                    cx,
                                );
                            });
                        })
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.0))
                                .text_color(theme.danger_text),
                        ),
                ),
        );
    }

    card
}

fn render_add_card(theme: &Theme, entity: Entity<StickerSettingPage>) -> impl IntoElement {
    div()
        .id("sticker-add-card")
        .group(SharedString::from("sticker-add-card"))
        .w(px(CARD_WIDTH))
        .h(px(CARD_HEIGHT))
        .p_3()
        .rounded_lg()
        .border_1()
        .border_dashed()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            entity.update(cx, |this, cx| this.open_create_modal(window, cx));
        })
        .child(
            div()
                .relative()
                .size(px(36.0))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size(px(30.0))
                        .rounded(px(10.0))
                        .bg(theme.text_secondary)
                        .child(
                            div().absolute().top(px(3.0)).left(px(3.0)).child(
                                Icon::new(IconName::EmojiCatStar)
                                    .size(px(15.0))
                                    .text_color(gpui::white()),
                            ),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .size(px(20.0))
                        .rounded_full()
                        .border(px(3.0))
                        .border_color(theme.tokens.theme_setting_primary)
                        .bg(theme.brand)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Plus)
                                .size(px(11.0))
                                .text_color(gpui::white()),
                        ),
                ),
        )
}

fn render_sticker_grid_row(
    row_ix: usize,
    grid_cells: &[StickerGridCell],
    grid_columns: usize,
    theme: &Theme,
    entity: Entity<StickerSettingPage>,
) -> AnyElement {
    let gap_x = sticker_grid_gap_x();
    let start = row_ix * grid_columns;
    let end = (start + grid_columns).min(grid_cells.len());
    let mut row = h_flex()
        .w_full()
        .h(px(STICKER_ROW_HEIGHT))
        .gap_x(px(gap_x))
        .items_start();

    for cell in &grid_cells[start..end] {
        row = row.child(match cell {
            StickerGridCell::Sticker(sticker) => {
                render_sticker_card(sticker, theme, entity.clone()).into_any_element()
            }
            StickerGridCell::Add => render_add_card(theme, entity.clone()).into_any_element(),
        });
    }

    row.into_any_element()
}

impl StickerSettingPage {
    fn render_grid(
        &self,
        locale: &str,
        theme: &Theme,
        entity: Entity<StickerSettingPage>,
        image_cache: Entity<LruImageCache>,
    ) -> impl IntoElement {
        let sticker_count = self.sticker_count();
        let slots_left = MAX_STICKER_SLOTS.saturating_sub(sticker_count);
        let available = mezon_i18n::t(locale, "clanStickerSetting.content.available")
            .replace("{{left}}", &slots_left.to_string());
        let row_count = self.grid_row_count();
        let list_entity = entity.clone();
        let grid_height = px(row_count as f32 * STICKER_ROW_HEIGHT + STICKER_GRID_OVERFLOW_INSET);

        let grid_list = uniform_list(
            "clan-sticker-settings-grid",
            row_count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                let page = list_entity.read(cx);
                range
                    .map(|row_ix| {
                        render_sticker_grid_row(
                            row_ix,
                            &page.grid_cells,
                            page.grid_columns,
                            &theme,
                            list_entity.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_item_size(size(px(STICKER_CONTENT_MAX_WIDTH), px(STICKER_ROW_HEIGHT)))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
        .pt(px(STICKER_GRID_OVERFLOW_INSET))
        .track_scroll(&self.grid_scroll)
        .size_full();

        div()
            .w_full()
            .max_w(px(STICKER_CONTENT_MAX_WIDTH))
            .child(
                div()
                    .flex_shrink_0()
                    .mb(px(16.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_secondary)
                    .child(available.to_uppercase()),
            )
            .child(
                div()
                    .image_cache(image_cache)
                    .id("clan-sticker-settings-grid-container")
                    .w_full()
                    .h(grid_height)
                    .child(grid_list),
            )
    }
}

impl Render for StickerSettingPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store_count = StickerStore::global(cx)
            .read(cx)
            .for_clan(&self.clan_id_str)
            .len();
        if store_count != self.sticker_count() {
            self.rebuild_grid(cx);
        } else if self.sticker_count() == 0 {
            StickerStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        }

        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let entity = cx.entity();
        let image_cache = self.image_cache.clone();

        let requirements_section = v_flex()
            .flex_shrink_0()
            .w_full()
            .pb(px(24.0))
            .border_b_1()
            .border_color(theme.border)
            .gap_2()
            .child(body_text(
                mezon_i18n::t(&locale, "clanStickerSetting.content.description"),
                &theme,
            ))
            .child(
                section_heading_xs(
                    mezon_i18n::t(&locale, "clanStickerSetting.content.requirements"),
                    &theme,
                )
                .mt(px(8.0)),
            )
            .child(body_text(
                mezon_i18n::t(&locale, "clanStickerSetting.content.reqType"),
                &theme,
            ))
            .child(body_text(
                mezon_i18n::t(&locale, "clanStickerSetting.content.reqDim"),
                &theme,
            ))
            .child(body_text(
                mezon_i18n::t(&locale, "clanStickerSetting.content.reqSize"),
                &theme,
            ));

        div().relative().size_full().min_h_0().child(
            div()
                .id("clan-sticker-settings-scroll")
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(requirements_section)
                .child(
                    div()
                        .w_full()
                        .mt(px(16.0))
                        .child(self.render_upload_card(&locale, &theme, cx)),
                )
                .child(
                    div()
                        .w_full()
                        .mt(px(16.0))
                        .pb(px(60.0))
                        .child(self.render_grid(&locale, &theme, entity, image_cache)),
                ),
        )
    }
}
