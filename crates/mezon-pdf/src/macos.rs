use std::sync::Arc;

use core_graphics::color_space::CGColorSpace;
use core_graphics::context::{CGContext, CGInterpolationQuality};
use core_graphics::data_provider::CGDataProvider;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::{CGContextRef, CGDataProviderRef};
use foreign_types::ForeignType;

const CG_PDF_CROP_BOX: i32 = 1;
const CG_BITMAP_BGRA_PREMULTIPLIED: u32 = 2 | (2 << 12);

enum CGPDFDocumentOpaque {}
type CGPDFDocumentRef = *mut CGPDFDocumentOpaque;

enum CGPDFPageOpaque {}
type CGPDFPageRef = *mut CGPDFPageOpaque;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPDFDocumentCreateWithProvider(provider: CGDataProviderRef) -> CGPDFDocumentRef;
    fn CGPDFDocumentRelease(document: CGPDFDocumentRef);
    fn CGPDFDocumentGetNumberOfPages(document: CGPDFDocumentRef) -> usize;
    fn CGPDFDocumentGetPage(document: CGPDFDocumentRef, page_number: usize) -> CGPDFPageRef;
    fn CGPDFDocumentIsEncrypted(document: CGPDFDocumentRef) -> bool;
    fn CGPDFDocumentIsUnlocked(document: CGPDFDocumentRef) -> bool;
    fn CGPDFPageGetBoxRect(page: CGPDFPageRef, box_type: i32) -> CGRect;
    fn CGPDFPageGetRotationAngle(page: CGPDFPageRef) -> i32;
    fn CGContextDrawPDFPage(context: CGContextRef, page: CGPDFPageRef);
}

pub fn is_available() -> bool {
    true
}

pub fn unavailable_reason() -> Option<String> {
    None
}

pub struct Document {
    document: CGPDFDocumentRef,
    pages: usize,
}

unsafe impl Send for Document {}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe { CGPDFDocumentRelease(self.document) };
    }
}

impl Document {
    pub fn open(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let provider = CGDataProvider::from_buffer(Arc::new(bytes));
        let raw = unsafe { CGPDFDocumentCreateWithProvider(provider.as_ptr()) };
        if raw.is_null() {
            anyhow::bail!("file is not a readable pdf");
        }
        let mut document = Self {
            document: raw,
            pages: 0,
        };
        if unsafe { CGPDFDocumentIsEncrypted(raw) } && !unsafe { CGPDFDocumentIsUnlocked(raw) } {
            anyhow::bail!("pdf is password protected");
        }
        document.pages = unsafe { CGPDFDocumentGetNumberOfPages(raw) };
        Ok(document)
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    pub fn page_size(&self, index: usize) -> anyhow::Result<(f32, f32)> {
        let page = self.page(index)?;
        let rect = unsafe { CGPDFPageGetBoxRect(page, CG_PDF_CROP_BOX) };
        let quarter_turned = unsafe { CGPDFPageGetRotationAngle(page) }.rem_euclid(180) == 90;
        let width = rect.size.width as f32;
        let height = rect.size.height as f32;
        if quarter_turned {
            Ok((height, width))
        } else {
            Ok((width, height))
        }
    }

    pub fn render_page(&self, index: usize, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        let page = self.page(index)?;
        let color_space = CGColorSpace::create_device_rgb();
        let mut context = CGContext::create_bitmap_context(
            None,
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            &color_space,
            CG_BITMAP_BGRA_PREMULTIPLIED,
        );
        let canvas = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width as f64, height as f64),
        );
        context.set_interpolation_quality(CGInterpolationQuality::CGInterpolationQualityHigh);
        context.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);
        context.fill_rect(canvas);
        context.save();
        let box_rect = unsafe { CGPDFPageGetBoxRect(page, CG_PDF_CROP_BOX) };
        let rotation = unsafe { CGPDFPageGetRotationAngle(page) }.rem_euclid(360);
        let (box_width, box_height) = (box_rect.size.width, box_rect.size.height);
        let (upright_width, upright_height) = if rotation == 90 || rotation == 270 {
            (box_height, box_width)
        } else {
            (box_width, box_height)
        };
        context.scale(
            width as f64 / upright_width.max(1.0),
            height as f64 / upright_height.max(1.0),
        );
        match rotation {
            90 => {
                context.translate(0.0, upright_height);
                context.rotate(-std::f64::consts::FRAC_PI_2);
            }
            180 => {
                context.translate(upright_width, upright_height);
                context.rotate(std::f64::consts::PI);
            }
            270 => {
                context.translate(upright_width, 0.0);
                context.rotate(std::f64::consts::FRAC_PI_2);
            }
            _ => {}
        }
        context.translate(-box_rect.origin.x, -box_rect.origin.y);
        unsafe { CGContextDrawPDFPage(context.as_ptr(), page) };
        context.restore();
        Ok(context.data().to_vec())
    }

    fn page(&self, index: usize) -> anyhow::Result<CGPDFPageRef> {
        let page = unsafe { CGPDFDocumentGetPage(self.document, index + 1) };
        if page.is_null() {
            anyhow::bail!("pdf page {index} is missing");
        }
        Ok(page)
    }
}
