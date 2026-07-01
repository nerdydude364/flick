use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

const CHUNK: u64 = 64 * 1024;
const HASH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// `size`/`mtime` let a cache hit be rejected cheaply (a `stat()`) when the
/// file at `path` has since changed, instead of trusting a memoized hash
/// forever for the life of the process.
struct CachedHash {
    size: u64,
    mtime: Option<SystemTime>,
    hash: String,
}

static CONTENT_HASH_CACHE: LazyLock<Mutex<HashMap<PathBuf, CachedHash>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn stat_fingerprint(path: &Path) -> std::io::Result<(u64, Option<SystemTime>)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.len(), meta.modified().ok()))
}

/// Seeds the process-wide content-hash cache (e.g. from a folder scan on a
/// background thread) so gallery thumbnail workers don't re-read 128 KB per
/// file. The caller has just re-read `path`'s current bytes to produce
/// `hash`, so it always overwrites whatever was cached before — otherwise a
/// re-scan after a same-path file replacement would keep serving the old
/// file's hash (and therefore its thumbnail/sprite) forever.
pub fn prime_content_hash(path: PathBuf, hash: String) {
    let (size, mtime) = match stat_fingerprint(&path) {
        Ok(fp) => fp,
        Err(_) => return,
    };
    if let Ok(mut cache) = CONTENT_HASH_CACHE.lock() {
        cache.insert(path, CachedHash { size, mtime, hash });
    }
}

/// Like [`hash_video_file`], but memoized per path for the lifetime of the
/// process. A cheap `stat()` guards every lookup so a file replaced at the
/// same path is detected and re-hashed rather than trusting a stale memo.
pub fn hash_video_file_cached(path: &Path) -> std::io::Result<String> {
    let path_buf = path.to_path_buf();
    let (size, mtime) = stat_fingerprint(path)?;
    if let Ok(cache) = CONTENT_HASH_CACHE.lock()
        && let Some(cached) = cache.get(&path_buf)
        && cached.size == size
        && cached.mtime == mtime
    {
        return Ok(cached.hash.clone());
    }
    let hash = hash_video_file_bounded(path)?;
    if let Ok(mut cache) = CONTENT_HASH_CACHE.lock() {
        cache.insert(
            path_buf,
            CachedHash {
                size,
                mtime,
                hash: hash.clone(),
            },
        );
    }
    Ok(hash)
}

/// Runs [`hash_video_file`] on a helper thread with a hard timeout, so a
/// stalled read (network mount, disconnected removable drive) can't wedge
/// the calling worker/pool thread indefinitely — mirrors the timeout guard
/// `thumbnails/frame.rs` already uses around mpv frame extraction/probing.
fn hash_video_file_bounded(path: &Path) -> std::io::Result<String> {
    let path_buf = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(hash_video_file(&path_buf));
    });
    rx.recv_timeout(HASH_TIMEOUT).unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "hash_video_file timed out",
        ))
    })
}

/// Fast content fingerprint: size + first 64 KB + last 64 KB. Exact port of
/// `hashVideoFile` in main.js — used as the sprite cache key, so it must stay
/// stable across the rewrite (same video should reuse an existing Electron-era
/// cache key shape, even though the cache directory itself has moved).
pub fn hash_video_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();

    let mut hasher = Sha1::new();
    hasher.update(file_size.to_string().as_bytes());

    let mut head = vec![0u8; CHUNK.min(file_size) as usize];
    file.read_exact(&mut head)?;
    hasher.update(&head);

    if file_size > CHUNK * 2 {
        file.seek(SeekFrom::Start(file_size - CHUNK))?;
        let mut tail = vec![0u8; CHUNK as usize];
        file.read_exact(&mut tail)?;
        hasher.update(&tail);
    }

    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn hash_is_stable_for_same_content() {
        let path = std::env::temp_dir().join("flick-hash-test-stable.bin");
        std::fs::write(&path, vec![7u8; 1000]).unwrap();
        let a = hash_video_file(&path).unwrap();
        let b = hash_video_file(&path).unwrap();
        assert_eq!(a, b);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hash_differs_for_different_content() {
        let path_a = std::env::temp_dir().join("flick-hash-test-a.bin");
        let path_b = std::env::temp_dir().join("flick-hash-test-b.bin");
        std::fs::write(&path_a, vec![1u8; 1000]).unwrap();
        std::fs::write(&path_b, vec![2u8; 1000]).unwrap();
        assert_ne!(
            hash_video_file(&path_a).unwrap(),
            hash_video_file(&path_b).unwrap()
        );
        std::fs::remove_file(&path_a).unwrap();
        std::fs::remove_file(&path_b).unwrap();
    }

    #[test]
    fn handles_files_larger_than_two_chunks() {
        let path = std::env::temp_dir().join("flick-hash-test-large.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(&vec![3u8; (CHUNK * 3) as usize]).unwrap();
        let hash = hash_video_file(&path).unwrap();
        assert_eq!(hash.len(), 40); // SHA-1 hex digest length
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn prime_content_hash_overwrites_stale_entry() {
        let path = std::env::temp_dir().join("flick-hash-test-prime-overwrite.bin");
        std::fs::write(&path, vec![9u8; 1000]).unwrap();
        prime_content_hash(path.clone(), "aaaa".to_string());
        prime_content_hash(path.clone(), "bbbb".to_string());
        let cache = CONTENT_HASH_CACHE.lock().unwrap();
        assert_eq!(cache.get(&path).unwrap().hash, "bbbb");
        drop(cache);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hash_video_file_cached_detects_content_change() {
        let path = std::env::temp_dir().join("flick-hash-test-cached-change.bin");
        std::fs::write(&path, vec![1u8; 1000]).unwrap();
        let first = hash_video_file_cached(&path).unwrap();

        // Different length guarantees a different (size, mtime) fingerprint
        // even if the filesystem's mtime resolution is coarse.
        std::fs::write(&path, vec![2u8; 2000]).unwrap();
        let second = hash_video_file_cached(&path).unwrap();

        assert_ne!(first, second);
        assert_eq!(second, hash_video_file(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hash_video_file_cached_reuses_unchanged_file() {
        let path = std::env::temp_dir().join("flick-hash-test-cached-stable.bin");
        std::fs::write(&path, vec![5u8; 1000]).unwrap();
        let first = hash_video_file_cached(&path).unwrap();
        let second = hash_video_file_cached(&path).unwrap();
        assert_eq!(first, second);
        std::fs::remove_file(&path).unwrap();
    }
}
