use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub enum GpuHashError {
    Unavailable(&'static str),
}

impl Display for GpuHashError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for GpuHashError {}

pub trait GpuHasher {
    fn hash_sha256_batch(&self, pieces: &[&[u8]]) -> Result<Vec<[u8; 32]>, GpuHashError>;
}

pub struct StubGpuHasher;

impl StubGpuHasher {
    pub fn new() -> Self {
        Self
    }
}

impl GpuHasher for StubGpuHasher {
    fn hash_sha256_batch(&self, _pieces: &[&[u8]]) -> Result<Vec<[u8; 32]>, GpuHashError> {
        Err(GpuHashError::Unavailable(
            "GPU backend is not implemented yet. Enable this feature and wire wgpu/compute kernels.",
        ))
    }
}