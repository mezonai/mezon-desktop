use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A muxer that dies mid-file still leaves a plausible-looking header behind,
/// and the Windows sink writer even reports success for one. An MP4 without its
/// `moov` index plays nowhere, so check the file really carries one before we
/// hand it to the user as a finished recording.
pub fn is_playable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.len() == 0 {
        return false;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("mp4") | Some("mov") | Some("m4a") => has_moov(path).unwrap_or(false),
        _ => true,
    }
}

fn has_moov(path: &Path) -> std::io::Result<bool> {
    let mut file = File::open(path)?;
    let end = file.seek(SeekFrom::End(0))?;
    let mut offset = 0u64;

    while offset + 8 <= end {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)?;
        if &header[4..8] == b"moov" {
            return Ok(true);
        }
        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let advance = match size {
            // Zero means "this box runs to the end of the file", so nothing
            // follows it; one means a 64-bit length comes after the type.
            0 => return Ok(false),
            1 => {
                let mut large = [0u8; 8];
                file.read_exact(&mut large)?;
                u64::from_be_bytes(large)
            }
            size => size,
        };
        if advance < 8 {
            return Ok(false);
        }
        offset += advance;
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write");
        (dir, path)
    }

    fn box_of(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn a_header_only_mp4_from_an_abandoned_sink_writer_is_not_playable() {
        // Byte for byte what Media Foundation left behind on Windows when the
        // sink writer never encoded a sample: ftyp, its OS-version uuid, and an
        // mdat that runs to the end of a file with nothing in it.
        let mut bytes = box_of(b"ftyp", b"mp42\0\0\0\0mp41isom");
        bytes.extend_from_slice(&box_of(
            b"uuid",
            &[
                0x5c, 0xa7, 0x08, 0xfb, 0x32, 0x8e, 0x42, 0x05, 0xa8, 0x61, 0x65, 0x0e, 0xca, 0x0a,
                0x95, 0x96, 0x00, 0x00, 0x00, 0x0c, b'1', b'0', b'.', b'0', b'.', b'1', b'9', b'0',
                b'4', b'5', b'.', b'0',
            ],
        ));
        bytes.extend_from_slice(&[
            0, 0, 0, 0, b'm', b'd', b'a', b't', 0, 0, 0, 0, 0, 0, 0, 0x10,
        ]);
        assert_eq!(bytes.len(), 80);

        let (_dir, path) = write("call.mp4", &bytes);
        assert!(!is_playable(&path));
    }

    #[test]
    fn an_mp4_that_carries_its_index_is_playable() {
        let mut bytes = box_of(b"ftyp", b"mp42\0\0\0\0mp41isom");
        bytes.extend_from_slice(&box_of(b"mdat", &[0u8; 32]));
        bytes.extend_from_slice(&box_of(b"moov", &[0u8; 16]));
        let (_dir, path) = write("call.mp4", &bytes);
        assert!(is_playable(&path));
    }

    #[test]
    fn a_container_we_cannot_inspect_only_has_to_be_non_empty() {
        let (_dir, webm) = write("call.webm", &[1, 2, 3, 4]);
        assert!(is_playable(&webm));
        let (_dir, empty) = write("empty.webm", &[]);
        assert!(!is_playable(&empty));
    }
}
