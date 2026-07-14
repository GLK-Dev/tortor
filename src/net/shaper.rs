use std::pin::Pin;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct GlobalShaper;

// Tokens (bytes) available. We use isize to allow borrowing "ahead" slightly
static DOWNLOAD_TOKENS: AtomicIsize = AtomicIsize::new(0);
static UPLOAD_TOKENS: AtomicIsize = AtomicIsize::new(0);

// Global limits in bytes per second. 0 means unlimited.
static DOWNLOAD_LIMIT: AtomicIsize = AtomicIsize::new(0);
static UPLOAD_LIMIT: AtomicIsize = AtomicIsize::new(0);

impl GlobalShaper {
    pub fn set_limits(download_bps: usize, upload_bps: usize) {
        DOWNLOAD_LIMIT.store(download_bps as isize, Ordering::Relaxed);
        UPLOAD_LIMIT.store(upload_bps as isize, Ordering::Relaxed);
        
        // Reset tokens
        DOWNLOAD_TOKENS.store(download_bps as isize, Ordering::Relaxed);
        UPLOAD_TOKENS.store(upload_bps as isize, Ordering::Relaxed);
    }

    /// Run this in a background task (e.g. every 10ms)
    pub async fn run_replenisher() {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));
        loop {
            interval.tick().await;
            
            let dl_limit = DOWNLOAD_LIMIT.load(Ordering::Relaxed);
            if dl_limit > 0 {
                // Add tokens for 10ms (1/100 of a second)
                let add = dl_limit / 100;
                let current = DOWNLOAD_TOKENS.load(Ordering::Relaxed);
                if current < dl_limit {
                    DOWNLOAD_TOKENS.fetch_add(add, Ordering::Relaxed);
                }
            }

            let ul_limit = UPLOAD_LIMIT.load(Ordering::Relaxed);
            if ul_limit > 0 {
                let add = ul_limit / 100;
                let current = UPLOAD_TOKENS.load(Ordering::Relaxed);
                if current < ul_limit {
                    UPLOAD_TOKENS.fetch_add(add, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn acquire_download(amount: usize) -> usize {
        let limit = DOWNLOAD_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return amount;
        }

        let mut current = DOWNLOAD_TOKENS.load(Ordering::Relaxed);
        loop {
            if current <= 0 {
                return 0; // No tokens available right now
            }
            let to_take = std::cmp::min(current, amount as isize);
            match DOWNLOAD_TOKENS.compare_exchange_weak(
                current,
                current - to_take,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return to_take as usize,
                Err(val) => current = val,
            }
        }
    }

    pub fn acquire_upload(amount: usize) -> usize {
        let limit = UPLOAD_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return amount;
        }

        let mut current = UPLOAD_TOKENS.load(Ordering::Relaxed);
        loop {
            if current <= 0 {
                return 0;
            }
            let to_take = std::cmp::min(current, amount as isize);
            match UPLOAD_TOKENS.compare_exchange_weak(
                current,
                current - to_take,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return to_take as usize,
                Err(val) => current = val,
            }
        }
    }
}

pub struct ShapedStream<T> {
    inner: T,
}

impl<T> ShapedStream<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
    
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for ShapedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let limit = DOWNLOAD_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return Pin::new(&mut self.inner).poll_read(cx, buf);
        }

        let requested = buf.remaining();
        let allowed = GlobalShaper::acquire_download(requested);

        if allowed == 0 {
            // Waker logic to try again soon. For a simple shaper, we can just wake immediately or rely on timer.
            // A more robust approach uses a channel or standard tokio time, but for token bucket, waking after small delay is okay.
            let waker = cx.waker().clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                waker.wake();
            });
            return Poll::Pending;
        }

        // Limit the read buffer to  llowed
        let mut sub_buf = buf.take(allowed);
        match Pin::new(&mut self.inner).poll_read(cx, &mut sub_buf) {
            Poll::Ready(Ok(())) => {
                let bytes_read = sub_buf.filled().len();
                let unused = allowed - bytes_read;
                if unused > 0 {
                    DOWNLOAD_TOKENS.fetch_add(unused as isize, Ordering::Relaxed);
                }
                // Advance the original buffer by the amount read
                unsafe {
                    buf.assume_init(bytes_read);
                }
                buf.advance(bytes_read);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => {
                DOWNLOAD_TOKENS.fetch_add(allowed as isize, Ordering::Relaxed);
                Poll::Ready(Err(e))
            }
            Poll::Pending => {
                DOWNLOAD_TOKENS.fetch_add(allowed as isize, Ordering::Relaxed);
                Poll::Pending
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
        let limit = UPLOAD_LIMIT.load(Ordering::Relaxed);
        if limit == 0 {
            return Pin::new(&mut self.inner).poll_write(cx, buf);
        }

        let requested = buf.len();
        let allowed = GlobalShaper::acquire_upload(requested);

        if allowed == 0 {
            let waker = cx.waker().clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                waker.wake();
            });
            return Poll::Pending;
        }

        let slice = &buf[..allowed];
        match Pin::new(&mut self.inner).poll_write(cx, slice) {
            Poll::Ready(Ok(n)) => {
                let unused = allowed - n;
                if unused > 0 {
                    UPLOAD_TOKENS.fetch_add(unused as isize, Ordering::Relaxed);
                }
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(e)) => {
                UPLOAD_TOKENS.fetch_add(allowed as isize, Ordering::Relaxed);
                Poll::Ready(Err(e))
            }
            Poll::Pending => {
                UPLOAD_TOKENS.fetch_add(allowed as isize, Ordering::Relaxed);
                Poll::Pending
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
