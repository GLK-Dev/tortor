//! Checks whether a UPnP router can be found and forwards a port.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, watch};
use tortor::net::portmap::{run_upnp, PortMapStatus};

#[tokio::main]
async fn main() {
    let (status_tx, mut status_rx) = watch::channel(PortMapStatus::Searching);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let task = tokio::spawn(run_upnp(16881, Arc::new(status_tx), shutdown_rx));

    let _ = tokio::time::timeout(Duration::from_secs(25), async {
        while status_rx.changed().await.is_ok() {
            let status = status_rx.borrow().clone();
            println!("{status:?}");
            if !matches!(status, PortMapStatus::Searching) {
                break;
            }
        }
    })
    .await;

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}
