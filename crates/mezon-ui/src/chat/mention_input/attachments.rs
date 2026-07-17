use std::path::{Path, PathBuf};

pub const MAX_FILE_ATTACHMENTS: usize = 50;
pub const IMAGE_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
pub const MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024;

pub const POSTER_MAX_EDGE: u32 = 480;

#[derive(Debug, Clone)]
pub struct PendingAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub filetype: String,
    pub size: u64,
    pub is_image: bool,
    pub is_video: bool,
    pub width: u32,
    pub height: u32,
    pub duration: i32,
    pub poster_jpeg: Option<Vec<u8>>,
}

pub enum AttachmentLimit {
    Count,
    Size(u64),
}

pub fn mime_from_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

pub fn build_pending(path: PathBuf) -> Option<PendingAttachment> {
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let filename = path.file_name()?.to_str()?.to_string();
    let filetype = mime_from_extension(&path);
    let is_image = filetype.starts_with("image/");
    let is_video = filetype.starts_with("video/");
    let (width, height, poster_jpeg) = if is_video {
        match mezon_video::probe_video(&path.to_string_lossy(), POSTER_MAX_EDGE) {
            Some(probe) => (probe.width, probe.height, probe.poster_jpeg),
            None => (0, 0, None),
        }
    } else if is_image {
        let (w, h) = image::image_dimensions(&path).unwrap_or((0, 0));
        (w, h, None)
    } else {
        (0, 0, None)
    };
    Some(PendingAttachment {
        path,
        filename,
        filetype,
        size: meta.len(),
        is_image,
        is_video,
        width,
        height,
        duration: 0,
        poster_jpeg,
    })
}

pub fn size_limit_for(is_image: bool) -> u64 {
    if is_image {
        IMAGE_MAX_FILE_SIZE
    } else {
        MAX_FILE_SIZE
    }
}

pub fn validate_batch(
    existing: usize,
    candidates: &[PendingAttachment],
) -> Result<(), AttachmentLimit> {
    if existing + candidates.len() > MAX_FILE_ATTACHMENTS {
        return Err(AttachmentLimit::Count);
    }
    for candidate in candidates {
        let limit = size_limit_for(candidate.is_image);
        if candidate.size > limit {
            return Err(AttachmentLimit::Size(limit));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_maps_known_extensions() {
        assert_eq!(mime_from_extension(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_from_extension(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(mime_from_extension(Path::new("clip.mp4")), "video/mp4");
        assert_eq!(mime_from_extension(Path::new("doc.pdf")), "application/pdf");
        assert_eq!(
            mime_from_extension(Path::new("noext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn size_limit_differs_for_image_vs_file() {
        assert_eq!(size_limit_for(true), IMAGE_MAX_FILE_SIZE);
        assert_eq!(size_limit_for(false), MAX_FILE_SIZE);
    }

    #[test]
    fn validate_batch_enforces_count_and_size() {
        let img = PendingAttachment {
            path: PathBuf::from("a.png"),
            filename: "a.png".into(),
            filetype: "image/png".into(),
            size: 1024,
            is_image: true,
            is_video: false,
            width: 0,
            height: 0,
            duration: 0,
            poster_jpeg: None,
        };
        assert!(validate_batch(0, std::slice::from_ref(&img)).is_ok());
        assert!(matches!(
            validate_batch(MAX_FILE_ATTACHMENTS, std::slice::from_ref(&img)),
            Err(AttachmentLimit::Count)
        ));
        let big = PendingAttachment {
            size: IMAGE_MAX_FILE_SIZE + 1,
            ..img.clone()
        };
        assert!(matches!(
            validate_batch(0, std::slice::from_ref(&big)),
            Err(AttachmentLimit::Size(IMAGE_MAX_FILE_SIZE))
        ));
    }
}
