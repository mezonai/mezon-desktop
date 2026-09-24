pub fn is_available() -> bool {
    false
}

pub fn unavailable_reason() -> Option<String> {
    Some("pdf rendering is not available on this platform".to_string())
}

pub struct Document;

impl Document {
    pub fn open(_bytes: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::bail!("pdf rendering is not available on this platform")
    }

    pub fn page_count(&self) -> usize {
        0
    }

    pub fn page_size(&self, _index: usize) -> anyhow::Result<(f32, f32)> {
        anyhow::bail!("pdf rendering is not available on this platform")
    }

    pub fn render_page(&self, _index: usize, _width: u32, _height: u32) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("pdf rendering is not available on this platform")
    }
}
