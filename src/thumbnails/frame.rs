use libmpv2::Mpv;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

// Headless extraction/probe cores are created concurrently from multiple
// background threads (see sprite.rs's bounded-concurrency batches). Mpv core
// creation itself isn't documented as safe to run fully concurrently from
// many threads at once (ffmpeg/libmpv have some one-time global init), so
// serialize just that step — decoding/encoding afterward still proceeds
// independently and in parallel.
static MPV_CREATE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
pub enum ExtractError {
    Mpv(String),
    Timeout,
    Io(std::io::Error),
    UnexpectedSize { expected: usize, got: usize },
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mpv(e) => write!(f, "mpv error: {e}"),
            Self::Timeout => write!(f, "timed out waiting for frame extraction"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::UnexpectedSize { expected, got } => {
                write!(f, "raw frame size mismatch: expected {expected} bytes, got {got}")
            }
        }
    }
}

/// One decoded, scaled+letterboxed video frame as raw RGBA bytes, exactly
/// `width * height * 4` long.
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Extracts a single frame from `input` at `timestamp_secs`, scaled to fit
/// within `width`x`height` and letterboxed (black bars) to fill it exactly —
/// same `scale=...force_original_aspect_ratio=decrease,pad=...` recipe the
/// Electron app's ffmpeg invocation used, but run through mpv's own built-in
/// encode mode (`--o`/`--ovc=rawvideo`) instead of shelling out to ffmpeg.
/// mpv links the same decode/filter libraries ffmpeg does, so this needs no
/// external dependency at all — same trick mpv's own community thumbnailer
/// scripts (thumbfast, mpv_thumbnail_script) use, just driven through libmpv
/// directly instead of spawning the `mpv` CLI binary.
pub fn extract_frame(input: &Path, timestamp_secs: f64, width: u32, height: u32) -> Result<RawFrame, ExtractError> {
    let out_path = std::env::temp_dir().join(format!(
        "flick-frame-{}-{}.raw",
        std::process::id(),
        timestamp_secs.to_bits()
    ));
    let _ = std::fs::remove_file(&out_path);

    let vf = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,format=rgba"
    );

    let mpv = {
        let _guard = MPV_CREATE_LOCK.lock().unwrap();
        Mpv::with_initializer(|init| {
            init.set_property("vo", "null")?;
            // This is a headless, throwaway core used purely for decoding —
            // it must never touch a real audio device, which could otherwise
            // contend with (or destabilize) the main playback instance's
            // audio output running concurrently on the UI thread.
            init.set_property("ao", "null")?;
            init.set_property("o", out_path.to_string_lossy().as_ref())?;
            init.set_property("ovc", "rawvideo")?;
            init.set_property("of", "rawvideo")?;
            init.set_property("vf", vf.as_str())?;
            init.set_property("start", timestamp_secs.to_string().as_str())?;
            init.set_property("frames", "1")?;
            Ok(())
        })
        .map_err(|e| ExtractError::Mpv(e.to_string()))?
    };

    let client = mpv.create_client(None).map_err(|e| ExtractError::Mpv(e.to_string()))?;
    let _ = client.disable_deprecated_events();

    mpv.command("loadfile", &[&input.to_string_lossy(), "replace"])
        .map_err(|e| ExtractError::Mpv(e.to_string()))?;

    // The muxer/encoder only flushes and closes the output file during the
    // core's shutdown sequence — reading the file right at EndFile raced the
    // flush and got 0 bytes. And unlike the `mpv` CLI frontend, driving
    // encode mode through libmpv directly doesn't auto-quit once the input
    // is done; we have to explicitly request it, then wait for Shutdown.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut quit_sent = false;
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(ExtractError::Timeout);
        }
        match client.wait_event(1.0) {
            Some(Ok(libmpv2::events::Event::EndFile(_))) if !quit_sent => {
                quit_sent = true;
                let _ = mpv.command("quit", &[]);
            }
            Some(Ok(libmpv2::events::Event::Shutdown)) => break,
            Some(Err(e)) => return Err(ExtractError::Mpv(e.to_string())),
            _ => continue,
        }
    }

    let bytes = std::fs::read(&out_path).map_err(ExtractError::Io)?;
    let _ = std::fs::remove_file(&out_path);

    let expected = (width as usize) * (height as usize) * 4;
    if bytes.len() != expected {
        return Err(ExtractError::UnexpectedSize { expected, got: bytes.len() });
    }

    Ok(RawFrame { width, height, rgba: bytes })
}

/// Probes a video's duration in seconds without ffprobe, via mpv's own
/// `duration` property.
pub fn probe_duration(input: &Path) -> Result<f64, ExtractError> {
    let mpv = {
        let _guard = MPV_CREATE_LOCK.lock().unwrap();
        Mpv::with_initializer(|init| {
            init.set_property("vo", "null")?;
            init.set_property("ao", "null")?;
            init.set_property("pause", true)?;
            Ok(())
        })
        .map_err(|e| ExtractError::Mpv(e.to_string()))?
    };

    let client = mpv.create_client(None).map_err(|e| ExtractError::Mpv(e.to_string()))?;
    let _ = client.disable_deprecated_events();

    mpv.command("loadfile", &[&input.to_string_lossy(), "replace"])
        .map_err(|e| ExtractError::Mpv(e.to_string()))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(ExtractError::Timeout);
        }
        match client.wait_event(1.0) {
            Some(Ok(libmpv2::events::Event::FileLoaded)) => break,
            Some(Ok(libmpv2::events::Event::EndFile(_))) => break,
            Some(Err(e)) => return Err(ExtractError::Mpv(e.to_string())),
            _ => continue,
        }
    }

    mpv.get_property::<f64>("duration").map_err(|e| ExtractError::Mpv(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spike test (Phase 3): proves the ffmpeg-free, libmpv-encode-mode frame
    /// extraction actually works, by extracting a real frame from the
    /// synthetic test video and writing it to a PNG for visual inspection.
    /// Requires /tmp/flick-test-media/test.mp4 (generated via ffmpeg testsrc
    /// during manual testing) — ignored by default since it's an
    /// environment-dependent spike, not a normal CI-safe unit test.
    #[test]
    #[ignore]
    fn spike_extract_frame_and_save_png() {
        let input = Path::new("/tmp/flick-test-media/test.mp4");
        assert!(input.exists(), "test fixture missing: {}", input.display());

        let duration = probe_duration(input).expect("probe_duration failed");
        eprintln!("probed duration: {duration}s");
        assert!(duration > 0.0);

        let frame = extract_frame(input, 5.0, 160, 90).expect("extract_frame failed");
        assert_eq!(frame.rgba.len(), 160 * 90 * 4);

        let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            .expect("buffer size matches dimensions");
        img.save("/tmp/flick-frame-spike.png").expect("failed to save spike PNG");
        eprintln!("wrote /tmp/flick-frame-spike.png");
    }
}
