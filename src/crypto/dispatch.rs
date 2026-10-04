use crate::crypto::core;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HashAlgorithm {
    Sha1,
    Sha256,
}

/// Hashes a piece. The `sha1` and `sha2` crates already pick the fastest
/// implementation for the CPU at runtime (SHA-NI / AVX2 where available), so
/// no dispatch is needed here.
pub fn hash_piece(data: &[u8], algorithm: HashAlgorithm) -> Vec<u8> {
    match algorithm {
        HashAlgorithm::Sha1 => core::hash_sha1(data).to_vec(),
        HashAlgorithm::Sha256 => core::hash_sha256(data).to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_piece_matches_core_sha1() {
        let payload = b"piece-data-1";
        assert_eq!(
            hash_piece(payload, HashAlgorithm::Sha1),
            core::hash_sha1(payload).to_vec()
        );
    }

    #[test]
    fn hash_piece_matches_core_sha256() {
        let payload = b"piece-data-2";
        assert_eq!(
            hash_piece(payload, HashAlgorithm::Sha256),
            core::hash_sha256(payload).to_vec()
        );
    }
}
