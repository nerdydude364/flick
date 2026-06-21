use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

const CHUNK: u64 = 64 * 1024;

static CONTENT_HASH_CACHE: LazyLock<Mutex<HashMap<PathBuf, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Seeds the process-wide content-hash cache (e.g. from a folder scan on a
/// background thread) so gallery thumbnail workers don't re-read 128 KB per file.
pub fn prime_content_hash(path: PathBuf, hash: String) {
    if let Ok(mut cache) = CONTENT_HASH_CACHE.lock() {
        cache.entry(path).or_insert(hash);
    }
}

/// Like [`hash_video_file`], but memoized per path for the lifetime of the process.
pub fn hash_video_file_cached(path: &Path) -> std::io::Result<String> {
    let path_buf = path.to_path_buf();
    if let Ok(cache) = CONTENT_HASH_CACHE.lock()
        && let Some(hash) = cache.get(&path_buf)
    {
        return Ok(hash.clone());
    }
    let hash = hash_video_file(path)?;
    if let Ok(mut cache) = CONTENT_HASH_CACHE.lock() {
        cache.entry(path_buf).or_insert_with(|| hash.clone());
    }
    Ok(hash)
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
}
