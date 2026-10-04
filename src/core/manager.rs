use std::collections::{HashMap, HashSet, VecDeque};

use rand::Rng;

use crate::core::bitfield::Bitfield;

/// How many peers may download the same piece at once during endgame.
const MAX_ENDGAME_HOLDERS: u32 = 3;

#[derive(Debug, PartialEq, Eq)]
pub enum PieceState {
    Missing,
    Downloading,
    Downloaded,
}

pub struct TorrentManager {
    pub total_pieces: u32,
    missing: VecDeque<u32>,
    /// Piece index -> number of peers currently downloading it.
    in_progress: HashMap<u32, u32>,
    completed: HashSet<u32>,
    /// Number of connected peers that advertise each piece.
    availability: Vec<u16>,
    missing_dirty: bool,
}

impl TorrentManager {
    pub fn new(total_pieces: u32) -> Self {
        Self::from_completed(total_pieces, &[])
    }

    pub fn from_completed(total_pieces: u32, completed_pieces: &[u32]) -> Self {
        let completed: HashSet<u32> = completed_pieces
            .iter()
            .copied()
            .filter(|idx| *idx < total_pieces)
            .collect();

        let missing: VecDeque<u32> = (0..total_pieces)
            .filter(|idx| !completed.contains(idx))
            .collect();

        Self {
            total_pieces,
            missing,
            in_progress: HashMap::new(),
            completed,
            availability: vec![0; total_pieces as usize],
            missing_dirty: false,
        }
    }

    /// Picks the next piece for a peer that advertises `peer_has`.
    ///
    /// Rarest-first among missing pieces (random tie-break). When nothing is
    /// left unassigned the manager enters endgame and lets several peers race
    /// for the pieces that are still in flight.
    pub fn next_work_for(&mut self, peer_has: &Bitfield) -> Option<u32> {
        if self.missing_dirty {
            let completed = &self.completed;
            self.missing.retain(|p| !completed.contains(p));
            self.missing_dirty = false;
        }

        let mut rng = rand::thread_rng();
        let mut best: Option<(usize, u16)> = None;
        let mut ties = 0u32;
        for (pos, &piece) in self.missing.iter().enumerate() {
            if !peer_has.has(piece as usize) {
                continue;
            }
            let avail = self.availability[piece as usize];
            match best {
                Some((_, best_avail)) if avail > best_avail => {}
                Some((_, best_avail)) if avail == best_avail => {
                    ties += 1;
                    if rng.gen_range(0..=ties) == 0 {
                        best = Some((pos, avail));
                    }
                }
                _ => {
                    best = Some((pos, avail));
                    ties = 0;
                }
            }
        }

        if let Some((pos, _)) = best {
            let piece = self.missing.remove(pos)?;
            self.in_progress.insert(piece, 1);
            return Some(piece);
        }

        if !self.missing.is_empty() {
            return None;
        }

        let candidate = self
            .in_progress
            .iter()
            .filter(|(piece, holders)| {
                **holders < MAX_ENDGAME_HOLDERS && peer_has.has(**piece as usize)
            })
            .min_by_key(|(piece, holders)| (**holders, **piece))
            .map(|(piece, _)| *piece)?;
        *self.in_progress.get_mut(&candidate)? += 1;
        Some(candidate)
    }

    pub fn return_work(&mut self, piece_index: u32) {
        let Some(holders) = self.in_progress.get_mut(&piece_index) else {
            return;
        };
        *holders -= 1;
        if *holders == 0 {
            self.in_progress.remove(&piece_index);
            if !self.completed.contains(&piece_index) {
                self.missing.push_back(piece_index);
            }
        }
    }

    pub fn mark_completed(&mut self, piece_index: u32) {
        if piece_index >= self.total_pieces {
            return;
        }
        self.in_progress.remove(&piece_index);
        if self.completed.insert(piece_index) {
            self.missing_dirty = true;
        }
    }

    pub fn is_completed(&self, piece_index: u32) -> bool {
        self.completed.contains(&piece_index)
    }

    pub fn add_peer_pieces(&mut self, pieces: &Bitfield) {
        for index in pieces.iter_ones() {
            if let Some(slot) = self.availability.get_mut(index) {
                *slot = slot.saturating_add(1);
            }
        }
    }

