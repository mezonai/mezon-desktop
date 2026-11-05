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
use std::{sync::Arc, time::Duration};

const QR_RENDER_SIZE: usize = 700;
const COPIED_DURATION: Duration = Duration::from_secs(2);
const MEZON_LOGO: &str = "images/icon-logo-mezon.svg";
const PROFILE_LINK_ICON: &str = "icons/profile-qr-link.svg";
const PROFILE_FONT: &str = "IBM Plex Sans";

struct ProfileFonts;
impl gpui::Global for ProfileFonts {}

impl gpui::AssetSource for ProfileFonts {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf" => {
                include_bytes!("../../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf")
            }
            "fonts/lilex/Lilex-Regular.ttf" => {
                include_bytes!("../../../../assets/fonts/lilex/Lilex-Regular.ttf")
            }
            _ => return Ok(None),
        };
        Ok(Some(std::borrow::Cow::Borrowed(bytes)))
    }

    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

struct ProfileCardImage {
    name: String,
    username: String,
    link: String,
    labels: [&'static str; 3],
    theme: Arc<crate::theme::Theme>,
    avatar: Arc<gpui::RenderImage>,
    logo: Option<Arc<gpui::RenderImage>>,
    brand: Arc<gpui::RenderImage>,
    tick: Arc<gpui::RenderImage>,
}

impl ProfileCardImage {
    fn asset(path: &str, renderer: &gpui::SvgRenderer) -> anyhow::Result<Arc<gpui::RenderImage>> {
        use gpui::AssetSource;
        let bytes = crate::util::assets::Assets
            .load(path)?
            .ok_or_else(|| anyhow::anyhow!("Missing profile asset: {path}"))?;
        gpui::Image::from_bytes(gpui::ImageFormat::Svg, bytes.into_owned())
            .to_image_data(renderer.clone())
    }

    fn xml(value: &str) -> String {
        value
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect::<String>()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    fn color(color: Rgba) -> String {
        format!(
            "rgba({},{},{},{})",
            (color.r * 255.).round() as u8,
            (color.g * 255.).round() as u8,
            (color.b * 255.).round() as u8,
            color.a
        )
    }

    fn over(top: Rgba, bottom: Rgba) -> Rgba {
        let a = top.a + bottom.a * (1. - top.a);
        if a <= 0. {
            return Rgba::default();
        }
        Rgba {
            r: (top.r * top.a + bottom.r * bottom.a * (1. - top.a)) / a,
            g: (top.g * top.a + bottom.g * bottom.a * (1. - top.a)) / a,
            b: (top.b * top.a + bottom.b * bottom.a * (1. - top.a)) / a,
            a,
        }
    }

    fn png(pixels: &image::RgbaImage) -> anyhow::Result<Vec<u8>> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut bytes,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Up,
        )
        .write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(bytes)
    }

