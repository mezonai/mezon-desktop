use std::io::Read;
use std::path::{Path, PathBuf};

pub use mezon_store::PendingAttachment;

pub const MAX_FILE_ATTACHMENTS: usize = 50;
pub const IMAGE_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
pub const MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024;

pub const POSTER_MAX_EDGE: u32 = 480;

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
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::warn!("attachment skipped, cannot read {}: {err}", path.display());
            return None;
        }
    };
    if !meta.is_file() {
        tracing::warn!("attachment skipped, not a file: {}", path.display());
        return None;
    }
    let Some(filename) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        tracing::warn!("attachment skipped, no file name: {}", path.display());
        return None;
    };
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
    let duration = if filetype.starts_with("audio/") {
        audio_file_duration(&path, meta.len())
    } else {
        0
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
        duration,
        poster_jpeg,
    })
}

fn audio_file_duration(path: &Path, file_len: u64) -> i32 {
    const PREFIX: u64 = 256 * 1024;
    let read_len = usize::try_from(file_len.min(PREFIX)).unwrap_or(0);
    if read_len == 0 {
        return 0;
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return 0,
    };
    let mut buf = vec![0u8; read_len];
    let Ok(read) = file.read(&mut buf) else {
        return 0;
    };
    buf.truncate(read);
    mezon_audio::audio_duration_secs_with_len(&buf, file_len)
        .map(|secs| secs.floor() as i32)
        .filter(|secs| *secs > 0)
        .unwrap_or(0)
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
    fn build_pending_keeps_the_raw_vietnamese_name() {
        let dir = std::env::temp_dir().join("mezon_attachment_name_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Báo cáo tháng 8.pdf");
        std::fs::write(&path, b"%PDF-1.4").unwrap();

        let pending = build_pending(path.clone()).expect("a readable file builds a pending");
        assert_eq!(pending.filename, "Báo cáo tháng 8.pdf");
        assert_eq!(pending.filetype, "application/pdf");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn build_pending_reads_audio_duration() {
        let frames = 8_000u32;
        let data_len = frames * 2;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend(std::iter::repeat_n(0u8, data_len as usize));
        let path = std::env::temp_dir().join(format!("mezon-sound-{}.wav", std::process::id()));
        std::fs::write(&path, &wav).unwrap();
        let pending = build_pending(path.clone()).expect("wav builds");
        assert_eq!(pending.filetype, "audio/wav");
        assert_eq!(pending.duration, 1);
        assert_eq!(pending.size, wav.len() as u64);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn build_pending_skips_a_directory_and_a_missing_path() {
        assert!(build_pending(std::env::temp_dir()).is_none());
        assert!(build_pending(std::env::temp_dir().join("mezon_no_such_file_xyz")).is_none());
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
