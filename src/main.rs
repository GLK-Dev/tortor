use tortor::crypto::dispatch::{self, HashAlgorithm};

fn main() {
    let payload = b"torrent piece example";

    let sha1 = dispatch::hash_piece(payload, HashAlgorithm::Sha1);
    let sha256 = dispatch::hash_piece(payload, HashAlgorithm::Sha256);

    println!("Selected CPU backend: {:?}", dispatch::detect_backend());
    println!("SHA-1   : {}", hex::encode(sha1));
    println!("SHA-256 : {}", hex::encode(sha256));

    #[cfg(feature = "gpu")]
    {
        use tortor::crypto::gpu::{GpuHasher, StubGpuHasher};

        let gpu = StubGpuHasher::new();
        if let Err(err) = gpu.hash_sha256_batch(&[payload]) {
            println!("GPU path is scaffold-only for now: {err}");
        }
    }
}
