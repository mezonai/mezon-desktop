use std::io::Cursor;
use std::sync::Arc;

use gpui::{Image as ClipboardImage, ImageFormat, RenderImage};
use qrcode::EcLevel;

pub(crate) struct QrImage {
    pub(crate) render: Arc<RenderImage>,
    pub(crate) clipboard: ClipboardImage,
}

#[derive(Clone, Copy)]
pub(crate) struct QrImageOptions {
    pub(crate) target_size: usize,
    pub(crate) min_module_scale: usize,
    pub(crate) error_correction: EcLevel,
    pub(crate) clipboard_border: u32,
}

pub(crate) fn build_qr_image(data: &str, options: QrImageOptions) -> Option<QrImage> {
    let code =
        qrcode::QrCode::with_error_correction_level(data.as_bytes(), options.error_correction)
            .ok()?;
    let width = code.width();
    if width == 0 {
        return None;
    }

    let scale = (options.target_size / width).max(options.min_module_scale);
    let dimension = (width * scale) as u32;
    let mut buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        dimension,
        dimension,
        image::Rgba([255, 255, 255, 255]),
    );

    for (index, color) in code.to_colors().iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let origin_x = ((index % width) * scale) as u32;
        let origin_y = ((index / width) * scale) as u32;
        for y in 0..scale as u32 {
            for x in 0..scale as u32 {
                buffer.put_pixel(origin_x + x, origin_y + y, image::Rgba([0, 0, 0, 255]));
            }
        }
    }

    let clipboard_buffer = if options.clipboard_border == 0 {
        buffer.clone()
    } else {
        let border = options.clipboard_border;
        let mut bordered = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
            dimension + border * 2,
            dimension + border * 2,
            image::Rgba([255, 255, 255, 255]),
        );
        image::imageops::overlay(&mut bordered, &buffer, border.into(), border.into());
        bordered
    };

    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(clipboard_buffer)
        .write_to(&mut png, image::ImageFormat::Png)
        .ok()?;
    let clipboard = ClipboardImage::from_bytes(ImageFormat::Png, png.into_inner());
    let render = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));

    Some(QrImage { render, clipboard })
}
