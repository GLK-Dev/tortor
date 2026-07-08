use std::time::{Duration, Instant};

const BLOCK_SIZE: u32 = 16 * 1024;

#[derive(Debug)]
pub enum AssemblerState {
    InProgress,
    Complete(Vec<u8>),
    Error(String),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BlockClass {
    ExpectedNew,
    Duplicate,
    Unexpected,
}

#[derive(Debug, Clone)]
pub struct BlockRequest {
    pub begin: u32,
    pub length: u32,
    pub requested_at: Option<Instant>,
    pub completed: bool,
}

#[derive(Debug, Clone)]
pub struct PieceAssembler {
    pub piece_index: u32,
    pub expected_length: u32,
    buffer: Vec<u8>,
    blocks: Vec<BlockRequest>,
    received_bytes: u32,
}

impl PieceAssembler {
    pub fn new(piece_index: u32, expected_length: u32) -> Self {
        let num_blocks = if expected_length == 0 {
            0
        } else {
            (expected_length + BLOCK_SIZE - 1) / BLOCK_SIZE
        };

        let mut blocks = Vec::with_capacity(num_blocks as usize);
        for i in 0..num_blocks {
            let begin = i * BLOCK_SIZE;
            let length = std::cmp::min(BLOCK_SIZE, expected_length - begin);
            blocks.push(BlockRequest {
                begin,
                length,
                requested_at: None,
                completed: false,
            });
        }

        Self {
            piece_index,
            expected_length,
            buffer: vec![0u8; expected_length as usize],
            blocks,
            received_bytes: 0,
        }
    }

    pub fn in_flight_count(&self, timeout: Duration) -> usize {
        let now = Instant::now();
        self.blocks
            .iter()
            .filter(|b| {
                !b.completed
                    && b.requested_at
                        .map(|t| now.duration_since(t) <= timeout)
                        .unwrap_or(false)
            })
            .count()
    }

    pub fn next_request(&mut self, timeout: Duration) -> Option<(u32, u32, bool)> {
        let now = Instant::now();
        for block in self.blocks.iter_mut().filter(|b| !b.completed) {
            match block.requested_at {
                None => {
                    block.requested_at = Some(now);
                    return Some((block.begin, block.length, false));
                }
                Some(req_time) if now.duration_since(req_time) > timeout => {
                    block.requested_at = Some(now);
                    return Some((block.begin, block.length, true));
                }
                _ => continue,
            }
        }
        None
    }

    pub fn classify_block(&self, begin: u32, data_len: u32) -> BlockClass {
        if begin + data_len > self.expected_length {
            return BlockClass::Unexpected;
        }

        for block in &self.blocks {
            if block.begin == begin && block.length == data_len {
                if block.completed {
                    return BlockClass::Duplicate;
                }
                return BlockClass::ExpectedNew;
            }
        }

        BlockClass::Unexpected
    }

    pub fn received_bytes(&self) -> u32 {
        self.received_bytes
    }

    pub fn add_block(&mut self, begin: u32, data: &[u8]) -> AssemblerState {
        if begin + data.len() as u32 > self.expected_length {
            return AssemblerState::Error("block exceeds piece boundary".to_string());
        }

        let mut found = false;
        for block in &mut self.blocks {
            if block.begin == begin && block.length == data.len() as u32 {
                if block.completed {
                    return AssemblerState::InProgress;
                }
                block.completed = true;
                found = true;
                break;
            }
        }

        if !found {
            return AssemblerState::Error("received unexpected block".to_string());
        }

        let begin_usize = begin as usize;
        self.buffer[begin_usize..begin_usize + data.len()].copy_from_slice(data);
        self.received_bytes += data.len() as u32;

        if self.received_bytes == self.expected_length {
            AssemblerState::Complete(self.buffer.clone())
        } else {
            AssemblerState::InProgress
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembler_accepts_out_of_order_blocks() {
        let mut a = PieceAssembler::new(0, 32768);
        let state1 = a.add_block(16384, &[2u8; 16384]);
        assert!(matches!(state1, AssemblerState::InProgress));

        let state2 = a.add_block(0, &[1u8; 16384]);
        match state2 {
            AssemblerState::Complete(buf) => {
                assert_eq!(buf.len(), 32768);
                assert_eq!(buf[0], 1);
                assert_eq!(buf[16384], 2);
            }
            _ => panic!("expected complete state"),
        }
    }

    #[test]
    fn assembler_ignores_duplicate_block() {
        let mut a = PieceAssembler::new(0, 16384);
        let _ = a.add_block(0, &[9u8; 16384]);
        let state = a.add_block(0, &[9u8; 16384]);
        assert!(matches!(state, AssemblerState::InProgress));
    }
}
