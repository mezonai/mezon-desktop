use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Icon, IconName, h_flex, v_flex};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;
use crate::util::qr_image::{QrImage, QrImageOptions, build_qr_image};
use gpui::{
    App, ClipboardItem, Context, Entity, FocusHandle, FontWeight, ObjectFit, Rgba, SharedString,
    Task, Window, div, img, prelude::*, px,
};
use mezon_store::UserAccount;
use std::time::Duration;

const QR_RENDER_SIZE: usize = 700;
const COPIED_DURATION: Duration = Duration::from_secs(2);
const MEZON_LOGO: &str = "icons/profile-qr-brand.svg";
const PROFILE_LINK_ICON: &str = "icons/profile-qr-link.svg";

enum QrCenterLogo {
    Avatar,
    Mezon,
    Custom(std::sync::Arc<gpui::Image>),
}

pub(super) struct QrProfileModal {
    account: UserAccount,
    link: String,
    center_logo: QrCenterLogo,
    downloading: bool,
    menu_open: bool,
    copied_link: bool,
    avatar_cache: Entity<LruImageCache>,
    focus_handle: FocusHandle,
    locale: String,
    qr_image: Option<QrImage>,
    loading: bool,
    copied: bool,
    copy_reset_task: Option<Task<()>>,
    card_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    exporting: bool,
    export_task: Option<Task<()>>,
}

