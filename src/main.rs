use torrent_core::crypto_dispatch::{self, HashAlgorithm};

fn main() {
    let payload = b"torrent piece example";

    let sha1 = crypto_dispatch::hash_piece(payload, HashAlgorithm::Sha1);
    let sha256 = crypto_dispatch::hash_piece(payload, HashAlgorithm::Sha256);

    println!("Selected CPU backend: {:?}", crypto_dispatch::detect_backend());
    println!("SHA-1   : {}", hex::encode(sha1));
    println!("SHA-256 : {}", hex::encode(sha256));

    #[cfg(feature = "gpu")]
    {
        use torrent_core::gpu::{GpuHasher, StubGpuHasher};

        let gpu = StubGpuHasher::new();
        if let Err(err) = gpu.hash_sha256_batch(&[payload]) {
            println!("GPU path is scaffold-only for now: {err}");
        }
    }
}
