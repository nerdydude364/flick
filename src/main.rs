slint::include_modules!();

mod dialogs;
mod import;
mod library;
mod playlist;
mod reveal;
mod thumbnails;
mod ui_bridge;

use glow::HasContext;
use libmpv2::{
    Mpv,
    render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType},
};
use slint::VecModel;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_void};
use std::path::PathBuf;
use std::rc::Rc;
use ui_bridge::AppState;

// Slint hands us `get_proc_address` as `&'a dyn Fn(...)`, scoped to the single
// RenderingSetup call. libmpv2's `OpenGLInitParams<GLContext: 'static>` requires
// a 'static *type* (not a 'static *value*) for the context we pass alongside our
// own `fn` pointer, so we erase the borrow into a raw fat pointer here. This is
// only valid to dereference synchronously, inside the same RenderingSetup call
// where mpv resolves its GL function pointers — which is exactly how it's used
// below, immediately inside `Mpv::create_render_context`.
struct ProcAddrCtx(*const dyn Fn(&CStr) -> *const c_void);

fn mpv_get_proc_address(ctx: &ProcAddrCtx, name: &str) -> *mut c_void {
    let Ok(cname) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    let f: &dyn Fn(&CStr) -> *const c_void = unsafe { &*ctx.0 };
    f(&cname) as *mut c_void
}

/// Owns the mpv render context plus a keep-alive handle to the `Mpv` core it
/// borrows from. `RenderContext<'a>` is tied to `&'a Mpv`; we manufacture a
/// `'static` lifetime via `transmute` because the borrowed `Mpv` actually lives
/// in the `Rc`'s stable heap allocation, not in a stack frame that could move out
/// from under it. Soundness depends on `render_context` being dropped before
/// `_mpv_keep_alive` — guaranteed here because Rust drops struct fields in
/// declaration order.
struct MpvUnderlay {
    render_context: RenderContext<'static>,
    _mpv_keep_alive: Rc<Mpv>,
    gl: glow::Context,
}

impl MpvUnderlay {
    fn new(
        get_proc_address: &dyn Fn(&CStr) -> *const c_void,
        mpv: Rc<Mpv>,
        app_weak: slint::Weak<AppWindow>,
    ) -> Self {
        let gl = unsafe { glow::Context::from_loader_function_cstr(|s| get_proc_address(s)) };
        // SAFETY: `transmute` (not an `as` cast) is required here because the
        // source reference's lifetime is scoped to this RenderingSetup call,
        // shorter than the `'static` bound `*const dyn Trait` implies by default.
        // We only ever dereference this pointer synchronously, inside
        // `Mpv::create_render_context` below, while the borrow is still actually
        // valid — see the struct doc comment for the rest of the soundness case.
        let proc_addr_ctx = ProcAddrCtx(unsafe {
            std::mem::transmute::<
                &dyn Fn(&CStr) -> *const c_void,
                *const dyn Fn(&CStr) -> *const c_void,
            >(get_proc_address)
        });
        let mut render_context = mpv
            .create_render_context(vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address: mpv_get_proc_address,
                    ctx: proc_addr_ctx,
                }),
            ])
            .expect("failed to create mpv render context");

        // Fires on an arbitrary mpv-internal thread whenever a new decoded frame
        // is ready (or the render target otherwise needs a redraw). We can't
        // touch Slint/window state here, so hop to the UI thread and request a
        // repaint, which re-invokes BeforeRendering -> MpvUnderlay::render below.
        render_context.set_update_callback(move || {
            let app_weak = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.window().request_redraw();
                }
            });
        });

        // SAFETY: see struct doc comment above.
        let render_context: RenderContext<'static> = unsafe { std::mem::transmute(render_context) };

        Self {
            render_context,
            _mpv_keep_alive: mpv,
            gl,
        }
    }

    fn render(&self, width: i32, height: i32) {
        // Query the FBO Slint actually has bound right now rather than assuming
        // 0 (the default framebuffer) — on some platforms/renderers the UI may
        // be composited via a non-zero intermediate FBO, in which case mpv must
        // target that same FBO or its output never reaches the screen.
        let fbo = unsafe { self.gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING) };

        if let Err(err) = self.render_context.render::<()>(fbo, width, height, true) {
            eprintln!("mpv render error: {err}");
        }
    }
}

/// (Re)starts the slideshow's repeating timer at `duration_secs`. Stops
/// itself (and flips the UI toggle back off) once `navigate_image_relative`
/// reports it couldn't advance further — port of `startSlideshow`'s
/// not-looping-and-reached-the-end branch.
fn start_slideshow_timer(
    mpv: &Rc<Mpv>,
    slideshow_timer: &Rc<slint::Timer>,
    state: &Rc<RefCell<AppState>>,
    model: &Rc<VecModel<PlaylistItemData>>,
    app_weak: &slint::Weak<AppWindow>,
    duration_secs: f64,
) {
    let mpv = Rc::clone(mpv);
    let state = Rc::clone(state);
    let model = Rc::clone(model);
    let app_weak = app_weak.clone();
    slideshow_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs_f64(duration_secs.max(0.1)),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut state_ref = state.borrow_mut();
            if !state_ref.slideshow_on {
                return;
            }
            let advanced = match state_ref.mode {
                ui_bridge::Mode::Image => {
                    ui_bridge::navigate_image_relative(&mpv, &app, &mut state_ref, &model, 1)
                }
                ui_bridge::Mode::All => {
                    if !ui_bridge::all_slideshow_wants_timer(&state_ref) {
                        return;
                    }
                    ui_bridge::navigate_all_relative(&mpv, &app, &mut state_ref, &model, 1)
                }
                ui_bridge::Mode::Video => false,
            };
            if !advanced {
                state_ref.slideshow_on = false;
                app.set_slideshow_on(false);
            }
        },
    );
}

