use std::collections::{HashSet, VecDeque};

#[derive(Debug, PartialEq, Eq)]
pub enum PieceState {
    Missing,
    Downloading,
    Downloaded,
}

pub struct TorrentManager {
    pub total_pieces: u32,
    missing: VecDeque<u32>,
    in_progress: HashSet<u32>,
    completed: HashSet<u32>,
}

impl TorrentManager {
    pub fn new(total_pieces: u32) -> Self {
        let missing: VecDeque<u32> = (0..total_pieces).collect();

        Self {
            total_pieces,
            missing,
            in_progress: HashSet::new(),
            completed: HashSet::new(),
        }
    }

    pub fn get_next_work(&mut self) -> Option<u32> {
        if let Some(piece) = self.missing.pop_front() {
            self.in_progress.insert(piece);
            Some(piece)
        } else {
            None
        }
    }

    pub fn return_work(&mut self, piece_index: u32) {
        if self.in_progress.remove(&piece_index) {
            self.missing.push_back(piece_index);
        }
    }

    pub fn mark_completed(&mut self, piece_index: u32) {
        self.in_progress.remove(&piece_index);
        self.completed.insert(piece_index);
    }

    pub fn progress(&self) -> f32 {
        if self.total_pieces == 0 {
            0.0
        } else {
            self.completed.len() as f32 / self.total_pieces as f32
        }
    }

    pub fn is_done(&self) -> bool {
        self.completed.len() as u32 == self.total_pieces
    }

    pub fn piece_state(&self, piece_index: u32) -> Option<PieceState> {
        if piece_index >= self.total_pieces {
            return None;
        }

        if self.completed.contains(&piece_index) {
            Some(PieceState::Downloaded)
        } else if self.in_progress.contains(&piece_index) {
            Some(PieceState::Downloading)
        } else {
            Some(PieceState::Missing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_tracks_workflow() {
        let mut mgr = TorrentManager::new(2);
        assert_eq!(mgr.progress(), 0.0);
        assert_eq!(mgr.piece_state(0), Some(PieceState::Missing));

        let p0 = mgr.get_next_work();
        assert_eq!(p0, Some(0));
        assert_eq!(mgr.piece_state(0), Some(PieceState::Downloading));

        mgr.return_work(0);
        assert_eq!(mgr.piece_state(0), Some(PieceState::Missing));

        let p0_again = mgr.get_next_work();
        assert_eq!(p0_again, Some(1));

        mgr.mark_completed(1);
        assert!(mgr.progress() > 0.0);
        assert_eq!(mgr.piece_state(1), Some(PieceState::Downloaded));
    }
}
