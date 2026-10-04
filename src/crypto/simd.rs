use crate::crypto::core;

/// # Safety
/// The caller must have verified at runtime that the CPU supports SSE4.1.
#[target_feature(enable = "sse4.1")]
pub unsafe fn hash_sha1_sse41(data: &[u8]) -> [u8; 20] {
    core::hash_sha1(data)
}

/// # Safety
/// The caller must have verified at runtime that the CPU supports SSE4.1.
#[target_feature(enable = "sse4.1")]
pub unsafe fn hash_sha256_sse41(data: &[u8]) -> [u8; 32] {
    core::hash_sha256(data)
}

/// # Safety
/// The caller must have verified at runtime that the CPU supports AVX2.
#[target_feature(enable = "avx2")]
pub unsafe fn hash_sha1_avx2(data: &[u8]) -> [u8; 20] {
    core::hash_sha1(data)
}

/// # Safety
/// The caller must have verified at runtime that the CPU supports AVX2.
#[target_feature(enable = "avx2")]
pub unsafe fn hash_sha256_avx2(data: &[u8]) -> [u8; 32] {
    core::hash_sha256(data)
}