/// Defers a heavy gallery thumbnail rebuild until after Slint paints the grid
/// shell with the loading overlay (see `open_gallery_grid`).
fn schedule_pending_gallery_reload(
    app_weak: slint::Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    gallery_model: Rc<VecModel<slint::Image>>,
    gallery_video_flags: Rc<VecModel<bool>>,
    gallery_failed_flags: Rc<VecModel<bool>>,
    gallery_tx: std::sync::mpsc::Sender<ui_bridge::GalleryThumbResult>,
) {
    if !state.borrow().pending_gallery_reload {
        return;
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(0), move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        ui_bridge::run_pending_gallery_reload(
            &mut state.borrow_mut(),
            &app,
            &ui_bridge::GalleryContext {
                thumbnails: &gallery_model,
                video_flags: &gallery_video_flags,
                failed_flags: &gallery_failed_flags,
                tx: &gallery_tx,
            },
        );
    });
}

/// Creates the mpv-backed OpenGL underlay and wires it to Slint's rendering
/// lifecycle (setup/render/teardown), plus the one-time initial autoplay this
/// triggers once the render context exists — see the `RenderingSetup` arm for
/// why loading a file has to wait until then.
struct ImportWiring {
    scan_tx: std::sync::mpsc::Sender<Vec<library::ScannedFile>>,
    file_import_tx: std::sync::mpsc::Sender<import::FileImportBatch>,
    startup_paths: Rc<RefCell<Vec<PathBuf>>>,
    gallery_model: Rc<VecModel<slint::Image>>,
    gallery_video_flags: Rc<VecModel<bool>>,
    gallery_failed_flags: Rc<VecModel<bool>>,
    gallery_tx: std::sync::mpsc::Sender<ui_bridge::GalleryThumbResult>,
}

/// Shared handles passed into UI callback wiring helpers.
struct AppContext {
    mpv: Rc<Mpv>,
    state: Rc<RefCell<AppState>>,
    model: Rc<VecModel<PlaylistItemData>>,
    scan_tx: std::sync::mpsc::Sender<Vec<library::ScannedFile>>,
    file_import_tx: std::sync::mpsc::Sender<import::FileImportBatch>,
    gallery_model: Rc<VecModel<slint::Image>>,
    gallery_video_flags: Rc<VecModel<bool>>,
    gallery_failed_flags: Rc<VecModel<bool>>,
    gallery_tx: std::sync::mpsc::Sender<ui_bridge::GalleryThumbResult>,
}

fn wire_video_underlay(
    app: &AppWindow,
    mpv: &Rc<Mpv>,
    state: &Rc<RefCell<AppState>>,
    model: &Rc<VecModel<PlaylistItemData>>,
    import_wiring: ImportWiring,
) {
    let mut underlay: Option<MpvUnderlay> = None;
    let app_weak = app.as_weak();

    let mpv = Rc::clone(mpv);
    let state = Rc::clone(state);
    let model = Rc::clone(model);
    let scan_tx = import_wiring.scan_tx.clone();
    let file_import_tx = import_wiring.file_import_tx.clone();
    let startup_paths = Rc::clone(&import_wiring.startup_paths);
    let gallery_model = Rc::clone(&import_wiring.gallery_model);
    let gallery_video_flags = Rc::clone(&import_wiring.gallery_video_flags);
    let gallery_failed_flags = Rc::clone(&import_wiring.gallery_failed_flags);
    let gallery_tx = import_wiring.gallery_tx.clone();
    app.window()
        .set_rendering_notifier(move |rendering_state, graphics_api| match rendering_state {
            slint::RenderingState::RenderingSetup => {
                let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = graphics_api else {
                    panic!(
                        "flick's native rewrite requires Slint's OpenGL renderer (femtovg backend)"
                    );
                };
                underlay = Some(MpvUnderlay::new(
                    get_proc_address,
                    Rc::clone(&mpv),
                    app_weak.clone(),
                ));
                // Must load the file only *after* the render context exists —
                // loading earlier means mpv tries to init `vo=libmpv` with no
                // render context attached yet, fails ("No render context set"),
                // and falls back to audio-only with no retry. This was the
                // actual cause of an earlier "black video, audio plays fine" bug.
                if let Some(app) = app_weak.upgrade() {
                    let paths = std::mem::take(&mut *startup_paths.borrow_mut());
                    if !paths.is_empty() {
                        let gallery = ui_bridge::GalleryContext {
                            thumbnails: &gallery_model,
                            video_flags: &gallery_video_flags,
                            failed_flags: &gallery_failed_flags,
                            tx: &gallery_tx,
                        };
                        import::import_paths(
                            paths,
                            &import::ImportContext {
                                app: &app,
                                mpv: &mpv,
                                state: &state,
                                model: &model,
                                scan_tx: &scan_tx,
                                file_import_tx: &file_import_tx,
                                gallery,
                            },
                        );
                    }
                }
            }
            slint::RenderingState::BeforeRendering => {
                if let (Some(underlay), Some(app)) = (underlay.as_ref(), app_weak.upgrade()) {
                    // The video underlay is fully hidden behind the opaque
                    // image viewer in image mode (Phase 5) — rendering it
                    // and force-requesting another redraw every frame was
                    // wasted GPU/CPU work in that case (noticeably
                    // sluggish with the image viewer up). Skip both; mpv's
                    // own update callback still wakes the loop if video
                    // genuinely has a new frame, so switching back to
                    // video mode doesn't need anything special to resume.
                    if (app.get_view_mode() == 0 && app.get_gallery_open())
                        || (app.get_view_mode() == 2
                            && app.get_current_is_video()
                            && app.get_gallery_open())
                    {
                        let size = app.window().size();
                        underlay.render(size.width as i32, size.height as i32);
                        // mpv's render API needs to be driven continuously
                        // (effectively once per vsync), not just
                        // reactively from its update callback — without
                        // this, Direct Rendering buffer negotiation for
                        // frames after the first one stalls and video
                        // freezes on frame 1 while audio keeps playing.
                        app.window().request_redraw();
                    }
                }
            }
            slint::RenderingState::RenderingTeardown => {
                underlay.take();
            }
            _ => {}
        })
        .expect(
            "Unable to set rendering notifier (does this Slint backend support OpenGL underlays?)",
        );
}

