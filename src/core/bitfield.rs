/// Fixed-size set of piece indices, stored in BitTorrent wire order
/// (most significant bit of the first byte is piece 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitfield {
    bytes: Vec<u8>,
    len: usize,
}

impl Bitfield {
    pub fn new(len: usize) -> Self {
        Self {
            bytes: vec![0u8; len.div_ceil(8)],
            len,
        }
    }

    /// Parses a wire BITFIELD payload. Rejects a wrong size or set spare bits.
    pub fn from_wire(len: usize, payload: &[u8]) -> Option<Self> {
        if payload.len() != len.div_ceil(8) {
            return None;
        }
        let spare = payload.len() * 8 - len;
        if spare > 0 {
            let last = *payload.last()?;
            if last & ((1u8 << spare) - 1) != 0 {
                return None;
            }
        }
        Some(Self {
            bytes: payload.to_vec(),
            len,
        })
    }

    pub fn from_indices(len: usize, indices: impl IntoIterator<Item = u32>) -> Self {
        let mut field = Self::new(len);
        for index in indices {
            field.set(index as usize);
        }
        field
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn has(&self, index: usize) -> bool {
        index < self.len && self.bytes[index / 8] & (0x80 >> (index % 8)) != 0
    }

    /// Returns `false` when `index` is out of range.
    pub fn set(&mut self, index: usize) -> bool {
        if index >= self.len {
            return false;
        }
        self.bytes[index / 8] |= 0x80 >> (index % 8);
        true
    }

    pub fn count(&self) -> usize {
        self.bytes.iter().map(|b| b.count_ones() as usize).sum()
    }

    pub fn iter_ones(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(|&i| self.has(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_query_bits_in_wire_order() {
        let mut bf = Bitfield::new(10);
        assert!(bf.set(0));
        assert!(bf.set(9));
        assert!(!bf.set(10));
        assert_eq!(bf.as_bytes(), &[0x80, 0x40]);
        assert!(bf.has(0) && bf.has(9) && !bf.has(1) && !bf.has(10));
        assert_eq!(bf.count(), 2);
        assert_eq!(bf.iter_ones().collect::<Vec<_>>(), vec![0, 9]);
    }

    #[test]
    fn from_wire_validates_size_and_spare_bits() {
        assert!(Bitfield::from_wire(10, &[0xFF]).is_none());
        assert!(Bitfield::from_wire(10, &[0xFF, 0xC1]).is_none());
        let bf = Bitfield::from_wire(10, &[0xFF, 0xC0]).unwrap();
        assert_eq!(bf.count(), 10);
        assert!(Bitfield::from_wire(8, &[0xFF]).is_some());
        assert!(Bitfield::from_wire(0, &[]).is_some());
    }
}
