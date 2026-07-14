use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, Level};
use tortor::net::dht::actor::{DhtManager, DhtManagerCommand};
use tortor::net::dht::routing::NodeId;
use tortor::net::swarm::SwarmEvent;

fn parse_hex(s: &str) -> [u8; 20] {
    let mut out = [0u8; 20];
    let bytes = s.as_bytes();
    for i in 0..20 {
        let high = (bytes[i*2] as char).to_digit(16).unwrap() as u8;
        let low = (bytes[i*2+1] as char).to_digit(16).unwrap() as u8;
        out[i] = (high << 4) | low;
    }
    out
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .init();

    info!("Starting DHT Test...");
    
    // Ubuntu 24.04 Desktop info hash
    let info_hash_hex = "4344503b7e5c3e03c40c313a072fa69c0d9a6c76";
    let info_hash = NodeId(parse_hex(info_hash_hex));

    let (swarm_tx, mut swarm_rx) = mpsc::unbounded_channel::<SwarmEvent>();
    
    let (dht_manager, dht_cmd_tx) = DhtManager::new(0, swarm_tx).await?;
    tokio::spawn(dht_manager.run());
    
    // Wait for bootstrap ping to finish (giving it a bit of time)
    tokio::time::sleep(Duration::from_secs(1)).await;
    
    dht_cmd_tx.send(DhtManagerCommand::StartSearch(info_hash)).await?;
    
    let timeout = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(timeout);
    
    loop {
        tokio::select! {
            Some(event) = swarm_rx.recv() => {
                if let SwarmEvent::DhtPeersReceived(peers) = event {
                    info!("SUCCESS: Received {} peers from DHT!", peers.len());
                    for peer in peers.iter().take(5) {
                        info!("Peer: {}", peer);
                    }
                    if peers.len() > 0 {
                        info!("Test completed successfully.");
                        return Ok(());
                    }
                }
            }
            _ = &mut timeout => {
                info!("Timeout reached without receiving peers.");
                return Ok(());
            }
        }
    }
}