/// Wires the bottom transport bar's mpv-backed controls: play/pause, seeking,
/// volume, fullscreen, A-B loop, speed cycling, screenshots, subtitles, and
/// the loop toggle (loop itself is just a flag `ui_bridge`'s prev/next reads).
fn wire_playback_controls(app: &AppWindow, mpv: &Rc<Mpv>, state: &Rc<RefCell<AppState>>) {
    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let app_weak = app.as_weak();
        app.on_toggle_play(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow().mode == ui_bridge::Mode::Image {
                return;
            }
            let now_playing = !app.get_playing();
            if !ui_bridge::log_mpv_err("toggle play", mpv.set_property("pause", !now_playing)) {
                return;
            }
            app.set_playing(now_playing);
        });
    }

    {
        let mpv = Rc::clone(mpv);
        app.on_seek_relative(move |seconds| {
            ui_bridge::log_mpv_err(
                "seek",
                mpv.command("seek", &[&seconds.to_string(), "relative"]),
            );
        });
    }

    {
        let mpv = Rc::clone(mpv);
        app.on_volume_changed(move |volume| {
            // mpv's native volume range is 0-100, matching the Slider range in app-window.slint.
            ui_bridge::log_mpv_err("volume change", mpv.set_property("volume", volume as f64));
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                let window = app.window();
                window.set_fullscreen(!window.is_fullscreen());
                app.set_is_fullscreen(window.is_fullscreen());
            }
        });
    }

    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let app_weak = app.as_weak();
        app.on_seek_to_ratio(move |ratio| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::handle_progress_click(&mpv, &app, &mut state.borrow_mut(), ratio);
        });
    }

    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let app_weak = app.as_weak();
        app.on_toggle_ab_loop(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::toggle_ab_loop(&mpv, &app, &mut state.borrow_mut());
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_dismiss_error(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_error_message("".into());
            }
        });
    }

    {
        let mpv = Rc::clone(mpv);
        // Cycles through preset speeds — simpler than a free-form slider for
        // a feature that didn't exist before (FEATURES.md TODO), and mirrors
        // how most players expose a quick speed toggle.
        let speeds: &[f64] = &[0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
        let speed_idx = Rc::new(RefCell::new(2usize)); // start at 1.0x
        let app_weak = app.as_weak();
        app.on_cycle_speed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut idx = speed_idx.borrow_mut();
            *idx = (*idx + 1) % speeds.len();
            let speed = speeds[*idx];
            if !ui_bridge::log_mpv_err("speed change", mpv.set_property("speed", speed)) {
                return;
            }
            let text = if speed == speed.trunc() {
                format!("{speed:.0}x")
            } else {
                format!("{speed}x")
            };
            app.set_playback_speed_text(text.into());
        });
    }

    {
        let mpv = Rc::clone(mpv);
        let app_weak = app.as_weak();
        app.on_take_screenshot(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let dir = dirs::picture_dir().unwrap_or_else(std::env::temp_dir);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let path = dir.join(format!("Flick-Screenshot-{timestamp}.png"));
            match mpv.command("screenshot-to-file", &[&path.to_string_lossy(), "video"]) {
                Ok(_) => ui_bridge::show_toast(
                    &app,
                    format!("Screenshot saved to {}", path.display()),
                    false,
                ),
                Err(err) => {
                    eprintln!("screenshot failed: {err}");
                    ui_bridge::show_toast(&app, format!("Screenshot failed: {err}"), true);
                }
            }
        });
    }

    {
        let mpv = Rc::clone(mpv);
        app.on_add_subtitle(move || {
            let Some(path) = dialogs::open_subtitle_file() else {
                return;
            };
            ui_bridge::log_mpv_err(
                "add subtitle",
                mpv.command("sub-add", &[&path.to_string_lossy(), "select"]),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let state = Rc::clone(state);
        app.on_toggle_loop(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            state.loop_on = !state.loop_on;
            app.set_loop_on(state.loop_on);
        });
    }
}

/// Wires sidebar/library-management callbacks shared by both the video and
/// image queues: reveal-in-file-manager, remove, drag-reorder, opening new
/// files, switching between video/image mode, and clearing the active queue.
fn wire_queue_management(app: &AppWindow, ctx: &AppContext) {
    {
        let state = Rc::clone(&ctx.state);
        app.on_reveal_item(move |queue_index| {
            let state = state.borrow();
            let item = state.active_queue().item(queue_index as usize);
            if let Some(item) = item {
                reveal::reveal_in_file_manager(&item.path);
            }
        });
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let app_weak = app.as_weak();
        app.on_remove_item(move |queue_index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::remove_item(
                &mpv,
                &app,
                &mut state.borrow_mut(),
                &model,
                queue_index as usize,
            );
        });
    }

    {
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        app.on_reorder_item(move |src, dst| {
            ui_bridge::reorder_item(&mut state.borrow_mut(), &model, src as usize, dst as usize);
        });
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let app_weak = app.as_weak();
        let scan_tx = ctx.scan_tx.clone();
        let file_import_tx = ctx.file_import_tx.clone();
        let gallery_model = Rc::clone(&ctx.gallery_model);
        let gallery_video_flags = Rc::clone(&ctx.gallery_video_flags);
        let gallery_failed_flags = Rc::clone(&ctx.gallery_failed_flags);
        let gallery_tx = ctx.gallery_tx.clone();
        let open_media: Rc<dyn Fn()> = Rc::new(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(picked) = dialogs::open_media_files() else {
                return;
            };
            import::import_paths(
                picked,
                &import::ImportContext {
                    app: &app,
                    mpv: &mpv,
                    state: &state,
                    model: &model,
                    scan_tx: &scan_tx,
                    file_import_tx: &file_import_tx,
                    gallery: ui_bridge::GalleryContext {
                        thumbnails: &gallery_model,
                        video_flags: &gallery_video_flags,
                        failed_flags: &gallery_failed_flags,
                        tx: &gallery_tx,
                    },
                },
            );
        });
        app.on_open_videos({
            let open_media = Rc::clone(&open_media);
            move || open_media()
        });
        app.on_open_images(move || open_media());
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let app_weak = app.as_weak();
        let gallery_model = Rc::clone(&ctx.gallery_model);
        let gallery_video_flags = Rc::clone(&ctx.gallery_video_flags);
        let gallery_failed_flags = Rc::clone(&ctx.gallery_failed_flags);
        let gallery_tx = ctx.gallery_tx.clone();
        app.on_set_view_mode(move |mode| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let target = match mode {
                0 => ui_bridge::Mode::Video,
                1 => ui_bridge::Mode::Image,
                _ => ui_bridge::Mode::All,
            };
            let gallery = ui_bridge::GalleryContext {
                thumbnails: &gallery_model,
                video_flags: &gallery_video_flags,
                failed_flags: &gallery_failed_flags,
                tx: &gallery_tx,
            };
            ui_bridge::set_mode(
                &mpv,
                &app,
                &mut state.borrow_mut(),
                &model,
                target,
                Some(&gallery),
            );
            schedule_pending_gallery_reload(
                app_weak.clone(),
                Rc::clone(&state),
                Rc::clone(&gallery_model),
                Rc::clone(&gallery_video_flags),
                Rc::clone(&gallery_failed_flags),
                gallery_tx.clone(),
            );
        });
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let app_weak = app.as_weak();
        let gallery_model = Rc::clone(&ctx.gallery_model);
        let gallery_video_flags = Rc::clone(&ctx.gallery_video_flags);
        let gallery_failed_flags = Rc::clone(&ctx.gallery_failed_flags);
        let gallery_tx = ctx.gallery_tx.clone();
        app.on_clear_queue(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::clear_library(
                &mpv,
                &app,
                &mut state.borrow_mut(),
                &model,
                &ui_bridge::GalleryContext {
                    thumbnails: &gallery_model,
                    video_flags: &gallery_video_flags,
                    failed_flags: &gallery_failed_flags,
                    tx: &gallery_tx,
                },
            );
        });
    }
}

