mod item;
mod shuffle;

pub use item::MediaItem;
use shuffle::fisher_yates;
use std::collections::HashSet;
use std::path::PathBuf;

/// What the caller should do after removing an item — mirrors the three
/// branches in `removeVideoAt`/`removeImageAt` (renderer/app.js), minus the
/// actual side effects (loading a file, resetting the player UI), which are
/// the caller's responsibility since `Queue` itself is side-effect-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    QueueEmpty,
    /// The removed item was the one playing; caller should load this index instead.
    NowPlayingChanged(usize),
    NoPlaybackChange,
}

/// One playlist (video queue or image queue). The two queues in the
/// Electron app share global `shuffleOn`/`loopOn`/`searchQuery` toggles but
/// each keep their own items + shuffle order — so those toggles are passed
/// in as parameters here rather than owned by `Queue`, and the same type is
/// used for both the video and image queue.
#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<MediaItem>,
    paths: HashSet<PathBuf>,
    play_order: Vec<usize>,
    now_playing: Option<usize>,
    next_id: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn item(&self, index: usize) -> Option<&MediaItem> {
        self.items.get(index)
    }

    pub fn now_playing(&self) -> Option<usize> {
        self.now_playing
    }

    pub fn set_now_playing(&mut self, index: Option<usize>) {
        self.now_playing = index;
    }

    /// Adds items not already present (deduped by path, matching the original's
    /// dedup-by-`file://`-URL — paths are the native-app equivalent). Returns
    /// the indices of items actually added; the caller decides what to do with
    /// them (auto-switch mode, autoplay if nothing was playing, re-shuffle if
    /// shuffle is already on — all orchestration in `enqueue()` in app.js that
    /// spans both queues / the player).
    pub fn enqueue(
        &mut self,
        candidates: impl IntoIterator<Item = (String, PathBuf)>,
    ) -> Vec<usize> {
        let mut added = Vec::new();
        for (name, path) in candidates {
            if self.paths.insert(path.clone()) {
                self.next_id += 1;
                added.push(self.items.len());
                let name_lower = name.to_lowercase();
                let size_bytes = std::fs::metadata(&path).map(|m| m.len()).ok();
                self.items.push(MediaItem {
                    id: self.next_id,
                    name,
                    path,
                    name_lower,
                    size_bytes,
                });
            }
        }
        added
    }

    /// Port of `removeVideoAt`/`removeImageAt`. Unlike the original's
    /// `removeVideoAt`, this also fixes up `play_order` on removal (the
    /// original only does this for images — leaving the video queue's
    /// `playOrder` stale after a remove-while-shuffled is a latent bug there,
    /// not an intentional difference, so both queues get the correct
    /// behavior here).
    pub fn remove_at(&mut self, index: usize) -> RemoveOutcome {
        self.paths.remove(&self.items[index].path);
        self.items.remove(index);

        if let Some(pos) = self.play_order.iter().position(|&i| i == index) {
            self.play_order.remove(pos);
        }
        for i in self.play_order.iter_mut() {
            if *i > index {
                *i -= 1;
            }
        }

        if self.items.is_empty() {
            self.now_playing = None;
            return RemoveOutcome::QueueEmpty;
        }

        match self.now_playing {
            Some(np) if np == index => {
                let next = index.min(self.items.len() - 1);
                self.now_playing = Some(next);
                RemoveOutcome::NowPlayingChanged(next)
            }
            Some(np) if np > index => {
                self.now_playing = Some(np - 1);
                RemoveOutcome::NoPlaybackChange
            }
            _ => RemoveOutcome::NoPlaybackChange,
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.paths.clear();
        self.play_order.clear();
        self.now_playing = None;
    }

    /// Resets the shuffle order back to identity (queue) order — port of
    /// `playOrder = queue.map((_, i) => i)` in `toggleShuffle()`'s off branch.
    pub fn reset_play_order(&mut self) {
        self.play_order = (0..self.items.len()).collect();
    }

    /// Port of `moveQueueItem`. `dst` must already be the post-removal
    /// "effective" destination index (the drop handler computes
    /// `dst > src ? dst - 1 : dst` before calling this — same split here).
    /// Resets `play_order` to identity order afterward, matching the
    /// original — a manual reorder cancels the current shuffle order.
    pub fn move_item(&mut self, src: usize, dst: usize) {
        let it = self.items.remove(src);
        self.items.insert(dst, it);

        self.now_playing = match self.now_playing {
            Some(np) if np == src => Some(dst),
            Some(np) if src < np && dst >= np => Some(np - 1),
            Some(np) if src > np && dst <= np => Some(np + 1),
            other => other,
        };

        self.play_order = (0..self.items.len()).collect();
    }

    /// Port of `filteredVideoIndices`/`filteredImageIndices`: case-insensitive
    /// substring match on item name, or every index if `search_query` is empty.
    pub fn filtered_indices(&self, search_query: &str) -> Vec<usize> {
        if search_query.is_empty() {
            return (0..self.items.len()).collect();
        }
        let q = search_query.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.name_lower.contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    /// Port of `currentOrderList`/`getActiveImageIndices`: filtered indices,
    /// reordered by the shuffle order when shuffle is on (falling back to the
    /// plain filtered order if the shuffle filter produces nothing — e.g. a
    /// stale/short play_order).
    pub fn current_order(&self, search_query: &str, shuffle_on: bool) -> Vec<usize> {
        let filtered = self.filtered_indices(search_query);
        if shuffle_on && self.play_order.len() >= 2 {
            let filtered_set: HashSet<usize> = filtered.iter().copied().collect();
            let result: Vec<usize> = self
                .play_order
                .iter()
                .copied()
                .filter(|i| filtered_set.contains(i))
                .collect();
            if !result.is_empty() {
                return result;
            }
        }
        filtered
    }

    /// Port of `playIndexInOrder`.
    pub fn play_index_in_order(&self, idx: usize, search_query: &str, shuffle_on: bool) -> usize {
        let order = self.current_order(search_query, shuffle_on);
        order.get(idx).copied().unwrap_or(idx)
    }

    /// Port of `indexOfInOrder`.
    pub fn index_of_in_order(
        &self,
        index: usize,
        search_query: &str,
        shuffle_on: bool,
    ) -> Option<usize> {
        self.current_order(search_query, shuffle_on)
            .iter()
            .position(|&i| i == index)
    }

    /// Port of `playableNext`.
    pub fn playable_next(
        &self,
        search_query: &str,
        shuffle_on: bool,
        loop_on: bool,
    ) -> Option<usize> {
        let order = self.current_order(search_query, shuffle_on);
        if order.len() < 2 {
            return if loop_on && order.len() == 1 {
                Some(order[0])
            } else {
                None
            };
        }
        // -1 when nothing is playing, matching `order.indexOf(-1) === -1` in JS.
        let pos: i64 = self
            .now_playing
            .and_then(|np| self.index_of_in_order(np, search_query, shuffle_on))
            .map(|p| p as i64)
            .unwrap_or(-1);
        if loop_on {
            let next = (pos + 1).rem_euclid(order.len() as i64) as usize;
            return Some(self.play_index_in_order(next, search_query, shuffle_on));
        }
        if pos < order.len() as i64 - 1 {
            return Some(self.play_index_in_order((pos + 1) as usize, search_query, shuffle_on));
        }
        None
    }

    /// Port of `playablePrev`.
    pub fn playable_prev(
        &self,
        search_query: &str,
        shuffle_on: bool,
        loop_on: bool,
    ) -> Option<usize> {
        let order = self.current_order(search_query, shuffle_on);
        if order.len() < 2 {
            return None;
        }
        let pos: i64 = self
            .now_playing
            .and_then(|np| self.index_of_in_order(np, search_query, shuffle_on))
            .map(|p| p as i64)
            .unwrap_or(-1);
        if loop_on {
            let prev = (pos - 1).rem_euclid(order.len() as i64) as usize;
            return Some(self.play_index_in_order(prev, search_query, shuffle_on));
        }
        if pos > 0 {
            return Some(self.play_index_in_order((pos - 1) as usize, search_query, shuffle_on));
        }
        None
    }

    /// Port of `reshuffle` (video queue): fresh Fisher-Yates shuffle, then
    /// jump now_playing to whatever ends up first in the new order.
    pub fn reshuffle_jump_to_first(&mut self, search_query: &str) -> Option<usize> {
        self.play_order = (0..self.items.len()).collect();
        fisher_yates(&mut self.play_order);
        let order = self.current_order(search_query, true);
        if order.is_empty() {
            return None;
        }
        self.now_playing = Some(order[0]);
        self.now_playing
    }

    /// Port of `reshuffleImages`: fresh Fisher-Yates shuffle, but if the
    /// currently-playing item is still in the active/filtered set, pin it to
    /// the front of the new order instead of jumping to a different item.
    pub fn reshuffle_keep_current_first(&mut self, search_query: &str) {
        self.play_order = (0..self.items.len()).collect();
        fisher_yates(&mut self.play_order);
        if let Some(now) = self.now_playing {
            let active = self.current_order(search_query, true);
            if active.contains(&now)
                && let Some(pos) = self.play_order.iter().position(|&i| i == now)
                && pos > 0
            {
                self.play_order.remove(pos);
                self.play_order.insert(0, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> (String, PathBuf) {
        (name.to_string(), PathBuf::from(format!("/media/{name}")))
    }

    fn queue_with(names: &[&str]) -> Queue {
        let mut q = Queue::new();
        q.enqueue(names.iter().map(|n| item(n)));
        q
    }

    #[test]
    fn enqueue_dedupes_by_path() {
        let mut q = Queue::new();
        let added = q.enqueue([item("a.mp4"), item("b.mp4"), item("a.mp4")]);
        assert_eq!(added.len(), 2);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn remove_last_item_empties_queue() {
        let mut q = queue_with(&["a", "b"]);
        q.remove_at(0);
        let outcome = q.remove_at(0);
        assert_eq!(outcome, RemoveOutcome::QueueEmpty);
        assert!(q.is_empty());
        assert_eq!(q.now_playing(), None);
    }

    #[test]
    fn remove_currently_playing_clamps_to_last_valid_index() {
        let mut q = queue_with(&["a", "b", "c"]);
        q.set_now_playing(Some(2));
        let outcome = q.remove_at(2);
        assert_eq!(outcome, RemoveOutcome::NowPlayingChanged(1));
        assert_eq!(q.now_playing(), Some(1));
    }

    #[test]
    fn remove_before_now_playing_shifts_index_down() {
        let mut q = queue_with(&["a", "b", "c"]);
        q.set_now_playing(Some(2));
        let outcome = q.remove_at(0);
        assert_eq!(outcome, RemoveOutcome::NoPlaybackChange);
        assert_eq!(q.now_playing(), Some(1));
    }

    #[test]
    fn move_item_updates_now_playing_when_moved_item_is_playing() {
        let mut q = queue_with(&["a", "b", "c"]);
        q.set_now_playing(Some(0));
        q.move_item(0, 2);
        assert_eq!(q.now_playing(), Some(2));
        assert_eq!(q.item(2).unwrap().name, "a");
    }

    #[test]
    fn move_item_shifts_now_playing_when_crossed() {
        let mut q = queue_with(&["a", "b", "c"]);
        q.set_now_playing(Some(1));
        q.move_item(2, 0);
        assert_eq!(q.now_playing(), Some(2));
    }

    #[test]
    fn filtered_indices_matches_case_insensitive_substring() {
        let q = queue_with(&["Holiday.mp4", "work.mp4", "holiday2.mp4"]);
        assert_eq!(q.filtered_indices("HOLIDAY"), vec![0, 2]);
        assert_eq!(q.filtered_indices(""), vec![0, 1, 2]);
    }

    #[test]
    fn playable_next_without_loop_stops_at_end() {
        let mut q = queue_with(&["a", "b"]);
        q.set_now_playing(Some(0));
        assert_eq!(q.playable_next("", false, false), Some(1));
        q.set_now_playing(Some(1));
        assert_eq!(q.playable_next("", false, false), None);
    }

    #[test]
    fn playable_next_with_loop_wraps_around() {
        let mut q = queue_with(&["a", "b"]);
        q.set_now_playing(Some(1));
        assert_eq!(q.playable_next("", false, true), Some(0));
    }

    #[test]
    fn playable_prev_without_loop_stops_at_start() {
        let mut q = queue_with(&["a", "b"]);
        q.set_now_playing(Some(0));
        assert_eq!(q.playable_prev("", false, false), None);
    }

    #[test]
    fn single_item_with_loop_repeats_itself() {
        let mut q = queue_with(&["a"]);
        q.set_now_playing(Some(0));
        assert_eq!(q.playable_next("", false, true), Some(0));
    }

    #[test]
    fn reshuffle_jump_to_first_picks_a_valid_item() {
        let mut q = queue_with(&["a", "b", "c", "d", "e"]);
        q.set_now_playing(Some(0));
        let chosen = q.reshuffle_jump_to_first("");
        assert!(chosen.is_some());
        assert!(chosen.unwrap() < 5);
    }

    #[test]
    fn reshuffle_keep_current_first_keeps_now_playing_unchanged() {
        let mut q = queue_with(&["a", "b", "c", "d", "e"]);
        q.set_now_playing(Some(3));
        q.reshuffle_keep_current_first("");
        // now_playing itself never changes in this variant — only play_order does.
        assert_eq!(q.now_playing(), Some(3));
        assert_eq!(q.current_order("", true)[0], 3);
    }
}
