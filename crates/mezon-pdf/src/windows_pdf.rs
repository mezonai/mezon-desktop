use std::cell::Cell;
use std::sync::OnceLock;

use windows::Data::Pdf::{PdfDocument as WinPdfDocument, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};

const POINTS_PER_DIP: f32 = 72.0 / 96.0;

/// `Windows.Data.Pdf` ships with the desktop SKUs, but Server Core and the trimmed
/// images leave the class unregistered, and activating it there fails with
/// `REGDB_E_CLASSNOTREG`. Probe once so a machine that genuinely cannot render says
/// so with the reason attached, instead of reporting every document as unreadable.
fn probe() -> &'static Option<String> {
    static PROBE: OnceLock<Option<String>> = OnceLock::new();
    PROBE.get_or_init(|| match PdfPageRenderOptions::new() {
        Ok(_) => None,
        Err(error) => Some(format!("Windows.Data.Pdf could not be activated: {error}")),
    })
}

pub fn is_available() -> bool {
    probe().is_none()
}

pub fn unavailable_reason() -> Option<String> {
    probe().clone()
}

pub struct Document {
    document: WinPdfDocument,
    pages: usize,
    /// `Windows.Data.Pdf` treats `DestinationWidth`/`DestinationHeight` as DIPs and
    /// multiplies them by the system DPI scale, so a 1200px request on a 150% display
    /// comes back as an 1800px bitmap. Learned from the first render and used to shrink
    /// later requests so the renderer lands on the size the caller asked for.
    render_scale: Cell<f32>,
}

unsafe impl Send for Document {}

impl Document {
    pub fn open(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let stream = InMemoryRandomAccessStream::new()?;
        let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0)?)?;
        writer.WriteBytes(&bytes)?;
        writer.StoreAsync()?.get()?;
        writer.FlushAsync()?.get()?;
        writer.DetachStream()?;
        stream.Seek(0)?;
        let document = WinPdfDocument::LoadFromStreamAsync(&stream)?.get()?;
        let pages = document.PageCount()? as usize;
        Ok(Self {
            document,
            pages,
            render_scale: Cell::new(1.0),
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    pub fn page_size(&self, index: usize) -> anyhow::Result<(f32, f32)> {
        let page = self.document.GetPage(index as u32)?;
        let size = page.Size()?;
        Ok((size.Width * POINTS_PER_DIP, size.Height * POINTS_PER_DIP))
    }

    pub fn render_page(&self, index: usize, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        let page = self.document.GetPage(index as u32)?;
        let options = PdfPageRenderOptions::new()?;
        let scale = self.render_scale.get();
        let request = |edge: u32| ((edge as f32 / scale).round() as u32).max(1);
        options.SetDestinationWidth(request(width))?;
        options.SetDestinationHeight(request(height))?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?
            .get()?;
        let encoded_len = u32::try_from(stream.Size()?)?;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(encoded_len)?.get()?;
        let mut encoded = vec![0u8; encoded_len as usize];
        reader.ReadBytes(&mut encoded)?;
        let mut decoded = image::load_from_memory(&encoded)?.into_rgba8();
        let (got_width, got_height) = decoded.dimensions();
        if (got_width, got_height) != (width, height) {
            // The renderer applied its DPI scale on top of our request: remember it for
            // the next page and bring this one to the size the caller asked for.
            let observed = got_width as f32 * scale / width as f32;
            if observed.is_finite() && observed > 0.0 {
                self.render_scale.set(observed);
            }
            decoded = image::imageops::resize(
                &decoded,
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
        }
        let mut bgra = decoded.into_raw();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Ok(bgra)
    }
}
