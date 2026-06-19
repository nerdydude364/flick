use std::fs::File;
use std::io::Read;
use std::path::Path;

fn read_magic_bytes(path: &Path, count: usize) -> Vec<u8> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let mut buf = vec![0u8; count];
    match file.read(&mut buf) {
        Ok(n) => {
            buf.truncate(n);
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// Verifies that a file's content matches expected magic bytes for its media
/// type. Returns true for extensions where false positives are unlikely
/// (e.g. .mp4, .mov) — port of `confirmMediaMagic` in the Electron app's
/// main.js, same byte offsets and thresholds.
pub fn confirm_media_magic(path: &Path, ext_lower: &str) -> bool {
    match ext_lower {
        "ts" | "mts" => {
            // MPEG Transport Stream: sync byte 0x47 at positions 0 and 188 (188-byte packets)
            let buf = read_magic_bytes(path, 189);
            buf.len() >= 189 && buf[0] == 0x47 && buf[188] == 0x47
        }
        "jpg" | "jpeg" => {
            let buf = read_magic_bytes(path, 3);
            buf.len() >= 3 && buf[0] == 0xff && buf[1] == 0xd8 && buf[2] == 0xff
        }
        "png" => {
            let buf = read_magic_bytes(path, 4);
            buf.len() >= 4 && buf[0] == 0x89 && buf[1] == 0x50 && buf[2] == 0x4e && buf[3] == 0x47
        }
        "gif" => {
            let buf = read_magic_bytes(path, 4);
            buf.len() >= 4 && &buf[0..4] == b"GIF8"
        }
        "webp" => {
            let buf = read_magic_bytes(path, 12);
            buf.len() >= 12 && &buf[0..4] == b"RIFF" && &buf[8..12] == b"WEBP"
        }
        "bmp" => {
            let buf = read_magic_bytes(path, 2);
            buf.len() >= 2 && buf[0] == 0x42 && buf[1] == 0x4d
        }
        "tiff" | "tif" => {
            let buf = read_magic_bytes(path, 4);
            if buf.len() < 4 {
                return false;
            }
            (buf[0] == 0x49 && buf[1] == 0x49 && buf[2] == 0x2a && buf[3] == 0x00)
                || (buf[0] == 0x4d && buf[1] == 0x4d && buf[2] == 0x00 && buf[3] == 0x2a)
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("flick-magic-test-{name}"));
        let mut f = File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn validates_png_magic_bytes() {
        let path = write_temp("a.png", &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a]);
        assert!(confirm_media_magic(&path, "png"));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_mismatched_png_magic_bytes() {
        let path = write_temp("b.png", b"not a png");
        assert!(!confirm_media_magic(&path, "png"));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn trusts_unlisted_extensions_by_default() {
        let path = write_temp("c.mp4", b"anything");
        assert!(confirm_media_magic(&path, "mp4"));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn validates_transport_stream_sync_bytes() {
        let mut bytes = vec![0u8; 189];
        bytes[0] = 0x47;
        bytes[188] = 0x47;
        let path = write_temp("d.ts", &bytes);
        assert!(confirm_media_magic(&path, "ts"));
        std::fs::remove_file(&path).unwrap();
    }
}
