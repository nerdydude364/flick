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

fn named_media_entries(files: Vec<PathBuf>) -> Vec<(String, PathBuf, Option<u64>)> {
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
    pub app_weak: &'a slint::Weak<AppWindow>,
    pub sprite_timer: &'a Rc<slint::Timer>,
    pub sprite_tx: &'a std::sync::mpsc::Sender<(String, bool)>,
    pub scan_tx: &'a std::sync::mpsc::Sender<Vec<library::ScannedFile>>,
}

/// Loads media files into the queue and kicks off background folder scans —
/// shared by the file picker, drag-and-drop, and CLI / "Open with" launches.
pub fn import_paths(paths: Vec<PathBuf>, ctx: &ImportContext<'_>) {
    if paths.is_empty() {
        return;
    }
    let (files, folders) = partition_paths(paths);
    if !files.is_empty() {
        let named = named_media_entries(files);
        if !named.is_empty() {
            let played = ui_bridge::enqueue_paths(
                ctx.mpv,
                ctx.app,
                &mut ctx.state.borrow_mut(),
                ctx.model,
                named,
            );
            if let Some(idx) = played {
                ui_bridge::schedule_sprite_generation(
                    ctx.app_weak.clone(),
                    ctx.state,
                    ctx.model,
                    ctx.sprite_timer,
                    ctx.sprite_tx.clone(),
                    idx,
                );
            }
        }
    }
    scan_folders(folders, ctx.scan_tx);
}
