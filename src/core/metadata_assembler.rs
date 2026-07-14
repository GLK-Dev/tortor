use std::collections::HashSet;
use anyhow::{bail, Result};

pub struct MetadataAssembler {
    metadata_size: usize,
    buffer: Vec<u8>,
    received_pieces: HashSet<u32>,
    total_pieces: u32,
}

impl MetadataAssembler {
    pub const PIECE_SIZE: usize = 16384;

    pub fn new(metadata_size: usize) -> Self {
        let total_pieces = (metadata_size + Self::PIECE_SIZE - 1) / Self::PIECE_SIZE;
        Self {
            metadata_size,
            buffer: vec![0u8; metadata_size],
            received_pieces: HashSet::new(),
            total_pieces: total_pieces as u32,
        }
    }

    pub fn add_piece(&mut self, piece: u32, data: &[u8]) -> Result<bool> {
        if piece >= self.total_pieces {
            bail!("Piece index out of bounds");
        }
        
        let start = (piece as usize) * Self::PIECE_SIZE;
        let mut end = start + Self::PIECE_SIZE;
        if end > self.metadata_size {
            end = self.metadata_size;
        }

        if data.len() != end - start {
            bail!("Invalid metadata piece size: expected {}, got {}", end - start, data.len());
        }

        self.buffer[start..end].copy_from_slice(data);
        self.received_pieces.insert(piece);

        Ok(self.is_complete())
    }

    pub fn is_complete(&self) -> bool {
        self.received_pieces.len() == self.total_pieces as usize
    }

    pub fn get_buffer(&self) -> &[u8] {
        &self.buffer
    }

    pub fn next_missing_piece(&self) -> Option<u32> {
        for i in 0..self.total_pieces {
            if !self.received_pieces.contains(&i) {
                return Some(i);
            }
        }
        None
    }
}
