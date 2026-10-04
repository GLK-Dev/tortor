//! Global bandwidth limiting: a token bucket per direction and a stream
//! wrapper that waits for tokens instead of polling.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// Smallest slice worth waiting for; avoids a stream of one-byte reads.
const MIN_CHUNK: usize = 4096;
/// Burst size: a quarter second of traffic, at least one chunk.
const MIN_BURST: f64 = 16.0 * 1024.0;

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token bucket for one direction. A rate of 0 bytes/s means unlimited.
pub struct Limiter {
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

impl Limiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: AtomicU64::new(bytes_per_sec),
            bucket: Mutex::new(Bucket {
                tokens: burst(bytes_per_sec),
                last: Instant::now(),
            }),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
        let mut bucket = self.bucket.lock().unwrap();
        bucket.tokens = bucket.tokens.min(burst(bytes_per_sec));
        bucket.last = Instant::now();
    }

    /// Takes up to `want` bytes. When the bucket is empty returns how long to
    /// wait until a useful amount is available.
    pub fn acquire(&self, want: usize) -> Result<usize, Duration> {
        self.acquire_at(want, Instant::now())
    }

    fn acquire_at(&self, want: usize, now: Instant) -> Result<usize, Duration> {
        let rate = self.rate();
        if rate == 0 || want == 0 {
            return Ok(want);
        }

        let mut bucket = self.bucket.lock().unwrap();
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.last = now;
        bucket.tokens = (bucket.tokens + elapsed * rate as f64).min(burst(rate));

        let need = want.min(MIN_CHUNK) as f64;
        if bucket.tokens < need {
            let wait = (need - bucket.tokens) / rate as f64;
            return Err(Duration::from_secs_f64(wait).max(Duration::from_millis(1)));
        }

        let take = (bucket.tokens as usize).min(want);
        bucket.tokens -= take as f64;
        Ok(take)
    }

    /// Returns tokens that were taken but not used.
    pub fn give_back(&self, bytes: usize) {
        let rate = self.rate();
        if rate == 0 || bytes == 0 {
            return;
        }
        let mut bucket = self.bucket.lock().unwrap();
        bucket.tokens = (bucket.tokens + bytes as f64).min(burst(rate));
    }
}

fn burst(rate: u64) -> f64 {
    (rate as f64 / 4.0).max(MIN_BURST)
}

/// Download and upload limiters shared by every connection.
pub struct Shaper {
    pub download: Limiter,
    pub upload: Limiter,
}

impl Shaper {
    pub fn new(download_bps: u64, upload_bps: u64) -> Self {
        Self {
            download: Limiter::new(download_bps),
            upload: Limiter::new(upload_bps),
        }
    }
}

static GLOBAL: OnceLock<Arc<Shaper>> = OnceLock::new();

pub struct GlobalShaper;

impl GlobalShaper {
    pub fn get() -> Arc<Shaper> {
        GLOBAL.get_or_init(|| Arc::new(Shaper::new(0, 0))).clone()
    }

    /// Sets the application-wide limits in bytes per second (0 = unlimited).
    pub fn set_limits(download_bps: u64, upload_bps: u64) {
        let shaper = Self::get();
        shaper.download.set_rate(download_bps);
        shaper.upload.set_rate(upload_bps);
    }
}

/// Wraps a stream so reads and writes draw from a [`Shaper`].
pub struct ShapedStream<T> {
    inner: T,
    shaper: Arc<Shaper>,
    read_delay: Option<Pin<Box<Sleep>>>,
    write_delay: Option<Pin<Box<Sleep>>>,
}

impl<T> ShapedStream<T> {
    /// Uses the application-wide limits.
    pub fn new(inner: T) -> Self {
        Self::with_shaper(inner, GlobalShaper::get())
    }

