use std::sync::OnceLock;

/// True when `FLICK_DEBUG=1` (checked once at first use).
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FLICK_DEBUG").as_deref() == Ok("1"))
}

/// `eprintln!` gated on [`enabled`].
#[macro_export]
macro_rules! flick_debug {
    ($($arg:tt)*) => {{
        if $crate::debug::enabled() {
            eprintln!($($arg)*);
        }
    }};
}
