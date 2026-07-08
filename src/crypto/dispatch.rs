use crate::crypto::core;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HashAlgorithm {
    Sha1,
    Sha256,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CpuBackend {
    Avx2,
    Sse41,
    Portable,
}

pub fn detect_backend() -> CpuBackend {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("avx2") {
            return CpuBackend::Avx2;
        }

        if std::is_x86_feature_detected!("sse4.1") {
            return CpuBackend::Sse41;
        }
    }

    CpuBackend::Portable
}

pub fn hash_piece(data: &[u8], algorithm: HashAlgorithm) -> Vec<u8> {
    match detect_backend() {
        CpuBackend::Avx2 => hash_with_avx2(data, algorithm),
        CpuBackend::Sse41 => hash_with_sse41(data, algorithm),
        CpuBackend::Portable => hash_with_portable(data, algorithm),
    }
}

fn hash_with_portable(data: &[u8], algorithm: HashAlgorithm) -> Vec<u8> {
    match algorithm {
        HashAlgorithm::Sha1 => core::hash_sha1(data).to_vec(),
        HashAlgorithm::Sha256 => core::hash_sha256(data).to_vec(),
    }
}

fn hash_with_sse41(data: &[u8], algorithm: HashAlgorithm) -> Vec<u8> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        match algorithm {
            HashAlgorithm::Sha1 => {
                // Safety: this branch is selected only after runtime CPU feature check.
                unsafe { crate::crypto::simd::hash_sha1_sse41(data).to_vec() }
            }
            HashAlgorithm::Sha256 => {
                // Safety: this branch is selected only after runtime CPU feature check.
                unsafe { crate::crypto::simd::hash_sha256_sse41(data).to_vec() }
            }
        }
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        hash_with_portable(data, algorithm)
    }
}

fn hash_with_avx2(data: &[u8], algorithm: HashAlgorithm) -> Vec<u8> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        match algorithm {
            HashAlgorithm::Sha1 => {
                // Safety: this branch is selected only after runtime CPU feature check.
                unsafe { crate::crypto::simd::hash_sha1_avx2(data).to_vec() }
            }
            HashAlgorithm::Sha256 => {
                // Safety: this branch is selected only after runtime CPU feature check.
                unsafe { crate::crypto::simd::hash_sha256_avx2(data).to_vec() }
            }
        }
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        hash_with_portable(data, algorithm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_matches_portable_sha1() {
        let payload = b"piece-data-1";
        let expected = core::hash_sha1(payload).to_vec();
        let got = hash_piece(payload, HashAlgorithm::Sha1);
        assert_eq!(got, expected);
    }

    #[test]
    fn dispatch_matches_portable_sha256() {
        let payload = b"piece-data-2";
        let expected = core::hash_sha256(payload).to_vec();
        let got = hash_piece(payload, HashAlgorithm::Sha256);
        assert_eq!(got, expected);
    }
}
