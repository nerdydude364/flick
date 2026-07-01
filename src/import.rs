use crate::AppWindow;
use crate::library;
use crate::ui_bridge;
use crate::ui_bridge::AppState;
use libmpv2::Mpv;
use slint::VecModel;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Splits filesystem paths into regular files vs directories (folders are
/// scanned recursively, same as the Open Folder action).
pub fn partition_paths(paths: impl IntoIterator<Item = PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    for path in paths {
        if path.is_dir() {
            folders.push(path);
        } else {
            files.push(path);
        }
    }
    (files, folders)
}

pub type FileImportBatch = Vec<(String, PathBuf, Option<u64>)>;

/// CLI args and OS "Open with Flick" launches (file associations, Finder /
/// Explorer "Open with", etc.). Keeps only paths that exist — existing media
/// files and folders — so startup uses the same import path as the file
/// picker and drag-and-drop. A single valid item then auto-plays via
/// `enqueue_paths` → `finish_import` (grid view only when count >= 2).
pub fn launch_paths_from_argv() -> Vec<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let existing: Vec<PathBuf> = candidates.into_iter().filter(|p| p.exists()).collect();
    let (files, folders) = partition_paths(existing);
    let mut paths = library::filter_valid_media(files);
    paths.extend(folders);
    paths
}

pub fn named_media_entries(files: Vec<PathBuf>) -> FileImportBatch {
    library::filter_valid_media(files)
        .into_iter()
        .map(|p| {
            let size = std::fs::metadata(&p).ok().map(|m| m.len());
            (ui_bridge::basename(&p), p, size)
        })
        .collect()
}

pub fn scan_folders(
    folders: impl IntoIterator<Item = PathBuf>,
    scan_tx: &std::sync::mpsc::Sender<Vec<library::ScannedFile>>,
) {
    for folder in folders {
        let tx = scan_tx.clone();
        std::thread::spawn(move || {
            library::scan_folder(&folder, |batch| {
                if tx.send(batch).is_err() {
                    crate::flick_debug!(
                        "[import] scan batch channel closed for {}",
                        folder.display()
                    );
                }
            });
        });
    }
}

pub struct ImportContext<'a> {
    pub app: &'a AppWindow,
    pub mpv: &'a Rc<Mpv>,
    pub state: &'a Rc<RefCell<AppState>>,
    pub model: &'a Rc<VecModel<crate::PlaylistItemData>>,
    pub scan_tx: &'a std::sync::mpsc::Sender<Vec<library::ScannedFile>>,
    pub file_import_tx: &'a std::sync::mpsc::Sender<(u64, FileImportBatch)>,
    pub gallery: ui_bridge::GalleryContext<'a>,
}

/// Bounded worker pool width for hashing a picked/dropped-file batch —
/// matches `gallery.rs::CONCURRENCY`'s reasoning: enough parallelism to not
/// serialize on one slow file, not so much that it floods the disk/CPU.
const IMPORT_HASH_WORKERS: usize = 4;
/// Progressive delivery chunk size, matching `library::scan::SCAN_BATCH_SIZE`
/// — lets the sidebar/gallery start populating before the whole batch (which
/// can be thousands of files) finishes hashing.
const IMPORT_HASH_CHUNK: usize = 50;

/// Loads media files into the queue and kicks off background folder scans —
/// shared by the file picker, drag-and-drop, CLI args, and OS "Open with"
/// file-association launches (all converge on `enqueue_paths` / `finish_import`).
///
/// Everything expensive — including `partition_paths`'s `is_dir()` `stat()`
/// per path — runs on a background thread. `partition_paths` used to run
/// synchronously here, which for a large batch of picked/dropped paths on
/// slow or network-mounted storage was a direct, visible UI freeze.
pub fn import_paths(paths: Vec<PathBuf>, ctx: &ImportContext<'_>) {
    if paths.is_empty() {
        return;
    }
    let session = {
        let mut state = ctx.state.borrow_mut();
        state.library_loading = true;
        if state.library_loading_message.is_empty() {
            state.library_loading_message = "Importing…".into();
        }
        state.library_session
    };
    ui_bridge::sync_loading_ui(ctx.app, &mut ctx.state.borrow_mut());
    let file_import_tx = ctx.file_import_tx.clone();
    let scan_tx = ctx.scan_tx.clone();
    std::thread::spawn(move || {
        let (files, folders) = partition_paths(paths);
        if !folders.is_empty() {
            scan_folders(folders, &scan_tx);
        }
        if !files.is_empty() {
            hash_and_send_files(files, session, &file_import_tx);
        }
    });
}

/// Hashes a picked/dropped-file batch with a small bounded worker pool
/// (mirroring `gallery.rs::run_thumbnail_pool`'s shared-cursor design)
/// instead of one sequential thread, delivering results progressively in
/// `IMPORT_HASH_CHUNK`-sized batches (mirroring `scan_folder`'s batching)
/// instead of one all-or-nothing send at the end. `hash_video_file_cached`
/// itself is bounded (see `thumbnails::hash::hash_video_file_bounded`), so a
/// single stalled read (network mount, disconnected drive) blocks at most
/// one of `IMPORT_HASH_WORKERS` workers rather than the whole batch.
fn hash_and_send_files(
    files: Vec<PathBuf>,
    session: u64,
    tx: &std::sync::mpsc::Sender<(u64, FileImportBatch)>,
) {
    let named = named_media_entries(files);
    if named.is_empty() {
        return;
    }
    let items = std::sync::Arc::new(named);
    let cursor = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let workers = IMPORT_HASH_WORKERS.min(items.len());
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let items = std::sync::Arc::clone(&items);
        let cursor = std::sync::Arc::clone(&cursor);
        let tx = tx.clone();
        handles.push(std::thread::spawn(move || {
            let mut batch = Vec::with_capacity(IMPORT_HASH_CHUNK);
            loop {
                let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(entry) = items.get(i) else { break };
                if let Err(err) = crate::thumbnails::hash::hash_video_file_cached(&entry.1) {
                    crate::flick_debug!(
                        "[import] content hash failed {}: {err}",
                        entry.1.display()
                    );
                }
                batch.push(entry.clone());
                if batch.len() >= IMPORT_HASH_CHUNK {
                    let chunk =
                        std::mem::replace(&mut batch, Vec::with_capacity(IMPORT_HASH_CHUNK));
                    if tx.send((session, chunk)).is_err() {
                        return;
                    }
                }
            }
            if !batch.is_empty() {
                let _ = tx.send((session, batch));
            }
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
}

pub fn apply_file_import_batch(batch: FileImportBatch, ctx: &ImportContext<'_>) {
    if batch.is_empty() {
        return;
    }
    ui_bridge::enqueue_paths(
        ctx.mpv,
        ctx.app,
        &mut ctx.state.borrow_mut(),
        ctx.model,
        batch,
        &ctx.gallery,
    );
}
