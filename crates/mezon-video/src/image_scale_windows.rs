use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapFrameDecode, IWICBitmapSource,
    IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{Interface, PCWSTR};

use crate::ScaledImage;

fn imaging_factory() -> Option<IWICImagingFactory> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
    }
}

fn target_size(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest == 0 || longest <= max_px {
        return (width, height);
    }
    let scale = max_px as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn frame_to_scaled(
    factory: &IWICImagingFactory,
    frame: &IWICBitmapFrameDecode,
    max_px: u32,
) -> Option<ScaledImage> {
    unsafe {
        let mut width = 0u32;
        let mut height = 0u32;
        frame.GetSize(&mut width, &mut height).ok()?;
        if width == 0 || height == 0 {
            return None;
        }
        let (target_width, target_height) = target_size(width, height, max_px);

        let source: IWICBitmapSource = frame.cast().ok()?;

        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &source,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        let converted: IWICBitmapSource = converter.cast().ok()?;

        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(
                &converted,
                target_width,
                target_height,
                WICBitmapInterpolationModeFant,
            )
            .ok()?;
        let scaled_source: IWICBitmapSource = scaler.cast().ok()?;

        let stride = target_width.checked_mul(4)?;
        let size = (stride as usize).checked_mul(target_height as usize)?;
        let mut buffer = vec![0u8; size];
        scaled_source
            .CopyPixels(std::ptr::null(), stride, &mut buffer)
            .ok()?;

        Some(ScaledImage {
            width: target_width,
            height: target_height,
            bgra: buffer,
        })
    }
}

pub fn scaled_image_decode_path(path: &Path, max_px: u32) -> Option<ScaledImage> {
    let factory = imaging_factory()?;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        frame_to_scaled(&factory, &frame, max_px)
    }
}

pub fn scaled_image_decode(bytes: &[u8], max_px: u32) -> Option<ScaledImage> {
    let factory = imaging_factory()?;
    let mut owned = bytes.to_vec();
    unsafe {
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(&mut owned).ok()?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        frame_to_scaled(&factory, &frame, max_px)
    }
}
