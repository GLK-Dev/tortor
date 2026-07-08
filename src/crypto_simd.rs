use crate::crypto_core;

#[target_feature(enable = "sse4.1")]
pub unsafe fn hash_sha1_sse41(data: &[u8]) -> [u8; 20] {
    crypto_core::hash_sha1(data)
}

#[target_feature(enable = "sse4.1")]
pub unsafe fn hash_sha256_sse41(data: &[u8]) -> [u8; 32] {
    crypto_core::hash_sha256(data)
}

#[target_feature(enable = "avx2")]
pub unsafe fn hash_sha1_avx2(data: &[u8]) -> [u8; 20] {
    crypto_core::hash_sha1(data)
}

#[target_feature(enable = "avx2")]
pub unsafe fn hash_sha256_avx2(data: &[u8]) -> [u8; 32] {
    crypto_core::hash_sha256(data)
}