impl QrProfileModal {
    pub(super) fn open(
        data: String,
        account: UserAccount,
        locale: String,
        window: &mut Window,
        cx: &mut App,
    ) {
        let modal = cx.new(|cx| Self::new(account, data.clone(), locale, cx));
        let focus_handle = modal.read(cx).focus_handle.clone();
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.clone().into(), cx));
        window.focus(&focus_handle, cx);

        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let qr_image = executor
                .spawn(async move { Self::build_profile_qr(&data) })
                .await;
            modal.update(cx, |this, cx| {
                this.qr_image = qr_image;
                this.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn new(account: UserAccount, link: String, locale: String, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| {
            if let Some(qr) = this.qr_image.take() {
                crate::image_cache::queue_atlas_drop(cx, qr.render);
            }
        })
        .detach();
        Self {
            account,
            link,
            center_logo: QrCenterLogo::Avatar,
            downloading: false,
            menu_open: false,
            copied_link: false,
            avatar_cache: crate::image_cache::shared_avatar_cache(cx),
            focus_handle: cx.focus_handle(),
            locale,
            qr_image: None,
            loading: true,
            copied: false,
            copy_reset_task: None,
            card_bounds: None,
            exporting: false,
            export_task: None,
        }
    }

    fn reset_copy_after_delay(&mut self, cx: &mut Context<Self>) {
        if !self.copied || self.copy_reset_task.is_some() {
            return;
        }
        self.copy_reset_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                this.copy_reset_task = None;
                cx.notify();
            });
        }));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
    fn tr(&self, key: &'static str) -> &'static str {
        mezon_i18n::t(&self.locale, key)
    }

    fn avatar(&self, size: f32, cx: &App) -> Avatar {
        let mut avatar = Avatar::new()
            .size_px(px(size))
            .image_cache(self.avatar_cache.clone());
        if let Some(url) = self
            .account
            .avatar_url
            .as_ref()
            .filter(|url| !url.is_empty())
        {
            avatar = avatar
                .src(crate::util::imgproxy::profile_url(cx, url))
                .fallback_src(SharedString::from(url.clone()));
        }
        avatar
    }

    fn action_button(
        id: &'static str,
        icon: &'static str,
        label: &'static str,
        icon_color: Rgba,
        theme: &crate::theme::Theme,
        copied: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let text_color = if copied {
            theme.status_online
        } else {
            theme.tokens.text_theme_primary
        };
        v_flex()
            .id(id)
            .flex_1()
            .h(px(68.))
            .gap(px(6.))
            .items_center()
            .justify_center()
            .rounded(px(16.))
            .border_1()
            .border_color(theme.tokens.border_primary)
            .cursor_pointer()
            .text_color(text_color)
            .when(copied, |el| {
                el.bg(gpui::Hsla::from(theme.status_online).opacity(0.2))
                    .border_color(gpui::Hsla::from(theme.status_online).opacity(0.4))
            })
            .when(!copied, |el| {
                el.hover({
                    let hover = theme.tokens.bg_item_theme_hover;
                    move |style| style.bg(hover)
                })
            })
            .child(gpui::svg().path(icon).size(px(20.)).text_color(if copied {
                text_color
            } else {
                icon_color
            }))
            .child(div().text_size(px(11.)).child(label))
    }

    fn logo_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let foreground = theme.tokens.text_theme_primary;
        let hover = theme.tokens.bg_item_hover;
        let row = |id: &'static str, label: &'static str, icon: gpui::AnyElement| {
            h_flex()
                .id(id)
                .h(px(32.))
                .w_full()
                .gap(px(10.))
                .px_3()
                .py_2()
                .rounded_lg()
                .items_center()
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(foreground)
                .hover(move |style| style.bg(hover))
                .child(icon)
                .child(div().flex_1().min_w_0().child(label))
        };
        v_flex()
            .absolute()
            .top(gpui::relative(1.))
            .right_0()
            .mt(px(8.))
            .w(px(224.))
            .p(px(6.))
            .gap(px(4.))
            .rounded(px(12.))
            .bg(theme.tokens.bg_theme_contexify)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .occlude()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .shadow_lg()
            .child(
                div()
                    .h(px(24.))
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(foreground)
                    .child(self.tr("setting.account.qrProfile.centerLogo")),
            )
            .child(
                row(
                    "choose-qr-logo",
                    self.tr("setting.account.qrProfile.chooseFromDevice"),
                    Icon::new(IconName::UploadImage)
                        .size(px(16.))
                        .text_color(theme.brand)
                        .into_any_element(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.choose_logo(cx))),
            )
            .child(
                row(
                    "default-qr-logo",
                    self.tr("setting.account.qrProfile.resetDefaultLogo"),
                    img(MEZON_LOGO).size(px(16.)).into_any_element(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.center_logo = QrCenterLogo::Mezon;
                    this.menu_open = false;
                    cx.notify();
                })),
            )
    }

    fn center_avatar(&self, cx: &App) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let avatar_background = theme.brand;
        let name = if self.account.username.is_empty() {
            &self.account.display_name
        } else {
            &self.account.username
        };
        let initial = name
            .chars()
            .next()
            .unwrap_or('U')
            .to_uppercase()
            .to_string();
        let fallback = move || {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(avatar_background)
                .text_color(gpui::white())
                .text_size(px(15.))
                .child(initial.clone())
                .into_any_element()
        };
        div()
            .size_full()
            .rounded(px(8.))
            .overflow_hidden()
            .image_cache(self.avatar_cache.clone())
            .child(
                if let Some(url) = self
                    .account
                    .avatar_url
                    .as_ref()
                    .filter(|url| !url.is_empty())
                {
                    img(crate::util::imgproxy::profile_url(cx, url))
                        .size_full()
                        .rounded(px(8.))
                        .object_fit(ObjectFit::Cover)
                        .with_fallback(fallback)
                        .into_any_element()
                } else {
                    fallback()
                },
            )
            .into_any_element()
    }

    fn build_profile_qr(data: &str) -> Option<QrImage> {
        build_qr_image(
            data,
            QrImageOptions {
                target_size: QR_RENDER_SIZE,
                min_module_scale: 2,
                error_correction: qrcode::EcLevel::H,
                clipboard_border: 0,
            },
        )
    }

    fn export(&mut self, download: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.exporting || self.downloading || (!download && self.copied) {
            return;
        }
        self.menu_open = false;
        self.exporting = true;
        self.downloading = download;
        let entity = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = entity.update(cx, |this, cx| {
                let capture = this
                    .card_bounds
                    .ok_or_else(|| anyhow::anyhow!("Profile card has not been laid out"))
                    .and_then(|bounds| crate::app::capture::capture_region(window, bounds));
                match capture {
                    Ok(capture) => {
                        this.export_task = Some(cx.spawn(async move |this, cx| {
                            let result = cx
                                .background_executor()
                                .spawn(async move { capture() })
                                .await;
                            let _ = this
                                .update(cx, |this, cx| this.finish_export(result, download, cx));
                        }));
                    }
                    Err(error) => this.finish_export(Err(error), download, cx),
                }
            });
        });
        cx.notify();
    }

    fn finish_export(
        &mut self,
        result: anyhow::Result<gpui::Image>,
        download: bool,
        cx: &mut Context<Self>,
    ) {
        self.export_task = None;
        self.exporting = false;
        let image = match result {
            Ok(image) => image,
            Err(error) => {
                self.downloading = false;
                tracing::warn!("Cannot export profile QR: {error}");
                let message = self.tr("setting.account.qrProfile.errorExport");
                Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                cx.notify();
                return;
            }
        };
        if !download {
            cx.write_to_clipboard(ClipboardItem::new_image(&image));
            self.copied = true;
            self.reset_copy_after_delay(cx);
            let message = self.tr("invitation.messages.qrCopiedSuccess");
            Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
            cx.notify();
            return;
        }
        self.downloading = true;
        let picker = cx.prompt_for_new_path(std::path::Path::new(""), Some("mezon-profile-qr.png"));
        cx.spawn(async move |this, cx| {
            let Some(path) = crate::util::file_dialog::resolve(picker, cx).await else {
                let _ = this.update(cx, |this, cx| {
                    this.downloading = false;
                    cx.notify();
                });
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { std::fs::write(path, &image.bytes) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.downloading = false;
                if result.is_ok() {
                    let message = this.tr("setting.account.qrProfile.qrDownloaded");
                    Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
                } else {
                    let message = this.tr("setting.account.qrProfile.errorExport");
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_logo(&mut self, cx: &mut Context<Self>) {
        self.menu_open = false;
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("setting.account.qrProfile.chooseFromDevice").into()),
        });
        cx.spawn(async move |this, cx| {
            let Some(paths) = crate::util::file_dialog::resolve(picker, cx).await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let logo = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(path).ok()?;
                    let image = image::load_from_memory(&bytes).ok()?.thumbnail(256, 256);
                    let mut bytes = std::io::Cursor::new(Vec::new());
                    image.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
                    Some(std::sync::Arc::new(gpui::Image::from_bytes(
                        gpui::ImageFormat::Png,
                        bytes.into_inner(),
                    )))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(logo) = logo {
                    this.center_logo = QrCenterLogo::Custom(logo);
                } else {
                    let message = this.tr("setting.account.qrProfile.errorImage");
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for QrProfileModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let background = theme.surfaces.primary.ramp();
        let foreground = theme.tokens.text_theme_primary;
        let secondary = theme.tokens.theme_setting_nav;
        let border = theme.tokens.border_primary;
        let paper = crate::theme::Theme::light();
        let name = if self.account.display_name.is_empty() {
            &self.account.username
        } else {
            &self.account.display_name
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _, cx| {
                if this.menu_open {
                    this.menu_open = false;
                    cx.notify();
                } else {
                    Self::close(cx);
                }
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.menu_open {
                        this.menu_open = false;
                        cx.notify();
                    }
                }),
            )
            .relative()
            .w(px(380.))
            .rounded(px(16.))
            .bg(background)
            .text_color(foreground)
            .border_1()
            .border_color(border)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .px(px(20.))
                    .pt(px(20.))
                    .pb(px(4.))
                    .child(
                        div()
                            .id("close-profile-qr")
                            .size(px(36.))
                            .rounded_full()
                            .border_1()
                            .border_color(border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover({
                                let hover = theme.tokens.bg_item_theme_hover;
                                move |style| style.bg(hover)
                            })
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(foreground),
                            )
                            .on_click(|_, _, cx| Self::close(cx)),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .id("customize-profile-qr")
                                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .size(px(36.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(border)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .when(self.menu_open, |el| el.bg(theme.brand))
                                    .when(!self.menu_open, |el| {
                                        el.hover({
                                            let hover = theme.tokens.bg_item_theme_hover;
                                            move |style| style.bg(hover)
                                        })
                                    })
                                    .child(Icon::new(IconName::PenEdit).size(px(16.)).text_color(
                                        if self.menu_open {
                                            gpui::white()
                                        } else {
                                            foreground.into()
                                        },
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.menu_open = !this.menu_open;
                                        cx.notify();
                                    })),
                            )
                            .when(self.menu_open, |el| {
                                el.child(gpui::deferred(self.logo_menu(cx)))
                            }),
                    ),
            )
            .child(
                v_flex()
                    .relative()
                    .debug_selector(|| "profile-qr-card".into())
                    .child(
                        gpui::canvas(
                            {
                                let entity = cx.entity().downgrade();
                                move |bounds, _, cx| {
                                    let _ = entity
                                        .update(cx, |this, _| this.card_bounds = Some(bounds));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .w_full()
                    .items_center()
                    .pt(px(4.))
                    .pb(px(14.))
                    .child(
                        h_flex()
                            .max_w(px(332.))
                            .gap(px(10.))
                            .items_center()
                            .child(self.avatar(32., cx).border_color(theme.brand))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .truncate()
                                    .child(name.to_uppercase()),
                            ),
                    )
                    .when(!self.account.username.is_empty(), |el| {
                        el.child(
                            div()
                                .mt(px(6.))
                                .px(px(10.))
                                .py(px(2.))
                                .rounded_full()
                                .border_1()
                                .border_color(border)
                                .bg(secondary)
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.brand)
                                .child(format!("@{}", self.account.username)),
                        )
                    })
                    .child(
                        v_flex()
                            .mt(px(20.))
                            .px(px(35.))
                            .py(px(10.))
                            .rounded(px(16.))
                            .bg(gpui::white())
                            .shadow_lg()
                            .items_center()
                            .child(
                                h_flex()
                                    .w(px(175.))
                                    .justify_between()
                                    .items_center()
                                    .pb(px(8.))
                                    .mb(px(8.))
                                    .border_b_1()
                                    .border_color(paper.border)
                                    .child(
                                        h_flex()
                                            .gap(px(6.))
                                            .items_center()
                                            .child(img(MEZON_LOGO).size(px(16.)))
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(paper.tokens.text_theme_primary)
                                                    .child(
                                                        self.tr(
                                                            "setting.account.qrProfile.brandName",
                                                        ),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .px(px(6.))
                                            .py(px(2.))
                                            .rounded_full()
                                            .bg(paper.tokens.bg_item_hover)
                                            .text_color(theme.brand)
                                            .text_size(px(9.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(self.tr("profile.qr_profile").to_uppercase()),
                                    ),
                            )
                            .child(
                                div()
                                    .relative()
                                    .size(px(175.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(match &self.qr_image {
                                        Some(image) => img(image.render.clone())
                                            .size_full()
                                            .object_fit(ObjectFit::Contain)
                                            .into_any_element(),
                                        None => div()
                                            .text_size(px(12.))
                                            .text_color(paper.tokens.text_theme_primary)
                                            .child(mezon_i18n::t(
                                                &self.locale,
                                                if self.loading {
                                                    "inviteToChannel.qrModal.generating"
                                                } else {
                                                    "inviteToChannel.qrModal.errorGenerating"
                                                },
                                            ))
                                            .into_any_element(),
                                    })
                                    .when(self.qr_image.is_some(), |el| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .size(px(36.))
                                                .flex_none()
                                                .border(px(2.))
                                                .border_color(gpui::white())
                                                .p(px(2.))
                                                .rounded(px(12.))
                                                .shadow_md()
                                                .bg(gpui::white())
                                                .overflow_hidden()
                                                .child(match &self.center_logo {
                                                    QrCenterLogo::Mezon => img(MEZON_LOGO)
                                                        .size_full()
                                                        .rounded(px(8.))
                                                        .object_fit(ObjectFit::Contain)
                                                        .into_any_element(),
                                                    QrCenterLogo::Custom(image) => {
                                                        img(image.clone())
                                                            .size_full()
                                                            .rounded(px(8.))
                                                            .object_fit(ObjectFit::Cover)
                                                            .into_any_element()
                                                    }
                                                    QrCenterLogo::Avatar => self.center_avatar(cx),
                                                }),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_center()
                                    .gap(px(6.))
                                    .pt(px(8.))
                                    .mt(px(8.))
                                    .border_t_1()
                                    .border_color(paper.border)
                                    .child(
                                        Icon::new(IconName::Tick)
                                            .size(px(14.))
                                            .text_color(theme.brand),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.brand)
                                            .child(
                                                self.tr(
                                                    "setting.account.qrProfile.verifiedByMezon",
                                                ),
                                            ),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .debug_selector(|| "profile-qr-actions".into())
                    .w_full()
                    .px(px(24.))
                    .pb(px(20.))
                    .gap(px(12.))
                    .child(
                        Self::action_button(
                            "copy-profile-qr",
                            if self.loading || (self.exporting && !self.downloading) {
                                IconName::LoadingSpinner.path()
                            } else if self.copied {
                                IconName::Tick.path()
                            } else {
                                IconName::CopyIcon.path()
                            },
                            if self.copied {
                                self.tr("invitation.buttons.copied")
                            } else {
                                self.tr("contextMenu.copyImage")
                            },
                            theme.tokens.text_secondary,
                            &theme,
                            self.copied,
                        )
                        .when(self.loading, |el| el.opacity(0.5).cursor_default())
                        .when(self.exporting || self.downloading, |el| el.cursor_default())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.export(false, window, cx)),
                        ),
                    )
                    .child(
                        Self::action_button(
                            "download-profile-qr",
                            if self.downloading || self.loading {
                                IconName::LoadingSpinner.path()
                            } else {
                                IconName::Download.path()
                            },
                            self.tr("common.download"),
                            theme.text_link,
                            &theme,
                            false,
                        )
                        .when(self.loading, |el| el.opacity(0.5).cursor_default())
                        .when(self.exporting || self.downloading, |el| el.cursor_default())
                        .on_click(cx.listener(|this, _, window, cx| this.export(true, window, cx))),
                    )
                    .child(
                        Self::action_button(
                            "copy-profile-link",
                            if self.copied_link {
                                IconName::Tick.path()
                            } else {
                                PROFILE_LINK_ICON
                            },
                            if self.copied_link {
                                self.tr("inviteToChannel.copyLink")
                            } else {
                                self.tr("contextMenu.copyLink")
                            },
                            theme.brand,
                            &theme,
                            self.copied_link,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.copied_link {
                                return;
                            }
                            cx.write_to_clipboard(ClipboardItem::new_string(this.link.clone()));
                            this.copied_link = true;
                            let message = this.tr("message.actions.copyLinkSuccess");
                            Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(COPIED_DURATION).await;
                                let _ = this.update(cx, |this, cx| {
                                    this.copied_link = false;
                                    cx.notify();
                                });
                            })
                            .detach();
                            cx.notify();
                        })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> UserAccount {
        UserAccount {
            user_id: 1,
            username: "ozu".into(),
            display_name: "duoc.phungvan".into(),
            email: None,
            avatar_url: None,
            phone_number: None,
            about_me: None,
            password_setted: false,
            logo: None,
            status: String::new(),
            user_status: String::new(),
            dob_seconds: 0,
            create_time_seconds: 0,
        }
    }

    #[gpui::test]
    fn logo_menu_handles_toggle_hover_reset_and_outside_click(cx: &mut gpui::TestAppContext) {
        for theme in ["dark", "light", "purple_haze"] {
            cx.update(|cx| crate::theme::set_theme(crate::theme::resolve_theme(theme), cx));
            let (view, window) = cx.add_window_view(|_, cx| {
                QrProfileModal::new(
                    account(),
                    "https://mezon.ai/chat/ozu".into(),
                    "en".into(),
                    cx,
                )
            });
            window.run_until_parked();
            let edit = gpui::point(px(380. - 20. - 36. / 2.), px(20. + 36. / 2.));
            window.simulate_click(edit, gpui::Modifiers::default());
            assert!(
                window.read(|cx| view.read(cx).menu_open),
                "edit opens the menu in {theme}"
            );
            window.simulate_click(edit, gpui::Modifiers::default());
            assert!(
                !window.read(|cx| view.read(cx).menu_open),
                "edit also dismisses the menu in {theme}"
            );
            window.simulate_click(edit, gpui::Modifiers::default());
            let reset = gpui::point(
                px(380. - 20. - 224. / 2.),
                px(20. + 36. + 8. + 6. + 24. + 4. + 32. + 4. + 32. / 2.),
            );
            window.simulate_mouse_move(reset, None, gpui::Modifiers::default());
            assert!(
                window.read(|cx| view.read(cx).menu_open),
                "hover must keep the menu open"
            );
            window.simulate_click(reset, gpui::Modifiers::default());
            assert!(
                window.read(|cx| matches!(view.read(cx).center_logo, QrCenterLogo::Mezon)),
                "the second row resets the logo"
            );
            assert!(!window.read(|cx| view.read(cx).menu_open));
            window.simulate_click(edit, gpui::Modifiers::default());
            let choose = gpui::point(reset.x, reset.y - px(32. + 4.));
            window.simulate_click(choose, gpui::Modifiers::default());
            assert!(
                window.did_prompt_for_paths(),
                "the first row opens the image picker"
            );
            window.simulate_path_prompt_response(|_| None);
            window.run_until_parked();
            assert!(!window.read(|cx| view.read(cx).menu_open));
            assert!(
                window.read(|cx| matches!(view.read(cx).center_logo, QrCenterLogo::Mezon)),
                "cancelling must preserve the current logo"
            );
            window.simulate_click(edit, gpui::Modifiers::default());
            window.simulate_click(
                gpui::point(px(20.), px(20. + 36.)),
                gpui::Modifiers::default(),
            );
            assert!(
                !window.read(|cx| view.read(cx).menu_open),
                "clicking outside the popup dismisses it"
            );
            assert!(window.read(|cx| matches!(view.read(cx).center_logo, QrCenterLogo::Mezon)));
        }
    }

    #[test]
    fn profile_assets_render_without_external_references() {
        use gpui::AssetSource;
        let assets = crate::util::assets::Assets;
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(crate::util::assets::Assets));
        for path in [
            IconName::Close.path(),
            IconName::PenEdit.path(),
            PROFILE_LINK_ICON,
            MEZON_LOGO,
        ] {
            let bytes = assets
                .load(path)
                .expect("embedded asset")
                .expect("asset bytes");
            let image = renderer.render_single_frame(&bytes, 1.).expect("valid SVG");
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0),
                "{path} must have visible pixels"
            );
        }
    }
    #[test]
    fn copied_buttons_use_success_colors_and_restore_the_default_style() {
        for theme in ["dark", "light", "purple_haze"] {
            let palette = crate::theme::resolve_theme(theme);
            let foreground = palette.tokens.text_theme_primary;
            for id in ["copy-profile-qr", "copy-profile-link"] {
                let mut normal = QrProfileModal::action_button(
                    id,
                    PROFILE_LINK_ICON,
                    "Copy",
                    foreground,
                    &palette,
                    false,
                );
                let mut copied = QrProfileModal::action_button(
                    id,
                    IconName::Tick.path(),
                    "Copied",
                    foreground,
                    &palette,
                    true,
                );
                assert_eq!(
                    copied.style().text.color,
                    Some(palette.status_online.into())
                );
                assert_ne!(copied.style().background, normal.style().background);
                assert_ne!(copied.style().border_color, normal.style().border_color);
                let mut reset = QrProfileModal::action_button(
                    id,
                    PROFILE_LINK_ICON,
                    "Copy",
                    foreground,
                    &palette,
                    false,
                );
                assert_eq!(reset.style().text.color, Some(foreground.into()));
                assert_eq!(reset.style().background, normal.style().background);
                assert_eq!(reset.style().border_color, normal.style().border_color);
            }
        }
    }
    #[gpui::test]
    fn capture_bounds_follow_the_profile_card_layout(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::set_theme(crate::theme::resolve_theme("dark"), cx));
        let (view, window) = cx.add_window_view(|_, cx| {
            QrProfileModal::new(
                account(),
                "https://mezon.ai/chat/ozu".into(),
                "en".into(),
                cx,
            )
        });
        window.run_until_parked();
        let bounds = window
            .debug_bounds("profile-qr-card")
            .expect("card laid out");
        let actions = window
            .debug_bounds("profile-qr-actions")
            .expect("actions laid out");
        assert_eq!(window.read(|cx| view.read(cx).card_bounds), Some(bounds));
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(bounds.bottom() <= actions.top());
    }

    #[gpui::test]
    fn copying_profile_link_writes_clipboard_and_notifies_toast(cx: &mut gpui::TestAppContext) {
        let shell = cx.update(Shell::init);
        let updates = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = updates.clone();
        let _subscription =
            cx.update(|cx| cx.observe(&shell, move |_, _| observed.set(observed.get() + 1)));
        cx.update(|cx| crate::theme::set_theme(crate::theme::resolve_theme("dark"), cx));
        let (view, window) = cx.add_window_view(|_, cx| {
            QrProfileModal::new(
                account(),
                "https://mezon.ai/chat/ozu".into(),
                "vi".into(),
                cx,
            )
        });
        window.run_until_parked();
        let actions = window
            .debug_bounds("profile-qr-actions")
            .expect("actions laid out");
        let button_width = (actions.size.width - px(24. * 2. + 12. * 2.)) / 3.;
        let position = gpui::point(
            actions.right() - px(24.) - button_width / 2.,
            actions.top() + px(68. / 2.),
        );
        window.simulate_click(position, gpui::Modifiers::default());
        assert!(window.read(|cx| view.read(cx).copied_link));
        assert!(
            updates.get() > 0,
            "copy link must also trigger the toast surface"
        );
        assert_eq!(
            window.update(|_, cx| cx.read_from_clipboard().unwrap().text()),
            Some("https://mezon.ai/chat/ozu".into())
        );
    }
    #[gpui::test]
    fn failed_export_preserves_clipboard_and_restores_export_state(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            Shell::init(cx);
            cx.write_to_clipboard(ClipboardItem::new_string("ozu".into()));
            let modal = cx.new(|cx| {
                QrProfileModal::new(
                    account(),
                    "https://mezon.ai/chat/ozu".into(),
                    "en".into(),
                    cx,
                )
            });
            modal.update(cx, |this, cx| {
                this.exporting = true;
                this.downloading = true;
                this.finish_export(
                    Err(anyhow::anyhow!("image rendering unavailable")),
                    false,
                    cx,
                );
                assert!(!this.copied);
                assert!(!this.exporting);
                assert!(!this.downloading);
            });
            assert_eq!(cx.read_from_clipboard().unwrap().text(), Some("ozu".into()));
        });
    }

    #[gpui::test]
    fn copying_image_writes_the_captured_profile_png(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            Shell::init(cx);
            let pixels = image::RgbaImage::from_pixel(64, 96, image::Rgba([255; 4]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            pixels
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            let expected = gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.into_inner());
            let modal = cx.new(|cx| {
                QrProfileModal::new(
                    account(),
                    "https://mezon.ai/chat/ozu".into(),
                    "en".into(),
                    cx,
                )
            });
            modal.update(cx, |this, cx| {
                this.exporting = true;
                this.finish_export(Ok(expected.clone()), false, cx);
                assert!(!this.exporting);
                assert!(this.copied);
                assert!(!this.downloading);
            });
            let clipboard = cx.read_from_clipboard().expect("clipboard");
            let Some(gpui::ClipboardEntry::Image(image)) = clipboard.entries().first() else {
                panic!("expected QR PNG");
            };
            assert_eq!(image.format, expected.format);
            assert_eq!(image.bytes, expected.bytes);
        });
    }
}
