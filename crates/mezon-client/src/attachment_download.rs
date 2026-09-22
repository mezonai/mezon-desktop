use std::path::{Path, PathBuf};

pub fn clean_download_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned = trimmed.replace("@webp", "");
    let parsed = url::Url::parse(&cleaned).ok()?;
    match parsed.scheme() {
        "http" | "https" => Some(cleaned),
        _ => None,
    }
}

/// The name a download is saved under.
///
/// A sound picked from the panel is sent as its label — `u need to leave` — while
/// the object itself is `…/2097582928409137152.wav`; the file box shows the label,
/// so the label is what the save dialog must suggest. Saved bare, though, the file
/// has no type: Windows and most Linux desktops open it as text. Keep the label
/// and borrow the extension from the URL; with no label at all, the URL's own
/// name is the best there is.
pub fn resolve_download_filename(filename: &str, url: &str) -> String {
    let from_name = sanitize_filename(filename);
    let has_name = !filename.trim().is_empty();
    if has_name && extension_of(&from_name).is_some() {
        return from_name;
    }
    let from_url = url
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .filter(|segment| !segment.is_empty())
        .map(sanitize_filename)
        .filter(|segment| segment != "download");
    match from_url {
        Some(from_url) if !has_name => from_url,
        Some(from_url) => match extension_of(&from_url) {
            Some(ext) => format!("{from_name}.{ext}"),
            None => from_name,
        },
        None => from_name,
    }
}

fn extension_of(name: &str) -> Option<&str> {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
}

pub fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

pub async fn download_url_to_downloads(url: &str, filename: &str) -> anyhow::Result<PathBuf> {
    let url = clean_download_url(url).ok_or_else(|| anyhow::anyhow!("invalid download url"))?;
    let filename = resolve_download_filename(filename, &url);
    let (bytes, _) = crate::transport_runtime::fetch_bytes(&url).await?;
    write_bytes_to_downloads(&filename, &bytes).await
}

pub async fn write_bytes_to_downloads(filename: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow::anyhow!("no download directory available"))?;
    let filename = filename.to_string();
    let bytes = bytes.to_vec();
    crate::transport_runtime::handle()
        .spawn_blocking(move || write_bytes_to_downloads_sync(&dir, &filename, &bytes))
        .await
        .map_err(|e| anyhow::anyhow!("file write task failed: {e}"))?
}

/// Pick a free path for `filename` inside `dir`.
///
/// `filename` is a *name*, never a path: it is sanitized first, so a server-supplied
/// `../../autostart/x.desktop` or `/home/u/.bashrc` can never place the file outside
/// `dir`. An empty or path-only name falls back to `download`.
///
/// The name is only reserved once it is written; use [`reserve_path_in`] when two
/// writers can race for the same name.
pub fn unique_path_in(dir: &Path, filename: &str) -> PathBuf {
    let filename = sanitize_filename(filename);
    let path = Path::new(&filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str());

    let mut candidate = dir.join(&filename);
    let mut counter = 1u32;
    while candidate.exists() {
        let name = match ext {
            Some(ext) => format!("{stem} ({counter}).{ext}"),
            None => format!("{stem} ({counter})"),
        };
        candidate = dir.join(name);
        counter += 1;
        if counter > 9999 {
            break;
        }
    }
    candidate
}

