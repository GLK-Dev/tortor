//! Selective download: which pieces are needed for the chosen files and which
//! finished pieces cannot be served because part of them was never stored.

use crate::core::bitfield::Bitfield;
use crate::core::torrent::TorrentFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Pieces that overlap at least one selected file.
    pub wanted: Bitfield,
    /// Pieces that overlap a file that is not stored. Their data is verified
    /// when downloaded but incomplete on disk, so they are never served.
    pub unservable: Bitfield,
}

impl Selection {
    /// Every piece is wanted and stored.
    pub fn all(total_pieces: usize) -> Self {
        Self {
            wanted: Bitfield::full(total_pieces),
            unservable: Bitfield::new(total_pieces),
        }
    }

    /// `selected[i]` tells whether `files[i]` is downloaded.
    pub fn from_files(
        files: &[TorrentFile],
        selected: &[bool],
        piece_length: u32,
        total_pieces: usize,
    ) -> Self {
        let mut wanted = Bitfield::new(total_pieces);
        let mut unservable = Bitfield::new(total_pieces);
        let piece_length = piece_length.max(1) as u64;

        let mut offset = 0u64;
        for (file, &keep) in files.iter().zip(selected) {
            if file.length > 0 {
                let first = (offset / piece_length) as usize;
                let last = ((offset + file.length - 1) / piece_length) as usize;
                for piece in first..=last.min(total_pieces.saturating_sub(1)) {
                    if keep {
                        wanted.set(piece);
                    } else {
                        unservable.set(piece);
                    }
                }
            }
            offset += file.length;
        }

        Self { wanted, unservable }
    }

    /// Pieces that overlap at least one file whose flag is `true`.
    pub fn pieces_of_files(
        files: &[TorrentFile],
        flags: &[bool],
        piece_length: u32,
        total_pieces: usize,
    ) -> Bitfield {
        let mut pieces = Bitfield::new(total_pieces);
        let piece_length = piece_length.max(1) as u64;

        let mut offset = 0u64;
        for (file, &flag) in files.iter().zip(flags) {
            if flag && file.length > 0 {
                let first = (offset / piece_length) as usize;
                let last = ((offset + file.length - 1) / piece_length) as usize;
                for piece in first..=last.min(total_pieces.saturating_sub(1)) {
                    pieces.set(piece);
                }
            }
            offset += file.length;
        }
        pieces
    }

    /// Pieces that must be downloaded again when `after` replaces `before`:
    /// every piece that overlaps a file that was skipped and is now selected.
    /// Such a piece may already be complete, but the bytes of the skipped file
    /// were discarded when it was written.
    pub fn pieces_to_refetch(
        files: &[TorrentFile],
        before: &[bool],
        after: &[bool],
        piece_length: u32,
        total_pieces: usize,
    ) -> Bitfield {
        let newly_selected: Vec<bool> = before
            .iter()
            .zip(after)
            .map(|(was, now)| !*was && *now)
            .collect();
        Self::pieces_of_files(files, &newly_selected, piece_length, total_pieces)
    }

    /// True when everything is downloaded and stored.
    pub fn is_all(&self) -> bool {
        self.wanted.count() == self.wanted.len() && self.unservable.count() == 0
    }

    /// Bytes still to be fetched if no piece were complete.
    pub fn wanted_bytes(&self, piece_length: u32, total_length: u64) -> u64 {
        self.wanted
            .iter_ones()
            .map(|piece| {
                let start = piece as u64 * piece_length as u64;
                total_length.saturating_sub(start).min(piece_length as u64)
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(length: u64) -> TorrentFile {
        TorrentFile {
            length,
            path: vec!["f".into()],
        }
    }

    #[test]
    fn boundary_pieces_are_wanted_but_not_servable() {
        // Pieces of 10 bytes; files cover 0..15, 15..40 (so pieces 0..=3 overall).
        let files = [file(15), file(25)];
        let total_pieces = 4;

        let only_second = Selection::from_files(&files, &[false, true], 10, total_pieces);
        // File 2 spans pieces 1..=3; piece 0 only belongs to file 1.
        assert_eq!(
            only_second.wanted.iter_ones().collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // Pieces 0 and 1 touch the skipped first file.
        assert_eq!(
            only_second.unservable.iter_ones().collect::<Vec<_>>(),
            vec![0, 1]
        );

        let only_first = Selection::from_files(&files, &[true, false], 10, total_pieces);
        assert_eq!(
            only_first.wanted.iter_ones().collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            only_first.unservable.iter_ones().collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let both = Selection::from_files(&files, &[true, true], 10, total_pieces);
        assert!(both.is_all());
    }

    #[test]
    fn newly_selected_files_force_a_refetch_of_their_pieces() {
        let files = [file(15), file(25)];
        let refetch = Selection::pieces_to_refetch(&files, &[true, false], &[true, true], 10, 4);
        // File 2 covers pieces 1..=3.
        assert_eq!(refetch.iter_ones().collect::<Vec<_>>(), vec![1, 2, 3]);

        let none = Selection::pieces_to_refetch(&files, &[true, true], &[true, false], 10, 4);
        assert_eq!(none.count(), 0);
    }

    #[test]
    fn zero_length_files_are_ignored() {
        let files = [file(10), file(0), file(10)];
        let sel = Selection::from_files(&files, &[true, false, false], 10, 2);
        assert_eq!(sel.wanted.iter_ones().collect::<Vec<_>>(), vec![0]);
        assert_eq!(sel.unservable.iter_ones().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn wanted_bytes_accounts_for_the_short_last_piece() {
        let sel = Selection::all(3);
        assert_eq!(sel.wanted_bytes(10, 25), 25);
        let files = [file(10), file(15)];
        let last_only = Selection::from_files(&files, &[false, true], 10, 3);
        assert_eq!(last_only.wanted_bytes(10, 25), 15);
    }
}