    pub fn remove_peer_pieces(&mut self, pieces: &Bitfield) {
        for index in pieces.iter_ones() {
            if let Some(slot) = self.availability.get_mut(index) {
                *slot = slot.saturating_sub(1);
            }
        }
    }

    pub fn add_peer_piece(&mut self, index: u32) {
        if let Some(slot) = self.availability.get_mut(index as usize) {
            *slot = slot.saturating_add(1);
        }
    }

    pub fn availability(&self, index: u32) -> u16 {
        self.availability.get(index as usize).copied().unwrap_or(0)
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
        } else if self.in_progress.contains_key(&piece_index) {
            Some(PieceState::Downloading)
        } else {
            Some(PieceState::Missing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(len: usize) -> Bitfield {
        Bitfield::from_indices(len, 0..len as u32)
    }

    #[test]
    fn manager_tracks_workflow() {
        let mut mgr = TorrentManager::new(2);
        assert_eq!(mgr.progress(), 0.0);
        assert_eq!(mgr.piece_state(0), Some(PieceState::Missing));

        let p = mgr.next_work_for(&all(2)).unwrap();
        assert_eq!(mgr.piece_state(p), Some(PieceState::Downloading));

        mgr.return_work(p);
        assert_eq!(mgr.piece_state(p), Some(PieceState::Missing));

        let again = mgr.next_work_for(&Bitfield::from_indices(2, [p])).unwrap();
        assert_eq!(again, p);
        mgr.mark_completed(p);
        assert!(mgr.progress() > 0.0);
        assert_eq!(mgr.piece_state(p), Some(PieceState::Downloaded));
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

    #[test]
    fn only_assigns_pieces_the_peer_has() {
        let mut mgr = TorrentManager::new(4);
        let peer = Bitfield::from_indices(4, [2]);
        assert_eq!(mgr.next_work_for(&peer), Some(2));
        assert_eq!(mgr.next_work_for(&peer), None);
        assert_eq!(mgr.next_work_for(&Bitfield::new(4)), None);
    }

    #[test]
    fn prefers_rarest_piece() {
        let mut mgr = TorrentManager::new(3);
        mgr.add_peer_pieces(&Bitfield::from_indices(3, [0, 1]));
        mgr.add_peer_pieces(&Bitfield::from_indices(3, [0, 1]));
        mgr.add_peer_pieces(&Bitfield::from_indices(3, [0, 2]));
        assert_eq!(mgr.availability(0), 3);
        assert_eq!(mgr.availability(2), 1);
        assert_eq!(mgr.next_work_for(&all(3)), Some(2));
        assert_eq!(mgr.next_work_for(&all(3)), Some(1));
        mgr.remove_peer_pieces(&Bitfield::from_indices(3, [0, 1]));
        assert_eq!(mgr.availability(0), 2);
    }

    #[test]
    fn completed_pieces_are_never_reassigned() {
        let mut mgr = TorrentManager::new(3);
        mgr.mark_completed(0);
        mgr.mark_completed(2);
        assert_eq!(mgr.next_work_for(&all(3)), Some(1));
        // A peer that only has pieces we already completed gets nothing.
        assert_eq!(mgr.next_work_for(&Bitfield::from_indices(3, [0, 2])), None);
    }

    #[test]
    fn endgame_shares_in_flight_pieces() {
        let mut mgr = TorrentManager::new(1);
        let peer = all(1);
        assert_eq!(mgr.next_work_for(&peer), Some(0));
        assert_eq!(mgr.next_work_for(&peer), Some(0));
        assert_eq!(mgr.next_work_for(&peer), Some(0));
        assert_eq!(mgr.next_work_for(&peer), None);

        mgr.return_work(0);
        mgr.mark_completed(0);
        mgr.return_work(0);
        mgr.return_work(0);
        assert!(mgr.is_done());
        assert_eq!(mgr.next_work_for(&peer), None);
    }

    #[test]
    fn failed_piece_returns_to_queue_only_after_last_holder_gives_up() {
        let mut mgr = TorrentManager::new(1);
        let peer = all(1);
        mgr.next_work_for(&peer);
        mgr.next_work_for(&peer);
        mgr.return_work(0);
        assert_eq!(mgr.piece_state(0), Some(PieceState::Downloading));
        mgr.return_work(0);
        assert_eq!(mgr.piece_state(0), Some(PieceState::Missing));
    }
}