    fn image_url(image: &gpui::RenderImage, white: bool) -> anyhow::Result<String> {
        use base64::Engine;
        let size = image.size(0);
        let mut bytes = image
            .as_bytes(0)
            .ok_or_else(|| anyhow::anyhow!("Missing image frame"))?
            .to_vec();
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            if white {
                pixel[..3].fill(255);
            }
        }
        let pixels = image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)
            .ok_or_else(|| anyhow::anyhow!("Invalid image frame"))?;
        Ok(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(Self::png(&pixels)?)
        ))
    }

    fn fitted(
        text: &str,
        size: f32,
        max_width: f32,
        system: &gpui::WindowTextSystem,
    ) -> (String, f32) {
        use unicode_segmentation::UnicodeSegmentation;
        let measure = |value: &str| {
            f32::from(
                system
                    .layout_line(
                        value,
                        px(size),
                        &[gpui::TextRun {
                            len: value.len(),
                            font: gpui::font(PROFILE_FONT),
                            ..Default::default()
                        }],
                        None,
                    )
                    .width,
            )
        };
        let width = measure(text);
        if width <= max_width {
            return (Self::xml(text), width.max(1.));
        }
        let parts: Vec<_> = text.graphemes(true).collect();
        let (mut low, mut high) = (0, parts.len());
        while low < high {
            let mid = (low + high).div_ceil(2);
            if measure(&format!("{}…", parts[..mid].concat())) <= max_width {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        let text = format!("{}…", parts[..low].concat());
        (Self::xml(&text), measure(&text).clamp(1., max_width))
    }

    fn svg(&self, system: &gpui::WindowTextSystem) -> anyhow::Result<String> {
        use std::fmt::Write;
        let paper = crate::theme::Theme::light();
        let (name, name_width) = Self::fitted(&self.name.to_uppercase(), 18., 222., system);
        let (username, username_width) =
            Self::fitted(&format!("@{}", self.username), 11., 270., system);
        let (brand, brand_width) = Self::fitted(self.labels[0], 12., 108., system);
        let (badge, badge_width) = Self::fitted(&self.labels[1].to_uppercase(), 8., 78., system);
        let (verified, verified_width) = Self::fitted(self.labels[2], 9., 180., system);
        let name_left = (340. - 42. - name_width) / 2.;
        let username_box = username_width + 22.;
        let badge_box = badge_width + 16.;
        let badge_left = 280.5 - badge_box;
        let verified_box = verified_width + 32.;
        let verified_left = (340. - verified_box) / 2.;
        let (mut x1, mut y1, mut x2, mut y2) = (0., 0., 340., 384.);
        let mut stops = String::new();
        let mut background = Self::color(self.theme.surfaces.primary.solid);
        if let Some(gradient) = self.theme.surfaces.primary.gradient {
            let angle = (gradient.angle % 360. - 90.).to_radians();
            let (dx, dy) = (angle.cos() * 340. / 384., angle.sin());
            let span = if dx.abs() > dy.abs() { 340. } else { 384. };
            x1 = 170. - dx / dx.hypot(dy) * span / 2.;
            y1 = 192. - dy / dx.hypot(dy) * span / 2.;
            x2 = 340. - x1;
            y2 = 384. - y1;
            for stop in gradient.stops {
                let mut color = stop.color;
                if let Some(base) = gradient.base {
                    color = Self::over(color, base);
                }
                if let Some(overlay) = gradient.overlay {
                    color = Self::over(overlay, color);
                }
                write!(
                    &mut stops,
                    r#"<stop offset="{}" stop-color="{}"/>"#,
                    stop.position,
                    Self::color(color)
                )?;
            }
            background = "url(#background)".into();
        }
        let code =
            qrcode::QrCode::with_error_correction_level(self.link.as_bytes(), qrcode::EcLevel::H)?;
        let width = code.width();
        let modules = code.to_colors();
        let mut qr = String::new();
        for y in 0..width {
            let mut x = 0;
            while x < width {
                if modules[y * width + x] != qrcode::Color::Dark {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < width && modules[y * width + x] == qrcode::Color::Dark {
                    x += 1;
                }
                let run = x - start;
                write!(&mut qr, "M{start} {y}h{run}v1h-{run}z")?;
            }
        }
        Ok(format!(
            include_str!("../../assets/templates/profile-qr.svg"),
            font = PROFILE_FONT,
            avatar_x = name_left + 16.,
            name_left = name_left,
            name_x = name_left + 42.,
            name = name,
            name_width = name_width,
            username = username,
            username_width = username_width,
            username_visibility = if self.username.is_empty() {
                "hidden"
            } else {
                "visible"
            },
            username_box = username_box,
            username_left = (340. - username_box) / 2.,
            brand = brand,
            brand_width = brand_width,
            badge = badge,
            badge_width = badge_width,
            badge_box = badge_box,
            badge_left = badge_left,
            badge_x = badge_left + 8.,
            verified = verified,
            verified_width = verified_width,
            verified_box = verified_box,
            verified_left = verified_left,
            verified_x = verified_left + 23.,
            seal_x = verified_left + 10.,
            tick_x = verified_left + 6.,
            accent = Self::color(self.theme.brand),
            ink = Self::color(paper.text_primary),
            divider = Self::color(paper.border),
            badge_fill = Self::color(paper.tokens.bg_item_hover),
            username_fill = Self::color(self.theme.tokens.theme_setting_nav),
            border = Self::color(self.theme.tokens.border_primary),
            background = background,
            x1 = x1,
            y1 = y1,
            x2 = x2,
            y2 = y2,
            stops = stops,
            qr = qr,
            qr_scale = 175. / width as f32,
            avatar_image = Self::image_url(&self.avatar, false)?,
            logo_image = Self::image_url(self.logo.as_ref().unwrap_or(&self.brand), false)?,
            logo_visibility = if self.logo.is_some() {
                "visible"
            } else {
                "hidden"
            },
            initial_visibility = if self.logo.is_none() {
                "visible"
            } else {
                "hidden"
            },
            initial = Self::xml(
                &(if self.username.is_empty() {
                    &self.name
                } else {
                    &self.username
                })
                .chars()
                .next()
                .unwrap_or('U')
                .to_uppercase()
                .to_string()
            ),
            brand_image = Self::image_url(&self.brand, false)?,
            tick_image = Self::image_url(&self.tick, true)?,
        ))
    }

    fn render(&self, system: Arc<gpui::TextSystem>) -> anyhow::Result<gpui::Image> {
        static RENDERER: std::sync::OnceLock<gpui::SvgRenderer> = std::sync::OnceLock::new();
        let renderer = RENDERER.get_or_init(|| gpui::SvgRenderer::new(Arc::new(ProfileFonts)));
        let svg = self.svg(&gpui::WindowTextSystem::new(system))?;
        let image =
            renderer.render_single_frame(svg.as_bytes(), 1. / gpui::SMOOTH_SVG_SCALE_FACTOR)?;
        let size = image.size(0);
        let mut bytes = image
            .as_bytes(0)
            .ok_or_else(|| anyhow::anyhow!("Missing profile frame"))?
            .to_vec();
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let pixels = image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)
            .ok_or_else(|| anyhow::anyhow!("Invalid profile frame"))?;
        Ok(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            Self::png(&pixels)?,
        ))
    }
}

