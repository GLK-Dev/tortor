use criterion::{black_box, criterion_group, criterion_main, Criterion};
use torrent_core::crypto_core;
use torrent_core::crypto_dispatch::{self, HashAlgorithm};

fn crypto_bench(c: &mut Criterion) {
    let mut payload = vec![0u8; 256 * 1024];
    for (i, b) in payload.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }

    let mut group = c.benchmark_group("piece_hash_256kb");

    group.bench_function("dispatch_sha1", |b| {
        b.iter(|| crypto_dispatch::hash_piece(black_box(&payload), HashAlgorithm::Sha1));
    });

    group.bench_function("portable_sha1", |b| {
        b.iter(|| crypto_core::hash_sha1(black_box(&payload)));
    });

    group.bench_function("dispatch_sha256", |b| {
        b.iter(|| crypto_dispatch::hash_piece(black_box(&payload), HashAlgorithm::Sha256));
    });

    group.bench_function("portable_sha256", |b| {
        b.iter(|| crypto_core::hash_sha256(black_box(&payload)));
    });

    group.finish();
}

criterion_group!(benches, crypto_bench);
criterion_main!(benches);