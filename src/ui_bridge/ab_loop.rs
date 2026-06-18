use super::state::{AbLoopState, AppState};
use crate::AppWindow;
use libmpv2::Mpv;

/// Pushes the current A-B loop state to the AppWindow's display properties.
fn sync_ab_loop_ui(app: &AppWindow, state: &AppState) {
    let (a, b, picking, active) = match state.ab_loop {
        AbLoopState::Off => (-1.0, -1.0, false, false),
        AbLoopState::PickingA => (-1.0, -1.0, true, false),
        AbLoopState::PickingB { a } => (a, -1.0, true, false),
        AbLoopState::Active { a, b } => (a, b, false, true),
    };
    app.set_ab_loop_a(a as f32);
    app.set_ab_loop_b(b as f32);
    app.set_ab_loop_picking(picking);
    app.set_ab_loop_active(active);
}

/// Port of `toggleAbLoopMode`: any non-Off state (picking or active) clears
/// back to Off; Off starts picking. mpv enforces the loop natively once both
/// properties are set, so unlike the original there's no manual clamping to port.
pub fn toggle_ab_loop(mpv: &Mpv, app: &AppWindow, state: &mut AppState) {
    state.ab_loop = match state.ab_loop {
        AbLoopState::Off => AbLoopState::PickingA,
        _ => {
            let _ = mpv.set_property("ab-loop-a", "no");
            let _ = mpv.set_property("ab-loop-b", "no");
            AbLoopState::Off
        }
    };
    sync_ab_loop_ui(app, state);
}

/// Routes a progress-bar click: registers an A/B point while picking (port of
/// `registerAbPointFromClientX`), otherwise seeks (port of `onProgressDown`'s
/// plain-seek branch — drag-scrubbing is deferred, click-to-seek only for now).
pub fn handle_progress_click(mpv: &Mpv, app: &AppWindow, state: &mut AppState, ratio: f32) {
    let duration = mpv.get_property::<f64>("duration").unwrap_or(0.0);
    if duration <= 0.0 {
        return;
    }
    let t = (ratio as f64).clamp(0.0, 1.0) * duration;

    match state.ab_loop {
        AbLoopState::PickingA => {
            let _ = mpv.set_property("ab-loop-a", t);
            state.ab_loop = AbLoopState::PickingB { a: t };
            sync_ab_loop_ui(app, state);
        }
        AbLoopState::PickingB { a } => {
            if t <= a {
                return; // end must be after start, matches the original's guard
            }
            let _ = mpv.set_property("ab-loop-b", t);
            let _ = mpv.command("seek", &[&a.to_string(), "absolute"]);
            state.ab_loop = AbLoopState::Active { a, b: t };
            sync_ab_loop_ui(app, state);
        }
        AbLoopState::Off | AbLoopState::Active { .. } => {
            let _ = mpv.command("seek", &[&t.to_string(), "absolute"]);
        }
    }
}
