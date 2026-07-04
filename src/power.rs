//! Keeps the OS from locking the screen or sleeping while video or slideshow
//! playback is active. Best-effort: if the platform mechanism isn't
//! available (e.g. no `systemd-inhibit` on a non-systemd Linux system),
//! playback still works — it's just not exempt from the normal idle timer.

use std::process::{Child, Command, Stdio};

pub struct WakeLock {
    active: bool,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    inhibitor: Option<Child>,
}

impl WakeLock {
    pub fn new() -> Self {
        Self {
            active: false,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            inhibitor: None,
        }
    }

    /// Engages/releases the platform sleep inhibitor. This is polled from a
    /// timer tick, so it only touches the OS on an actual true/false
    /// transition rather than on every call.
    pub fn set_active(&mut self, active: bool) {
        if active == self.active {
            return;
        }
        self.active = active;
        if active {
            self.engage();
        } else {
            self.release();
        }
    }

    // `-w <pid>` ties caffeinate's lifetime to ours, so a crash/force-kill
    // (which skips our `Drop` below) can't leave the machine permanently
    // unable to sleep.
    #[cfg(target_os = "macos")]
    fn engage(&mut self) {
        self.inhibitor = Command::new("caffeinate")
            .args(["-d", "-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    }

    #[cfg(target_os = "macos")]
    fn release(&mut self) {
        if let Some(mut child) = self.inhibitor.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // `systemd-inhibit` holds its lock for as long as the command it wraps
    // keeps running, so it's wrapped around a loop that polls our own pid —
    // same crash-safety reasoning as macOS's `-w` above, since there's no
    // direct equivalent flag here.
    #[cfg(target_os = "linux")]
    fn engage(&mut self) {
        let watch = format!(
            "while kill -0 {} 2>/dev/null; do sleep 5; done",
            std::process::id()
        );
        self.inhibitor = Command::new("systemd-inhibit")
            .args([
                "--what=idle:sleep",
                "--who=Flick",
                "--why=Media playback",
                "sh",
                "-c",
                &watch,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    }

    #[cfg(target_os = "linux")]
    fn release(&mut self) {
        if let Some(mut child) = self.inhibitor.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // Per-thread OS flag; Windows clears it automatically if the thread/process
    // dies, so no crash-safety workaround is needed here.
    #[cfg(target_os = "windows")]
    fn engage(&mut self) {
        unsafe {
            win32::SetThreadExecutionState(
                win32::ES_CONTINUOUS | win32::ES_SYSTEM_REQUIRED | win32::ES_DISPLAY_REQUIRED,
            );
        }
    }

    #[cfg(target_os = "windows")]
    fn release(&mut self) {
        unsafe {
            win32::SetThreadExecutionState(win32::ES_CONTINUOUS);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn engage(&mut self) {}

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn release(&mut self) {}
}

impl Drop for WakeLock {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    unsafe extern "system" {
        pub fn SetThreadExecutionState(flags: u32) -> u32;
    }
    pub const ES_CONTINUOUS: u32 = 0x8000_0000;
    pub const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    pub const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
}