/// Wires image-mode-only callbacks: gallery prev/next, slideshow start/stop
/// and its duration slider, and the polling timer that drives animated-GIF
/// playback (see `tick_gif_animation`'s doc comment for why it's a poll).
fn wire_image_viewer(
    app: &AppWindow,
    ctx: &AppContext,
    slideshow_timer: &Rc<slint::Timer>,
    sprite_timer: &Rc<slint::Timer>,
    sprite_tx: &std::sync::mpsc::Sender<(String, bool)>,
) {
    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let app_weak = app.as_weak();
        app.on_navigate_image(move |delta| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            match state.mode {
                ui_bridge::Mode::Image => {
                    ui_bridge::navigate_image_relative(&mpv, &app, &mut state, &model, delta);
                }
                ui_bridge::Mode::All => {
                    ui_bridge::navigate_all_relative(&mpv, &app, &mut state, &model, delta);
                }
                ui_bridge::Mode::Video => {}
            }
        });
    }

    // Slideshow: a single persistent Repeated timer, (re)started at the
    // configured interval whenever slideshow is turned on or the duration
    // slider changes, stopped when turned off (including by the gallery
    // toggle below). Auto-navigate stops itself (turns the toggle back off)
    // once it can't advance further — port of the original's "reached the
    // end, not looping" branch in `startSlideshow`.
    let slideshow_timer = Rc::clone(slideshow_timer);

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let gallery_model = Rc::clone(&ctx.gallery_model);
        let gallery_tx = ctx.gallery_tx.clone();
        let slideshow_timer = Rc::clone(&slideshow_timer);
        let gallery_video_flags = Rc::clone(&ctx.gallery_video_flags);
        let gallery_failed_flags = Rc::clone(&ctx.gallery_failed_flags);
        let app_weak = app.as_weak();
        app.on_toggle_gallery(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let gallery = ui_bridge::GalleryContext {
                thumbnails: &gallery_model,
                video_flags: &gallery_video_flags,
                failed_flags: &gallery_failed_flags,
                tx: &gallery_tx,
            };
            ui_bridge::toggle_gallery(&mpv, &app, &mut state.borrow_mut(), &gallery);
            ui_bridge::sync_active_view_ui(&app, &mut state.borrow_mut());
            schedule_pending_gallery_reload(
                app_weak.clone(),
                Rc::clone(&state),
                Rc::clone(&gallery_model),
                Rc::clone(&gallery_video_flags),
                Rc::clone(&gallery_failed_flags),
                gallery_tx.clone(),
            );
            if !app.get_slideshow_on() {
                slideshow_timer.stop();
            }
        });
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let app_weak = app.as_weak();
        let sprite_timer = Rc::clone(sprite_timer);
        let sprite_tx = sprite_tx.clone();
        app.on_gallery_item_clicked(move |pos| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (schedule_sprites, queue_idx) = {
                let mut state = state.borrow_mut();
                let Some(&queue_idx) = state.gallery_order.get(pos as usize) else {
                    return;
                };
                match state.mode {
                    ui_bridge::Mode::Image => {
                        ui_bridge::show_image_at(&mpv, &app, &mut state, &model, queue_idx);
                        (false, queue_idx)
                    }
                    ui_bridge::Mode::All => {
                        ui_bridge::present_item(&mpv, &app, &mut state, &model, queue_idx);
                        (state.all_current_is_video, queue_idx)
                    }
                    ui_bridge::Mode::Video => {
                        ui_bridge::play_index(&mpv, &app, &mut state, &model, queue_idx);
                        (true, queue_idx)
                    }
                }
            };
            if schedule_sprites {
                ui_bridge::schedule_sprite_generation(
                    app_weak.clone(),
                    &state,
                    &model,
                    &sprite_timer,
                    sprite_tx.clone(),
                    queue_idx,
                );
            }
        });
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let slideshow_timer = Rc::clone(&slideshow_timer);
        let app_weak = app.as_weak();
        app.on_toggle_slideshow(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::toggle_slideshow(&mpv, &app, &mut state.borrow_mut(), &model);
            if app.get_slideshow_on() {
                let duration = state.borrow().slideshow_duration;
                start_slideshow_timer(&mpv, &slideshow_timer, &state, &model, &app_weak, duration);
            } else {
                slideshow_timer.stop();
            }
        });
    }

    {
        let mpv = Rc::clone(&ctx.mpv);
        let state = Rc::clone(&ctx.state);
        let model = Rc::clone(&ctx.model);
        let slideshow_timer = Rc::clone(&slideshow_timer);
        let app_weak = app.as_weak();
        app.on_slideshow_duration_changed(move |seconds| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::set_slideshow_duration(&app, &mut state.borrow_mut(), seconds as f64);
            if app.get_slideshow_on() {
                let duration = state.borrow().slideshow_duration;
                start_slideshow_timer(&mpv, &slideshow_timer, &state, &model, &app_weak, duration);
            }
        });
    }

    // Animated GIF playback: polls (rather than precisely self-scheduling)
    // so that showing a new image doesn't need to thread a Timer handle
    // through every ui_bridge function that can change the displayed image —
    // see `tick_gif_animation`'s doc comment.
    let gif_timer = slint::Timer::default();
    {
        let state = Rc::clone(&ctx.state);
        let app_weak = app.as_weak();
        gif_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(33),
            move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                ui_bridge::tick_gif_animation(&app, &mut state.borrow_mut());
            },
        );
    }
    std::mem::forget(gif_timer);
}