/// Claim a free path for `filename` inside `dir` by creating the file, so two
/// downloads started at the same time cannot settle on the same name. Creates `dir`
/// when it is missing, which is the norm on a minimal Linux install.
pub fn reserve_path_in(dir: &Path, filename: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let filename = sanitize_filename(filename);
    let path = Path::new(&filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str());

    let mut candidate = dir.join(&filename);
    let mut counter = 1u32;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if counter > 9999 {
                    return Err(error);
                }
                let name = match ext {
                    Some(ext) => format!("{stem} ({counter}).{ext}"),
                    None => format!("{stem} ({counter})"),
                };
                candidate = dir.join(name);
                counter += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn write_bytes_to_downloads_sync(
    dir: &Path,
    filename: &str,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).ok();
    let candidate = unique_path_in(dir, filename);
    std::fs::write(&candidate, bytes)?;
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_download_url_strips_webp_suffix() {
        let url = "https://cdn.example.com/a.png@webp";
        assert_eq!(
            clean_download_url(url).as_deref(),
            Some("https://cdn.example.com/a.png")
        );
    }

    #[test]
    fn clean_download_url_rejects_non_http() {
        assert!(clean_download_url("file:///tmp/x").is_none());
    }

    #[test]
    fn resolve_filename_falls_back_to_url_segment() {
        assert_eq!(
            resolve_download_filename("", "https://cdn.example.com/photo.jpg?token=1"),
            "photo.jpg"
        );
    }

    #[test]
    fn a_bare_label_borrows_the_extension_from_the_url() {
        // A sound from the picker: the attachment is named after the sound, the
        // object on the CDN after its id.
        assert_eq!(
            resolve_download_filename(
                "u need to leave",
                "https://cdn.komu.vn/1840673171137630208/2097582928409137152.wav"
            ),
            "u need to leave.wav"
        );
        assert_eq!(
            resolve_download_filename(
                "ngu-ngoc",
                "https://cdn.komu.vn/1785968850517364736/2085204327911133184.mp3?x=1"
            ),
            "ngu-ngoc.mp3"
        );
    }

    #[test]
    fn a_named_file_keeps_its_own_extension() {
        assert_eq!(
            resolve_download_filename("report.pdf", "https://cdn.example.com/abc.bin"),
            "report.pdf"
        );
    }

    #[test]
    fn a_bare_label_stays_bare_when_the_url_has_no_extension_either() {
        assert_eq!(
            resolve_download_filename("notes", "https://cdn.example.com/objects/abc"),
            "notes"
        );
        assert_eq!(
            resolve_download_filename("notes", "https://cdn.example.com/"),
            "notes"
        );
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("mezon-{label}-{}-{unique}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_name_from_the_server_cannot_climb_out_of_the_directory() {
        let dir = Path::new("/home/u/Downloads");
        for hostile in [
            "../../.config/autostart/x.desktop",
            "/etc/cron.d/x",
            "..",
            "sub/a.txt",
            "..\\..\\Startup\\x.lnk",
        ] {
            let path = unique_path_in(dir, hostile);
            assert_eq!(
                path.parent(),
                Some(dir),
                "{hostile} escaped the download directory: {path:?}"
            );
        }
    }

    #[test]
    fn an_empty_name_becomes_a_file_not_the_directory_itself() {
        let dir = Path::new("/home/u/Downloads");
        for blank in ["", "   ", "/", "//"] {
            let path = unique_path_in(dir, blank);
            assert_ne!(path, dir, "{blank:?} resolved to the directory itself");
            assert_eq!(path, dir.join("download"));
        }
    }

    #[test]
    fn the_first_name_and_the_collision_names_live_in_one_directory() {
        let dir = scratch_dir("uniq");
        let first = unique_path_in(&dir, "sub/a.txt");
        std::fs::write(&first, b"x").expect("write first");
        let second = unique_path_in(&dir, "sub/a.txt");
        assert_eq!(first, dir.join("a.txt"));
        assert_eq!(second, dir.join("a (1).txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reserving_a_path_creates_the_directory_and_claims_the_name() {
        let dir = scratch_dir("reserve").join("Downloads");
        assert!(!dir.exists(), "the directory must be missing to start with");

        let first = reserve_path_in(&dir, "ca.crt").expect("reserve first");
        assert_eq!(first, dir.join("ca.crt"));
        assert!(
            first.exists(),
            "an unclaimed name lets a second download take it"
        );

        let second = reserve_path_in(&dir, "ca.crt").expect("reserve second");
        assert_eq!(second, dir.join("ca (1).crt"));
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn reserving_sanitizes_the_name_too() {
        let dir = scratch_dir("reserve-hostile");
        let path = reserve_path_in(&dir, "../../../../etc/passwd").expect("reserve");
        assert_eq!(path, dir.join("passwd"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
