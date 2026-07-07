//! Keeps the OS from locking the screen or sleeping while video or slideshow
//! playback is active. Best-effort: if no platform mechanism is available,
//! playback still works — it's just not exempt from the normal idle timer.

use std::process::{Child, Command, Stdio};

pub struct WakeLock {
    active: bool,
    #[cfg(target_os = "macos")]
    inhibitor: Option<Child>,
    #[cfg(target_os = "linux")]
    inhibitor: Option<LinuxInhibitor>,
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

    // What actually stops the screen from locking/blanking on Linux is the
    // desktop's own screensaver daemon (gnome-shell, kwin, xfce4-power-manager,
    // ...), which all speak the same `org.freedesktop.ScreenSaver` D-Bus
    // interface — the same one VLC/browsers use, and it works whether or not
    // the session even involves systemd. `systemd-inhibit` only affects
    // logind's own idle/suspend handling, which most desktop environments
    // don't actually consult for screen-lock — it was necessary but not
    // sufficient, which is why the first cut of this "worked" (a process
    // existed) without actually keeping the screen unlocked. Try D-Bus first
    // and only fall back to the logind wrapper (still useful for suspend, and
    // for minimal window managers with no screensaver service) if that's
    // unavailable.
    #[cfg(target_os = "linux")]
    fn engage(&mut self) {
        if let Some(cookie) = Self::dbus_inhibit() {
            self.inhibitor = Some(LinuxInhibitor::Dbus(cookie));
            return;
        }
        // Fallback: wrap a loop that polls our own pid, so the lock can't
        // outlive us even if `Drop` gets skipped (crash/force-kill) — same
        // crash-safety reasoning as macOS's `-w` above, since `systemd-inhibit`
        // has no equivalent flag.
        let watch = format!(
            "while kill -0 {} 2>/dev/null; do sleep 5; done",
            std::process::id()
        );
        if let Ok(child) = Command::new("systemd-inhibit")
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
        {
            self.inhibitor = Some(LinuxInhibitor::Process(child));
        }
    }

    #[cfg(target_os = "linux")]
    fn release(&mut self) {
        match self.inhibitor.take() {
            Some(LinuxInhibitor::Dbus(cookie)) => Self::dbus_uninhibit(cookie),
            Some(LinuxInhibitor::Process(mut child)) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            None => {}
        }
    }

    /// Calls `ScreenSaver.Inhibit`, returning the cookie it hands back (needed
    /// to release the same lock later) — `None` if `dbus-send` is missing, the
    /// session bus has no screensaver service, or the reply couldn't be
    /// parsed.
    #[cfg(target_os = "linux")]
    fn dbus_inhibit() -> Option<u32> {
        let output = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.ScreenSaver",
                "/org/freedesktop/ScreenSaver",
                "org.freedesktop.ScreenSaver.Inhibit",
                "string:Flick",
                "string:Media playback",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.trim().strip_prefix("uint32 "))
            .and_then(|cookie| cookie.trim().parse().ok())
    }

    #[cfg(target_os = "linux")]
    fn dbus_uninhibit(cookie: u32) {
        let _ = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.ScreenSaver",
                "/org/freedesktop/ScreenSaver",
                "org.freedesktop.ScreenSaver.UnInhibit",
                &format!("uint32:{cookie}"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
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

#[cfg(target_os = "linux")]
enum LinuxInhibitor {
    /// `org.freedesktop.ScreenSaver.Inhibit` cookie, released via `UnInhibit`.
    Dbus(u32),
    /// `systemd-inhibit` fallback child process, released by killing it.
    Process(Child),
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