/// Wires the "open folder" button through the shared import pipeline so
/// merge-import, loading UI, and gallery refresh behave like drag-and-drop.
fn wire_folder_scan(app: &AppWindow, ctx: &AppContext) {
    let app_weak = app.as_weak();
    let mpv = Rc::clone(&ctx.mpv);
    let state = Rc::clone(&ctx.state);
    let model = Rc::clone(&ctx.model);
    let scan_tx = ctx.scan_tx.clone();
    let file_import_tx = ctx.file_import_tx.clone();
    let gallery_model = Rc::clone(&ctx.gallery_model);
    let gallery_video_flags = Rc::clone(&ctx.gallery_video_flags);
    let gallery_failed_flags = Rc::clone(&ctx.gallery_failed_flags);
    let gallery_tx = ctx.gallery_tx.clone();
    app.on_open_folder(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        let Some(folder) = dialogs::open_folder() else {
            return;
        };
        import::import_paths(
            vec![folder],
            &import::ImportContext {
                app: &app,
                mpv: &mpv,
                state: &state,
                model: &model,
                scan_tx: &scan_tx,
                file_import_tx: &file_import_tx,
                gallery: ui_bridge::GalleryContext {
                    thumbnails: &gallery_model,
                    video_flags: &gallery_video_flags,
                    failed_flags: &gallery_failed_flags,
                    tx: &gallery_tx,
                },
            },
        );
    });
}

/// Syncs the OS cursor with chrome auto-hide. Slint's TouchArea `mouse-cursor`
/// only updates on pointer events; winit hides immediately when chrome fades.
fn wire_chrome_cursor_hiding(app: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    let app_weak = app.as_weak();
    app.on_chrome_visibility_changed(move |chrome_visible| {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        app.window().with_winit_window(|winit_window| {
            winit_window.set_cursor_visible(chrome_visible);
        });
    });
}

/// Accepts OS file/folder drops onto the window via winit and forwards each
/// path to `drop_tx` for batched import on the UI thread.
fn wire_file_drop(app: &AppWindow, drop_tx: std::sync::mpsc::Sender<PathBuf>) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
    app.window().on_winit_window_event(move |_, event| {
        if let winit::event::WindowEvent::DroppedFile(path) = event {
            let _ = drop_tx.send(path.clone());
            EventResult::PreventDefault
        } else {
            EventResult::Propagate
        }
    });
}

