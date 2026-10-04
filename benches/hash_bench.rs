use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use tortor::crypto::dispatch::{hash_piece, HashAlgorithm};

fn hash_bench(c: &mut Criterion) {
    let mut payload = vec![0u8; 256 * 1024];
    for (i, b) in payload.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }

    let mut group = c.benchmark_group("piece_hash_256kb");
    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("sha1", |b| {
        b.iter(|| hash_piece(black_box(&payload), HashAlgorithm::Sha1));
    });

    group.bench_function("sha256", |b| {
        b.iter(|| hash_piece(black_box(&payload), HashAlgorithm::Sha256));
    });

    group.finish();
}

criterion_group!(benches, hash_bench);
criterion_main!(benches);
