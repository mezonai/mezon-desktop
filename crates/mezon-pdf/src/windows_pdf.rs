use windows::Data::Pdf::{PdfDocument as WinPdfDocument, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};

pub fn is_available() -> bool {
    true
}

pub fn unavailable_reason() -> Option<String> {
    None
}

pub struct Document {
    document: WinPdfDocument,
    pages: usize,
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
        Ok(Self { document, pages })
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    pub fn page_size(&self, index: usize) -> anyhow::Result<(f32, f32)> {
        let page = self.document.GetPage(index as u32)?;
        let size = page.Size()?;
        Ok((size.Width, size.Height))
    }

    pub fn render_page(&self, index: usize, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        let page = self.document.GetPage(index as u32)?;
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth(width)?;
        options.SetDestinationHeight(height)?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?
            .get()?;
        let encoded_len = u32::try_from(stream.Size()?)?;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(encoded_len)?.get()?;
        let mut encoded = vec![0u8; encoded_len as usize];
        reader.ReadBytes(&mut encoded)?;
        let decoded = image::load_from_memory(&encoded)?;
        let mut bgra = decoded.to_rgba8().into_raw();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Ok(bgra)
    }
}