/// Wires the sidebar list + search/shuffle/prev/next callbacks that drive
/// playback navigation through the active queue's filtered/shuffled order.
fn wire_playlist_navigation(
    app: &AppWindow,
    mpv: &Rc<Mpv>,
    state: &Rc<RefCell<AppState>>,
    model: &Rc<VecModel<PlaylistItemData>>,
    sprite_timer: &Rc<slint::Timer>,
    sprite_tx: &std::sync::mpsc::Sender<(String, bool)>,
) {
    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let model = Rc::clone(model);
        let app_weak = app.as_weak();
        let sprite_timer = Rc::clone(sprite_timer);
        let sprite_tx = sprite_tx.clone();
        app.on_item_clicked(move |queue_index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let index = queue_index as usize;
            let mode = state.borrow().mode;
            match mode {
                ui_bridge::Mode::Video => {
                    ui_bridge::play_index(&mpv, &app, &mut state.borrow_mut(), &model, index);
                    ui_bridge::schedule_sprite_generation(
                        app_weak.clone(),
                        &state,
                        &model,
                        &sprite_timer,
                        sprite_tx.clone(),
                        index,
                    );
                }
                ui_bridge::Mode::Image => {
                    ui_bridge::show_image_at(&mpv, &app, &mut state.borrow_mut(), &model, index);
                }
                ui_bridge::Mode::All => {
                    let is_video = {
                        let mut state_ref = state.borrow_mut();
                        ui_bridge::present_item(&mpv, &app, &mut state_ref, &model, index);
                        state_ref.all_current_is_video
                    };
                    if is_video {
                        ui_bridge::schedule_sprite_generation(
                            app_weak.clone(),
                            &state,
                            &model,
                            &sprite_timer,
                            sprite_tx.clone(),
                            index,
                        );
                    }
                }
            }
        });
    }

    {
        let state = Rc::clone(state);
        let app_weak = app.as_weak();
        app.on_list_item_hovered(move |queue_index, _display_index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::show_list_sprite_preview(
                &app,
                &mut state.borrow_mut(),
                queue_index as usize,
            );
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_list_item_unhovered(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            ui_bridge::hide_list_sprite_preview(&app);
        });
    }

    {
        let state = Rc::clone(state);
        let model = Rc::clone(model);
        app.on_search_changed(move |text| {
            let mut state = state.borrow_mut();
            state.search_query = text.to_lowercase();
            ui_bridge::rebuild_playlist_model(&mut state, &model);
        });
    }

    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let model = Rc::clone(model);
        let app_weak = app.as_weak();
        app.on_toggle_shuffle(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            state.shuffle_on = !state.shuffle_on;
            app.set_shuffle_on(state.shuffle_on);
            let query = state.search_query.clone();
            // Both queues reshuffle independently — shuffle is a single
            // global toggle, but each queue keeps its own play order, port
            // of `toggleShuffle()` driving `reshuffle()` and
            // `reshuffleImages()` together.
            if state.shuffle_on {
                if state.queue.len() >= 2
                    && state.mode == ui_bridge::Mode::Video
                    && let Some(idx) = state.queue.reshuffle_jump_to_first(&query)
                {
                    ui_bridge::play_index(&mpv, &app, &mut state, &model, idx);
                }
                if state.image_queue.len() >= 2 {
                    state.image_queue.reshuffle_keep_current_first(&query);
                    if state.mode == ui_bridge::Mode::Image {
                        ui_bridge::sync_image_viewer_ui(&app, &mut state);
                    }
                }
                if state.all_queue.len() >= 2
                    && state.mode == ui_bridge::Mode::All
                    && let Some(idx) = state.all_queue.reshuffle_jump_to_first(&query)
                {
                    ui_bridge::present_item(&mpv, &app, &mut state, &model, idx);
                }
            } else {
                state.queue.reset_play_order();
                state.image_queue.reset_play_order();
                state.all_queue.reset_play_order();
                if state.mode == ui_bridge::Mode::Image {
                    ui_bridge::sync_image_viewer_ui(&app, &mut state);
                } else if state.mode == ui_bridge::Mode::All {
                    ui_bridge::sync_all_view_ui(&app, &mut state);
                }
            }
            ui_bridge::rebuild_playlist_model(&mut state, &model);
        });
    }

    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let model = Rc::clone(model);
        let app_weak = app.as_weak();
        let sprite_timer = Rc::clone(sprite_timer);
        let sprite_tx = sprite_tx.clone();
        app.on_previous_track(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let prev = {
                let state = state.borrow();
                state
                    .queue
                    .playable_prev(&state.search_query, state.shuffle_on, state.loop_on)
            };
            if let Some(idx) = prev {
                ui_bridge::play_index(&mpv, &app, &mut state.borrow_mut(), &model, idx);
                ui_bridge::schedule_sprite_generation(
                    app_weak.clone(),
                    &state,
                    &model,
                    &sprite_timer,
                    sprite_tx.clone(),
                    idx,
                );
            }
        });
    }

    {
        let mpv = Rc::clone(mpv);
        let state = Rc::clone(state);
        let model = Rc::clone(model);
        let app_weak = app.as_weak();
        let sprite_timer = Rc::clone(sprite_timer);
        let sprite_tx = sprite_tx.clone();
        app.on_next_track(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let next = {
                let state = state.borrow();
                state
                    .queue
                    .playable_next(&state.search_query, state.shuffle_on, state.loop_on)
            };
            if let Some(idx) = next {
                ui_bridge::play_index(&mpv, &app, &mut state.borrow_mut(), &model, idx);
                ui_bridge::schedule_sprite_generation(
                    app_weak.clone(),
                    &state,
                    &model,
                    &sprite_timer,
                    sprite_tx.clone(),
                    idx,
                );
            }
        });
    }
}

