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

/// Routes a progress-bar click or drag-release: registers an A/B point while
/// picking (port of `registerAbPointFromClientX`), otherwise seeks (port of
/// `onProgressDown`'s plain-seek branch). This is the single authoritative,
/// frame-accurate seek per press/release cycle — continuous drag ticks go
/// through `queue_scrub_ratio`/`apply_pending_scrub` instead, which don't
/// block the UI thread on every pointer-move event.
pub fn handle_progress_click(mpv: &Mpv, app: &AppWindow, state: &mut AppState, ratio: f32) {
    let duration = app.get_duration() as f64;
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
            let _ = mpv.command("seek", &[&a.to_string(), "absolute+exact"]);
            state.ab_loop = AbLoopState::Active { a, b: t };
            sync_ab_loop_ui(app, state);
        }
        AbLoopState::Off | AbLoopState::Active { .. } => {
            let _ = mpv.command("seek", &[&t.to_string(), "absolute+exact"]);
        }
    }
}

/// Records the latest drag-scrub ratio and immediately tries to apply it
/// (see `apply_pending_scrub`) — most drag ticks land here with mpv already
/// idle, so a fast, exact seek fires right away instead of waiting on a
/// fixed polling interval.
pub fn queue_scrub_ratio(mpv: &Mpv, app: &AppWindow, state: &mut AppState, ratio: f32) {
    state.pending_scrub_ratio = Some(ratio);
    apply_pending_scrub(mpv, app, state);
}

/// Applies the latest queued drag-scrub ratio as a full-precision
/// (`absolute+exact`) seek — matching the sprite-preview thumbnail exactly
/// — but only if mpv isn't already mid-seek. Gating on mpv's own `seeking`
/// flag (a cheap property read) rather than a fixed interval means
/// responsiveness self-paces to how fast *this* video can actually be
/// seeked: a small/local file gets a fresh seek on nearly every drag tick,
/// while a slow one (huge file, network mount, sparse keyframes) naturally
/// throttles to its own completion time instead of piling up backlogged
/// seek commands — which is what caused the original hang (an unthrottled
/// exact seek issued on every raw pointer-move event).
///
/// Called both right after a new ratio is queued (so dragging feels
/// immediate) and once per drain-timer tick as a backstop, so a ratio
/// queued while mpv was mid-seek still gets serviced once mpv frees up even
/// if the mouse pauses momentarily mid-drag.
pub fn apply_pending_scrub(mpv: &Mpv, app: &AppWindow, state: &mut AppState) {
    if state.pending_scrub_ratio.is_none() {
        return;
    }
    if mpv.get_property::<bool>("seeking").unwrap_or(false) {
        return;
    }
    let Some(ratio) = state.pending_scrub_ratio.take() else {
        return;
    };
    if matches!(
        state.ab_loop,
        AbLoopState::PickingA | AbLoopState::PickingB { .. }
    ) {
        return; // preserve existing semantics: no continuous seeking while picking A/B points
    }
    let duration = app.get_duration() as f64;
    if duration <= 0.0 {
        return;
    }
    let t = (ratio as f64).clamp(0.0, 1.0) * duration;
    let _ = mpv.command("seek", &[&t.to_string(), "absolute+exact"]);
}
