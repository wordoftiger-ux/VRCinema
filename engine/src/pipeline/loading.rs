//! What the start of a film is doing right now (reading its link, downloading it, preparing its
//! first segments), for the panel's "Now loading" card. The clip factory writes the stage of the
//! film it starts; the panel reads it next to the channel's own view of the start
//! ([`crate::stats::LoadingInfo`]). Shared and cheap to clone.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Films remembered at most (only the one starting matters; older ones are let go).
const KEEP: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStage {
    /// yt-dlp (or a disk resolver) reads the link; ffprobe analyses the media.
    Resolving,
    /// yt-dlp downloads the whole film (sites that refuse ffmpeg's own reading).
    Downloading,
    /// ffmpeg starts and cuts the first segments.
    Preparing,
}

impl LoadStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::Preparing => "preparing",
        }
    }
}

/// The stage of every film being started, by queue item id.
#[derive(Clone, Default)]
pub struct LoadStages(Arc<Mutex<HashMap<i64, LoadStage>>>);

impl LoadStages {
    pub fn set(&self, item_id: i64, stage: LoadStage) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= KEEP && !map.contains_key(&item_id) {
            map.clear();
        }
        map.insert(item_id, stage);
    }

    pub fn get(&self, item_id: i64) -> Option<LoadStage> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&item_id)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_kept_per_film_and_old_films_are_let_go() {
        let s = LoadStages::default();
        assert_eq!(s.get(1), None);
        s.set(1, LoadStage::Resolving);
        s.set(1, LoadStage::Downloading);
        assert_eq!(s.get(1), Some(LoadStage::Downloading));
        for id in 2..=KEEP as i64 {
            s.set(id, LoadStage::Preparing);
        }
        assert_eq!(s.get(1), Some(LoadStage::Downloading));
        s.set(100, LoadStage::Resolving);
        assert_eq!(s.get(1), None, "beyond the limit the old ones go");
        assert_eq!(s.get(100).map(LoadStage::as_str), Some("resolving"));
    }
}
