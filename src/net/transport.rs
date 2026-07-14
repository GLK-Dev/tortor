use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

#[derive(Debug)]
pub enum PeerStream {
    Tcp(TcpStream),
    Quic(quinn::SendStream, quinn::RecvStream),
}

impl AsyncRead for PeerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            PeerStream::Tcp(ref mut stream) => Pin::new(stream).poll_read(cx, buf),
            PeerStream::Quic(_, ref mut recv_stream) => match Pin::new(recv_stream).poll_read(cx, buf) {
                Poll::Ready(Ok(r)) => Poll::Ready(Ok(r)),
                Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e))),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

impl AsyncWrite for PeerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            PeerStream::Tcp(ref mut stream) => Pin::new(stream).poll_write(cx, buf),
            PeerStream::Quic(ref mut send_stream, _) => match Pin::new(send_stream).poll_write(cx, buf) {
                Poll::Ready(Ok(r)) => Poll::Ready(Ok(r)),
                Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e))),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            PeerStream::Tcp(ref mut stream) => Pin::new(stream).poll_flush(cx),
            PeerStream::Quic(ref mut send_stream, _) => match Pin::new(send_stream).poll_flush(cx) {
                Poll::Ready(Ok(r)) => Poll::Ready(Ok(r)),
                Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e))),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            PeerStream::Tcp(ref mut stream) => Pin::new(stream).poll_shutdown(cx),
            PeerStream::Quic(ref mut send_stream, _) => match Pin::new(send_stream).poll_shutdown(cx) {
                Poll::Ready(Ok(r)) => Poll::Ready(Ok(r)),
                Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e))),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
