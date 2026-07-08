use tokio::net::TcpListener;

pub async fn bind_listener(addr: &str) -> tokio::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}