    pub fn with_shaper(inner: T, shaper: Arc<Shaper>) -> Self {
        Self {
            inner,
            shaper,
            read_delay: None,
            write_delay: None,
        }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

/// Waits out a pending delay. `Ready` means the caller may try again.
fn poll_delay(delay: &mut Option<Pin<Box<Sleep>>>, cx: &mut Context<'_>) -> Poll<()> {
    if let Some(sleep) = delay.as_mut() {
        if sleep.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        *delay = None;
    }
    Poll::Ready(())
}

fn start_delay(delay: &mut Option<Pin<Box<Sleep>>>, wait: Duration, cx: &mut Context<'_>) {
    let mut sleep = Box::pin(tokio::time::sleep(wait));
    // Polling once registers the waker for the timer.
    if sleep.as_mut().poll(cx).is_pending() {
        *delay = Some(sleep);
    } else {
        cx.waker().wake_by_ref();
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for ShapedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        let shaper = this.shaper.clone();
        let limiter = &shaper.download;
        if limiter.rate() == 0 {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        }
        if poll_delay(&mut this.read_delay, cx).is_pending() {
            return Poll::Pending;
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        let allowed = match limiter.acquire(buf.remaining()) {
            Ok(allowed) => allowed,
            Err(wait) => {
                start_delay(&mut this.read_delay, wait, cx);
                return Poll::Pending;
            }
        };

        let mut limited = ReadBuf::new(buf.initialize_unfilled_to(allowed));
        match Pin::new(&mut this.inner).poll_read(cx, &mut limited) {
            Poll::Ready(Ok(())) => {
                let read = limited.filled().len();
                buf.advance(read);
                limiter.give_back(allowed - read);
                Poll::Ready(Ok(()))
            }
            other => {
                limiter.give_back(allowed);
                other
            }
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for ShapedStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = &mut *self;
        let shaper = this.shaper.clone();
        let limiter = &shaper.upload;
        if limiter.rate() == 0 {
            return Pin::new(&mut this.inner).poll_write(cx, buf);
        }
        if poll_delay(&mut this.write_delay, cx).is_pending() {
            return Poll::Pending;
        }

        let allowed = match limiter.acquire(buf.len()) {
            Ok(allowed) => allowed,
            Err(wait) => {
                start_delay(&mut this.write_delay, wait, cx);
                return Poll::Pending;
            }
        };

        match Pin::new(&mut this.inner).poll_write(cx, &buf[..allowed]) {
            Poll::Ready(Ok(written)) => {
                limiter.give_back(allowed - written);
                Poll::Ready(Ok(written))
            }
            other => {
                limiter.give_back(allowed);
                other
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn unlimited_never_waits() {
        let limiter = Limiter::new(0);
        assert_eq!(limiter.acquire(1 << 20), Ok(1 << 20));
    }

    #[test]
    fn bucket_drains_then_refills_with_time() {
        let limiter = Limiter::new(100_000); // burst = 25_000
        let start = Instant::now();

        assert_eq!(limiter.acquire_at(100_000, start), Ok(25_000));
        let wait = limiter.acquire_at(10_000, start).unwrap_err();
        // 4096 bytes are needed at 100 kB/s: about 41 ms.
        assert!(
            wait >= Duration::from_millis(40) && wait <= Duration::from_millis(42),
            "{wait:?}"
        );

        let later = start + Duration::from_millis(100);
        assert_eq!(limiter.acquire_at(100_000, later), Ok(10_000));
    }

    #[test]
    fn unused_tokens_can_be_returned() {
        let limiter = Limiter::new(100_000);
        let start = Instant::now();
        assert_eq!(limiter.acquire_at(20_000, start), Ok(20_000));
        limiter.give_back(15_000);
        assert_eq!(limiter.acquire_at(25_000, start), Ok(20_000));
    }

    #[tokio::test]
    async fn writes_are_throttled_to_the_configured_rate() {
        let shaper = Arc::new(Shaper::new(0, 64 * 1024));
        let (writer, mut reader) = tokio::io::duplex(1 << 20);
        let mut shaped = ShapedStream::with_shaper(writer, shaper);

        let drain = tokio::spawn(async move {
            let mut total = 0usize;
            let mut buf = [0u8; 8192];
            while total < 48 * 1024 {
                total += reader.read(&mut buf).await.unwrap();
            }
        });

        let started = Instant::now();
        shaped.write_all(&vec![7u8; 48 * 1024]).await.unwrap();
        drain.await.unwrap();
        let elapsed = started.elapsed();

        // 16 KiB burst is free, the other 32 KiB take 0.5 s at 64 KiB/s.
        assert!(
            elapsed >= Duration::from_millis(400),
            "too fast: {elapsed:?}"
        );
        assert!(elapsed < Duration::from_secs(3), "too slow: {elapsed:?}");
    }

    #[tokio::test]
    async fn reads_are_throttled_and_data_is_intact() {
        let shaper = Arc::new(Shaper::new(64 * 1024, 0));
        let (mut writer, reader) = tokio::io::duplex(1 << 20);
        let mut shaped = ShapedStream::with_shaper(reader, shaper);

        let data: Vec<u8> = (0..48 * 1024).map(|i| (i % 251) as u8).collect();
        writer.write_all(&data).await.unwrap();
        drop(writer);

        let started = Instant::now();
        let mut received = Vec::new();
        shaped.read_to_end(&mut received).await.unwrap();

        assert_eq!(received, data);
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "{:?}",
            started.elapsed()
        );
    }
}
