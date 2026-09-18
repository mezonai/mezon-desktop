use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as backend;

#[cfg(windows)]
mod windows_pdf;
#[cfg(windows)]
use windows_pdf as backend;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
use unsupported as backend;

pub const MAX_RENDER_EDGE_PX: u32 = 4096;

pub struct PdfBitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct PdfPageSize {
    pub width: f32,
    pub height: f32,
}

pub struct PdfDocument {
    document: Mutex<backend::Document>,
    page_count: usize,
}

impl PdfDocument {
    pub fn from_bytes(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let document = backend::Document::open(bytes)?;
        let page_count = document.page_count();
        if page_count == 0 {
            anyhow::bail!("pdf has no pages");
        }
        Ok(Self {
            document: Mutex::new(document),
            page_count,
        })
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn page_size(&self, index: usize) -> anyhow::Result<PdfPageSize> {
        self.check_index(index)?;
        let document = self.lock()?;
        let (width, height) = document.page_size(index)?;
        Ok(PdfPageSize { width, height })
    }

    pub fn render_page(&self, index: usize, width: u32, height: u32) -> anyhow::Result<PdfBitmap> {
        self.check_index(index)?;
        let width = width.clamp(1, MAX_RENDER_EDGE_PX);
        let height = height.clamp(1, MAX_RENDER_EDGE_PX);
        let document = self.lock()?;
        let bgra = document.render_page(index, width, height)?;
        let expected = width as usize * height as usize * 4;
        if bgra.len() != expected {
            anyhow::bail!(
                "pdf renderer produced {} bytes, expected {expected}",
                bgra.len()
            );
        }
        Ok(PdfBitmap {
            width,
            height,
            bgra,
        })
    }

    fn check_index(&self, index: usize) -> anyhow::Result<()> {
        if index >= self.page_count {
            anyhow::bail!("page {index} is out of range of {} pages", self.page_count);
        }
        Ok(())
    }

    fn lock(&self) -> anyhow::Result<std::sync::MutexGuard<'_, backend::Document>> {
        self.document
            .lock()
            .map_err(|_| anyhow::anyhow!("pdf document lock poisoned"))
    }
}

pub fn is_supported() -> bool {
    backend::is_available()
}

pub fn unavailable_reason() -> Option<String> {
    backend::unavailable_reason()
}

pub fn fit_page_pixels(page: &PdfPageSize, target_width_px: f32) -> (u32, u32) {
    let width_pt = page.width.max(1.0);
    let height_pt = page.height.max(1.0);
    let width_px = target_width_px.max(1.0).min(MAX_RENDER_EDGE_PX as f32);
    let height_px = (width_px * height_pt / width_pt).max(1.0);
    if height_px > MAX_RENDER_EDGE_PX as f32 {
        let capped_height = MAX_RENDER_EDGE_PX as f32;
        let capped_width = (capped_height * width_pt / height_pt).max(1.0);
        return (capped_width.round() as u32, capped_height.round() as u32);
    }
    (width_px.round() as u32, height_px.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_page_keeps_aspect_ratio() {
        let page = PdfPageSize {
            width: 612.0,
            height: 792.0,
        };
        let (w, h) = fit_page_pixels(&page, 800.0);
        assert_eq!(w, 800);
        assert_eq!(h, 1035);
    }

    #[test]
    fn fit_page_caps_the_long_edge() {
        let page = PdfPageSize {
            width: 100.0,
            height: 4000.0,
        };
        let (w, h) = fit_page_pixels(&page, 800.0);
        assert_eq!(h, MAX_RENDER_EDGE_PX);
        assert_eq!(w, 102);
    }
}
