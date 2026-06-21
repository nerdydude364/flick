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
                let _ = tx.send(batch);
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
    pub file_import_tx: &'a std::sync::mpsc::Sender<FileImportBatch>,
    pub gallery: ui_bridge::GalleryContext<'a>,
}

/// Loads media files into the queue and kicks off background folder scans —
/// shared by the file picker, drag-and-drop, CLI args, and OS "Open with"
/// file-association launches (all converge on `enqueue_paths` / `finish_import`).
pub fn import_paths(paths: Vec<PathBuf>, ctx: &ImportContext<'_>) {
    if paths.is_empty() {
        return;
    }
    let (files, folders) = partition_paths(paths);
    if !files.is_empty() {
        {
            let mut state = ctx.state.borrow_mut();
            state.library_loading = true;
            state.library_loading_message = "Reading files…".into();
        }
        ui_bridge::sync_loading_ui(ctx.app, &mut ctx.state.borrow_mut());
        let tx = ctx.file_import_tx.clone();
        std::thread::spawn(move || {
            let named = named_media_entries(files);
            for (_, path, _) in &named {
                let _ = crate::thumbnails::hash::hash_video_file_cached(path);
            }
            let _ = tx.send(named);
        });
    }
    scan_folders(folders, ctx.scan_tx);
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