fn main() {
    let app = AppWindow::new().expect("failed to create AppWindow");

    let mpv = Rc::new(
        Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            // Without this, seeking past EOF makes mpv fully unload the file
            // (black screen, unresponsive to further seeks) instead of pausing
            // on the last frame — confirmed via manual testing.
            init.set_property("keep-open", "yes")?;
            Ok(())
        })
        .expect("failed to create mpv core"),
    );

    // Separate client purely for observing playback properties + catching
    // playback errors — keeps this independent of the main `mpv` handle used
    // for commands, matching the pattern libmpv's own examples use.
    let mpv_events = mpv
        .create_client(None)
        .expect("failed to create mpv event client");
    let _ = mpv_events.disable_deprecated_events();
    mpv_events
        .observe_property("time-pos", libmpv2::Format::Double, 1)
        .expect("observe time-pos");
    mpv_events
        .observe_property("duration", libmpv2::Format::Double, 2)
        .expect("observe duration");
    mpv_events
        .observe_property("pause", libmpv2::Format::Flag, 3)
        .expect("observe pause");
    // Drives queue auto-advance on natural end-of-file — see the
    // "eof-reached" handling in the drain timer below. `keep-open=yes`
    // (set at Mpv::with_initializer below) means mpv parks on the last
    // frame instead of unloading, so this flag is the only signal that a
    // track actually finished rather than being stopped/cleared.
    mpv_events
        .observe_property("eof-reached", libmpv2::Format::Flag, 4)
        .expect("observe eof-reached");

    let state = Rc::new(RefCell::new(AppState::new()));
    let model = Rc::new(VecModel::default());
    app.set_playlist_items(slint::ModelRc::from(model.clone()));
    let gallery_model: Rc<VecModel<slint::Image>> = Rc::new(VecModel::default());
    app.set_gallery_thumbnails(slint::ModelRc::from(gallery_model.clone()));
    let gallery_video_flags: Rc<VecModel<bool>> = Rc::new(VecModel::default());
    app.set_gallery_is_video(slint::ModelRc::from(gallery_video_flags.clone()));
    let gallery_failed_flags: Rc<VecModel<bool>> = Rc::new(VecModel::default());
    app.set_gallery_failed_flags(slint::ModelRc::from(gallery_failed_flags.clone()));
    app.set_view_mode(2);

    // Debounced thumbnail-sprite generation, triggered when the user selects
    // item changes (item click, prev/next, initial autoplay). Background
    // generation results come back over this channel and get drained by the
    // same timer that drains folder-scan batches, below.
    let sprite_timer = Rc::new(slint::Timer::default());
    let (sprite_tx, sprite_rx) = std::sync::mpsc::channel::<(String, bool)>();

    // Background gallery-grid poster generation results — see
    // `ui_bridge::gallery`'s doc comments. Drained by the same timer below.
    let (gallery_tx, gallery_rx) = std::sync::mpsc::channel::<ui_bridge::GalleryThumbResult>();

    // Paths from CLI / "Open with Flick" — queued once mpv's render context
    // exists (see `wire_video_underlay`). Same import pipeline as the file
    // picker; one valid file auto-plays, two or more open the grid.
    let startup_paths = Rc::new(RefCell::new(import::launch_paths_from_argv()));

    let (scan_tx, scan_rx) = std::sync::mpsc::channel::<Vec<library::ScannedFile>>();
    let (file_import_tx, file_import_rx) = std::sync::mpsc::channel::<import::FileImportBatch>();
    let (drop_tx, drop_rx) = std::sync::mpsc::channel::<PathBuf>();

    wire_video_underlay(
        &app,
        &mpv,
        &state,
        &model,
        ImportWiring {
            scan_tx: scan_tx.clone(),
            file_import_tx: file_import_tx.clone(),
            startup_paths: Rc::clone(&startup_paths),
            gallery_model: Rc::clone(&gallery_model),
            gallery_video_flags: Rc::clone(&gallery_video_flags),
            gallery_failed_flags: Rc::clone(&gallery_failed_flags),
            gallery_tx: gallery_tx.clone(),
        },
    );
    wire_playback_controls(&app, &mpv, &state);
    let app_ctx = AppContext {
        mpv: Rc::clone(&mpv),
        state: Rc::clone(&state),
        model: Rc::clone(&model),
        scan_tx: scan_tx.clone(),
        file_import_tx: file_import_tx.clone(),
        gallery_model: Rc::clone(&gallery_model),
        gallery_video_flags: Rc::clone(&gallery_video_flags),
        gallery_failed_flags: Rc::clone(&gallery_failed_flags),
        gallery_tx: gallery_tx.clone(),
    };
    wire_queue_management(&app, &app_ctx);
    // Centralize the slideshow timer so mode switches can stop it reliably.
    let slideshow_timer = Rc::new(slint::Timer::default());
    state.borrow_mut().slideshow_timer = Some(Rc::clone(&slideshow_timer));
    wire_image_viewer(&app, &app_ctx, &slideshow_timer, &sprite_timer, &sprite_tx);

    // Folder scanning runs on a background thread (recursive walk + magic-byte
    // checks, plus the size/video-hash `stat()`+SHA1 work — see
    // `library::ScannedFile` — shouldn't block the UI); batches come back
    // over this channel and get drained on the UI thread by the timer below.
    // `Rc`-based state can't cross threads, so the channel only carries
    // plain `PathBuf`s plus that already-computed metadata.
    wire_folder_scan(&app, &app_ctx);
    wire_file_drop(&app, drop_tx);
    wire_chrome_cursor_hiding(&app);

    {
        let mpv = Rc::clone(&mpv);
        let state = Rc::clone(&state);
        let model = Rc::clone(&model);
        let gallery_model = Rc::clone(&gallery_model);
        let app_weak = app.as_weak();
        let sprite_timer = Rc::clone(&sprite_timer);
        let sprite_tx = sprite_tx.clone();
        let scan_tx = scan_tx.clone();
        let file_import_tx = file_import_tx.clone();
        let file_import_rx = file_import_rx;
        let gallery_video_flags = Rc::clone(&gallery_video_flags);
        let gallery_tx = gallery_tx.clone();
        let drain_timer = slint::Timer::default();
        drain_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(100),
            move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                while let Ok(result) = gallery_rx.try_recv() {
                    ui_bridge::apply_gallery_thumb(
                        &mut state.borrow_mut(),
                        &app,
                        &ui_bridge::GalleryContext {
                            thumbnails: &gallery_model,
                            video_flags: &gallery_video_flags,
                            failed_flags: &gallery_failed_flags,
                            tx: &gallery_tx,
                        },
                        &model,
                        result,
                    );
                }
                while let Ok(batch) = file_import_rx.try_recv() {
                    import::apply_file_import_batch(
                        batch,
                        &import::ImportContext {
                            app: &app,
                            mpv: &mpv,
                            state: &state,
                            model: &model,
                            scan_tx: &scan_tx,
                            file_import_tx: &file_import_tx,
                            gallery: ui_bridge::GalleryContext {
                                thumbnails: &gallery_model,
                                video_flags: &gallery_video_flags,
                                failed_flags: &gallery_failed_flags,
                                tx: &gallery_tx,
                            },
                        },
                    );
                }
                let mut dropped = Vec::new();
                while let Ok(path) = drop_rx.try_recv() {
                    dropped.push(path);
                }
                if !dropped.is_empty() {
                    import::import_paths(
                        dropped,
                        &import::ImportContext {
                            app: &app,
                            mpv: &mpv,
                            state: &state,
                            model: &model,
                            scan_tx: &scan_tx,
                            file_import_tx: &file_import_tx,
                            gallery: ui_bridge::GalleryContext {
                                thumbnails: &gallery_model,
                                video_flags: &gallery_video_flags,
                                failed_flags: &gallery_failed_flags,
                                tx: &gallery_tx,
                            },
                        },
                    );
                }
                // Coalesce every batch sitting in the channel into one
                // `enqueue_paths` call instead of one call per batch. A large
                // folder scan can flood this channel with hundreds of batches
                // between ticks, and each call ends in a full
                // `rebuild_playlist_model` (O(current queue length)) — doing
                // that per-batch turns a 100k-item import into an O(n^2) pass
                // that visibly freezes the UI thread. Draining first bounds
                // the rebuild to at most once per tick no matter how many
                // batches arrived.
                let mut scanned = Vec::new();
                while let Ok(batch) = scan_rx.try_recv() {
                    scanned.extend(batch);
                }
                if !scanned.is_empty() {
                    let session_at_start = state.borrow().library_session;
                    let mut state_ref = state.borrow_mut();
                    let named = scanned
                        .into_iter()
                        .map(|f| {
                            if let Some(hash) = f.content_hash {
                                state_ref.prime_sprite_hash(f.path.clone(), hash);
                            }
                            (ui_bridge::basename(&f.path), f.path, f.size)
                        })
                        .collect();
                    drop(state_ref);
                    if session_at_start != state.borrow().library_session {
                        return;
                    }
                    ui_bridge::enqueue_paths(
                        &mpv,
                        &app,
                        &mut state.borrow_mut(),
                        &model,
                        named,
                        &ui_bridge::GalleryContext {
                            thumbnails: &gallery_model,
                            video_flags: &gallery_video_flags,
                            failed_flags: &gallery_failed_flags,
                            tx: &gallery_tx,
                        },
                    );
                }
                ui_bridge::tick_playlist_rebuild(&app, &mut state.borrow_mut(), &model);
                {
                    let gallery = ui_bridge::GalleryContext {
                        thumbnails: &gallery_model,
                        video_flags: &gallery_video_flags,
                        failed_flags: &gallery_failed_flags,
                        tx: &gallery_tx,
                    };
                    let mut state_ref = state.borrow_mut();
                    ui_bridge::try_start_pending_gallery_reload(&mut state_ref, &app, &gallery);
                    ui_bridge::try_finish_import_session(&mut state_ref, &app);
                    ui_bridge::try_start_pending_gallery_reload(&mut state_ref, &app, &gallery);
                }
                while let Ok((hash, ok)) = sprite_rx.try_recv() {
                    ui_bridge::apply_sprite_result(&app, &mut state.borrow_mut(), &model, hash, ok);
                }
                // Non-blocking poll (timeout 0.0): drain whatever's queued on the
                // playback-property/error event client since the last tick.
                loop {
                    match mpv_events.wait_event(0.0) {
                        Some(Ok(libmpv2::events::Event::PropertyChange {
                            name, change, ..
                        })) => match (name, change) {
                            ("time-pos", libmpv2::events::PropertyData::Double(v)) => {
                                app.set_current_time(v as f32);
                                app.set_current_time_text(ui_bridge::format_time(v).into());
                            }
                            ("duration", libmpv2::events::PropertyData::Double(v)) => {
                                app.set_duration(v as f32);
                                app.set_duration_text(ui_bridge::format_time(v).into());
                            }
                            ("pause", libmpv2::events::PropertyData::Flag(paused)) => {
                                app.set_playing(!paused);
                            }
                            ("eof-reached", libmpv2::events::PropertyData::Flag(true)) => {
                                let result = ui_bridge::advance_on_video_eof(
                                    &mpv,
                                    &app,
                                    &mut state.borrow_mut(),
                                    &model,
                                );
                                if let Some(idx) = result.video_index {
                                    ui_bridge::schedule_sprite_generation(
                                        app_weak.clone(),
                                        &state,
                                        &model,
                                        &sprite_timer,
                                        sprite_tx.clone(),
                                        idx,
                                    );
                                }
                                if result.restart_slideshow_timer && app.get_slideshow_on() {
                                    let duration = state.borrow().slideshow_duration;
                                    start_slideshow_timer(
                                        &mpv,
                                        &sprite_timer,
                                        &state,
                                        &model,
                                        &app_weak,
                                        duration,
                                    );
                                }
                            }
                            _ => {}
                        },
                        Some(Err(e)) => {
                            app.set_error_message(e.to_string().into());
                        }
                        Some(Ok(_)) => {}
                        None => break,
                    }
                }
            },
        );
        // Leaked intentionally: this timer must outlive `main` for the
        // duration of the app, same lifetime as the window itself.
        std::mem::forget(drain_timer);
    }

    wire_playlist_navigation(&app, &mpv, &state, &model, &sprite_timer, &sprite_tx);

    app.run().expect("event loop failed");
}