enum QrCenterLogo {
    Avatar,
    Mezon,
    Custom(Arc<gpui::RenderImage>),
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
    logo_task: Option<Task<()>>,
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
            if let QrCenterLogo::Custom(image) = &this.center_logo {
                crate::image_cache::queue_atlas_drop(cx, image.clone());
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
            logo_task: None,
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
                    this.set_center_logo(QrCenterLogo::Mezon, cx);
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

    fn export(&mut self, download: bool, cx: &mut Context<Self>) {
        use gpui::Asset;
        if self.loading || self.exporting || self.downloading || (!download && self.copied) {
            return;
        }
        self.menu_open = false;
        self.exporting = true;
        self.downloading = download;
        if !cx.has_global::<ProfileFonts>() {
            if let Err(error) =
                cx.text_system()
                    .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
                        "../../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"
                    ))])
            {
                self.finish_export(Err(error), download, cx);
                return;
            }
            cx.set_global(ProfileFonts);
        }
        let renderer = cx.svg_renderer();
        let avatar = self
            .account
            .avatar_url
            .as_ref()
            .filter(|url| !url.is_empty())
            .map(|url| {
                let resource =
                    gpui::Resource::Uri(crate::util::imgproxy::profile_url(cx, url).into());
                let cached = self.avatar_cache.read(cx).ready_render_image(&resource);
                let load: futures::future::BoxFuture<'static, _> =
                    Box::pin(crate::image_cache::AvatarImageLoader::load(resource, cx));
                let fallback: futures::future::BoxFuture<'static, _> =
                    Box::pin(crate::image_cache::AvatarImageLoader::load(
                        gpui::Resource::Uri(url.clone().into()),
                        cx,
                    ));
                async move {
                    if let Some(image) = cached {
                        return Some(image);
                    }
                    match load.await {
                        Ok(image) => Some(image),
                        Err(_) => fallback.await.ok(),
                    }
                }
            });
        let custom = match &self.center_logo {
            QrCenterLogo::Custom(image) => Some(image.clone()),
            _ => None,
        };
        let use_avatar = matches!(self.center_logo, QrCenterLogo::Avatar);
        let name = if self.account.display_name.is_empty() {
            self.account.username.clone()
        } else {
            self.account.display_name.clone()
        };
        let username = self.account.username.clone();
        let link = self.link.clone();
        let labels = [
            self.tr("setting.account.qrProfile.brandName"),
            self.tr("profile.qr_profile"),
            self.tr("setting.account.qrProfile.verifiedByMezon"),
        ];
        let theme = cx.theme().clone();
        let system = cx.text_system().clone();
        self.export_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let brand = ProfileCardImage::asset(MEZON_LOGO, &renderer)?;
                    let tick = ProfileCardImage::asset(IconName::Tick.path(), &renderer)?;
                    let default_avatar =
                        ProfileCardImage::asset("icons/avatar-user.svg", &renderer)?;
                    let avatar = match avatar {
                        Some(load) => load.await,
                        None => None,
                    };
                    let logo = if let Some(custom) = custom {
                        Some(custom)
                    } else if use_avatar {
                        avatar.clone()
                    } else {
                        Some(brand.clone())
                    };
                    ProfileCardImage {
                        name,
                        username,
                        link,
                        labels,
                        theme,
                        avatar: avatar.unwrap_or(default_avatar),
                        logo,
                        brand,
                        tick,
                    }
                    .render(system)
                })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_export(result, download, cx));
        }));
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

    fn set_center_logo(&mut self, logo: QrCenterLogo, cx: &mut Context<Self>) {
        if let QrCenterLogo::Custom(image) = std::mem::replace(&mut self.center_logo, logo) {
            crate::image_cache::queue_atlas_drop(cx, image);
        }
        self.logo_task = None;
        self.menu_open = false;
        self.copied = false;
        self.copy_reset_task = None;
        cx.notify();
    }

    fn choose_logo(&mut self, cx: &mut Context<Self>) {
        if self.exporting || self.downloading {
            return;
        }
        self.menu_open = false;
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("setting.account.qrProfile.chooseFromDevice").into()),
        });
        self.logo_task = Some(cx.spawn(async move |this, cx| {
            let path = crate::util::file_dialog::resolve(picker, cx)
                .await
                .and_then(|paths| paths.into_iter().next());
            let Some(path) = path else {
                let _ = this.update(cx, |this, _| this.logo_task = None);
                return;
            };
            let logo = cx
                .background_executor()
                .spawn(async move {
                    let mut pixels = image::open(path).ok()?.thumbnail(256, 256).to_rgba8();
                    for pixel in pixels.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    Some(Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                        pixels,
                    )])))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(logo) = logo {
                    this.set_center_logo(QrCenterLogo::Custom(logo), cx);
                } else {
                    this.logo_task = None;
                    let message = this.tr("setting.account.qrProfile.errorImage");
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                    cx.notify();
                }
            });
        }));
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
                                        if !this.exporting && !this.downloading {
                                            this.menu_open = !this.menu_open;
                                            cx.notify();
                                        }
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
                        .on_click(cx.listener(|this, _, _, cx| this.export(false, cx))),
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
                        .on_click(cx.listener(|this, _, _, cx| this.export(true, cx))),
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

    fn sample_card() -> ProfileCardImage {
        let renderer = gpui::SvgRenderer::new(Arc::new(crate::util::assets::Assets));
        let asset = |path| ProfileCardImage::asset(path, &renderer).unwrap();
        let avatar = Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_pixel(48, 48, image::Rgba([200, 80, 20, 255])),
        )]));
        ProfileCardImage {
            name: "duoc.phungvan".into(),
            username: "ozu".into(),
            link: "https://mezon.ai/chat/ozu".into(),
            labels: [
                mezon_i18n::t("en", "setting.account.qrProfile.brandName"),
                mezon_i18n::t("en", "profile.qr_profile"),
                mezon_i18n::t("en", "setting.account.qrProfile.verifiedByMezon"),
            ],
            theme: Arc::new(crate::theme::Theme::purple()),
            logo: Some(avatar.clone()),
            avatar,
            brand: asset(MEZON_LOGO),
            tick: asset(IconName::Tick.path()),
        }
    }

    fn sample_text_system() -> Arc<gpui::TextSystem> {
        Arc::new(gpui::TextSystem::new(Arc::new(gpui::NoopTextSystem::new())))
    }

    #[test]
    fn full_profile_png_has_embedded_assets_and_preserves_logo_colors() {
        let card = sample_card();
        let source = card
            .svg(&gpui::WindowTextSystem::new(sample_text_system()))
            .unwrap();
        assert!(source.contains("DUOC.PHUNGVAN"));
        assert!(source.contains("@ozu"));
        assert!(source.contains("data:image/png;base64,"));
        assert!(!source.contains("href=\"http"));
        let png = card.render(sample_text_system()).unwrap();
        let pixels = image::load_from_memory(&png.bytes).unwrap().to_rgba8();
        assert_eq!(pixels.dimensions(), (1020, 1152));
        assert_eq!(pixels.get_pixel(0, 0).0[3], 0);
        assert_eq!(pixels.get_pixel(170, 700).0, [255; 4]);
        assert_eq!(pixels.get_pixel(510, 682).0, [20, 80, 200, 255]);
        if let Some(path) = std::env::var_os("MEZON_PROFILE_QR_PREVIEW") {
            std::fs::write(path, &png.bytes).unwrap();
        }
    }

    #[test]
    fn profile_export_handles_escaped_unicode_names_themes_and_logo_fallbacks() {
        let mut card = sample_card();
        let system = gpui::WindowTextSystem::new(sample_text_system());
        card.name = "duoc.phungvan<&>".into();
        assert!(card.svg(&system).unwrap().contains("&lt;&amp;&gt;"));
        let (name, width) =
            ProfileCardImage::fitted(&"ozu 日本語 được ".repeat(80), 18., 222., &system);
        assert!(name.ends_with('…') && width <= 222.);
        let custom = card.render(sample_text_system()).unwrap();
        card.logo = None;
        let fallback = card.render(sample_text_system()).unwrap();
        assert_ne!(custom.bytes, fallback.bytes);
        card.logo = Some(card.brand.clone());
        let reset = card.render(sample_text_system()).unwrap();
        assert_ne!(fallback.bytes, reset.bytes);
        card.theme = Arc::new(crate::theme::Theme::light());
        let light = card.render(sample_text_system()).unwrap();
        assert_ne!(reset.bytes, light.bytes);
    }

    #[gpui::test]
    fn custom_logos_are_released_on_replace_reset_and_close(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::set_theme(crate::theme::Theme::dark(), cx));
        let (view, window) = cx.add_window_view(|_, cx| {
            QrProfileModal::new(
                account(),
                "https://mezon.ai/chat/ozu".into(),
                "en".into(),
                cx,
            )
        });
        let path = std::env::temp_dir().join(format!("mezon-qr-logo-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(48, 48, image::Rgba([20, 80, 200, 255]))
            .save(&path)
            .unwrap();
        window.update(|_, cx| view.update(cx, |this, cx| this.choose_logo(cx)));
        window.simulate_path_prompt_response(|_| Some(vec![path.clone()]));
        window.run_until_parked();
        let first = window.read(|cx| match &view.read(cx).center_logo {
            QrCenterLogo::Custom(image) => image.clone(),
            _ => panic!("picked logo"),
        });
        assert_eq!(&first.as_bytes(0).unwrap()[..4], &[200, 80, 20, 255]);
        std::fs::remove_file(path).unwrap();
        let second = sample_card().avatar;
        let last = sample_card().avatar;
        window.run_until_parked();
        window.update(|window, cx| {
            view.update(cx, |this, cx| {
                this.set_center_logo(QrCenterLogo::Custom(second.clone()), cx)
            });
            crate::image_cache::flush_atlas_drops(window, cx);
        });
        assert_eq!(Arc::strong_count(&first), 1);
        window.run_until_parked();
        window.update(|window, cx| {
            view.update(cx, |this, cx| this.set_center_logo(QrCenterLogo::Mezon, cx));
            crate::image_cache::flush_atlas_drops(window, cx);
        });
        assert_eq!(Arc::strong_count(&second), 1);
        window.update(|_, cx| {
            view.update(cx, |this, cx| {
                this.set_center_logo(QrCenterLogo::Custom(last.clone()), cx)
            })
        });
        window.run_until_parked();
        window.update(|window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();
        let (_, cleanup) = cx.add_window_view(|_, _| gpui::Empty);
        cleanup.update(|window, cx| crate::image_cache::flush_atlas_drops(window, cx));
        assert_eq!(Arc::strong_count(&last), 1);
    }

    #[gpui::test]
    fn copy_and_download_export_full_profile_without_screen_capture(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            Shell::init(cx);
            crate::theme::set_theme(crate::theme::Theme::dark(), cx);
        });
        let (view, window) = cx.add_window_view(|_, cx| {
            QrProfileModal::new(
                account(),
                "https://mezon.ai/chat/ozu".into(),
                "en".into(),
                cx,
            )
        });
        window.update(|_, cx| {
            view.update(cx, |this, cx| {
                this.loading = false;
                this.export(false, cx);
                assert!(this.exporting && !this.downloading);
            })
        });
        window.run_until_parked();
        let expected = window.update(|_, cx| {
            assert!(view.read(cx).copied && !view.read(cx).exporting);
            let clipboard = cx.read_from_clipboard().unwrap();
            let gpui::ClipboardEntry::Image(png) = &clipboard.entries()[0] else {
                panic!("profile PNG");
            };
            let dimensions = image::load_from_memory(&png.bytes).unwrap();
            assert_eq!((dimensions.width(), dimensions.height()), (1020, 1152));
            png.bytes.clone()
        });
        window.update(|_, cx| view.update(cx, |this, cx| this.export(true, cx)));
        window.run_until_parked();
        assert!(window.did_prompt_for_new_path());
        window.simulate_new_path_selection(|_| None);
        window.run_until_parked();
        assert!(window.read(|cx| !view.read(cx).downloading && !view.read(cx).exporting));
        let path = std::env::temp_dir().join(format!("mezon-profile-{}.png", std::process::id()));
        window.update(|_, cx| view.update(cx, |this, cx| this.export(true, cx)));
        window.run_until_parked();
        window.simulate_new_path_selection(|_| Some(path.clone()));
        window.run_until_parked();
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        std::fs::remove_file(path).unwrap();
        assert!(window.read(|cx| !view.read(cx).downloading));
    }
}
