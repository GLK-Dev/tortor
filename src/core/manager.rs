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

    pub fn from_completed(total_pieces: u32, completed_pieces: &[u32]) -> Self {
        let mut completed: HashSet<u32> = completed_pieces
            .iter()
            .copied()
            .filter(|idx| *idx < total_pieces)
            .collect();

        if completed.len() as u32 > total_pieces {
            completed.clear();
        }

        let missing: VecDeque<u32> = (0..total_pieces)
            .filter(|idx| !completed.contains(idx))
            .collect();

        Self {
            total_pieces,
            missing,
            in_progress: HashSet::new(),
            completed,
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

    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }

    pub fn completed_pieces(&self) -> Vec<u32> {
        let mut pieces: Vec<u32> = self.completed.iter().copied().collect();
        pieces.sort_unstable();
        pieces
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

    #[test]
    fn manager_restores_from_completed() {
        let mgr = TorrentManager::from_completed(4, &[1, 3, 9]);
        assert_eq!(mgr.completed_count(), 2);
        assert_eq!(mgr.piece_state(1), Some(PieceState::Downloaded));
        assert_eq!(mgr.piece_state(3), Some(PieceState::Downloaded));
        assert_eq!(mgr.piece_state(0), Some(PieceState::Missing));
        assert_eq!(mgr.piece_state(2), Some(PieceState::Missing));
    }
}
