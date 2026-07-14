use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::info;

use crate::net::dht::routing::{Contact, NodeId, RoutingTable};
use crate::net::dht::search::DhtSearch;
use crate::net::dht::server::{DhtCommand, DhtServer};
use crate::net::swarm::SwarmEvent;

pub enum DhtManagerCommand {
    StartSearch(NodeId),
}

pub struct DhtManager {
    local_id: NodeId,
    routing_table: RoutingTable,
    cmd_rx: mpsc::Receiver<DhtManagerCommand>,
    server_cmd_tx: mpsc::Sender<DhtCommand>,
    swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
}

impl DhtManager {
    pub async fn new(
        port: u16,
        swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
    ) -> anyhow::Result<(Self, mpsc::Sender<DhtManagerCommand>)> {
        // Generate random local ID
        let mut local_id_bytes = [0u8; 20];
        for b in &mut local_id_bytes {
            *b = rand::random();
        }
        let local_id = NodeId(local_id_bytes);

        let (server, server_cmd_tx) = DhtServer::new(port).await?;
        tokio::spawn(server.run());

        let mut routing_table = RoutingTable::new(local_id);
        
        // Add bootstrap nodes
        if let Ok(Ok(addrs)) = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::net::lookup_host("router.bittorrent.com:6881")
        ).await {
            for addr in addrs {
                routing_table.insert(Contact {
                    id: NodeId([0; 20]), // Fake ID, will be replaced upon ping reply
                    addr,
                });
            }
        }

        let (cmd_tx, cmd_rx) = mpsc::channel(100);

        let manager = Self {
            local_id,
            routing_table,
            cmd_rx,
            server_cmd_tx,
            swarm_tx,
        };

        Ok((manager, cmd_tx))
    }

    pub async fn run(mut self) {
        info!("DhtManager started with node ID {:?}", self.local_id);
        
        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(DhtManagerCommand::StartSearch(info_hash)) => {
                            info!("DhtManager starting recursive search for {:?}", info_hash);
                            
                            // Extract up to K closest nodes from routing table.
                            // For simplicity, we just dump all nodes and sort them.
                            let mut all_nodes = Vec::new();
                            for bucket in &self.routing_table.buckets {
                                all_nodes.extend(bucket.nodes.iter().cloned());
                            }
                            all_nodes.sort_by_key(|c| c.id.xor(&info_hash));
                            let k_nodes: Vec<_> = all_nodes.into_iter().take(8).collect();
                            
                            let search = DhtSearch::new(
                                info_hash,
                                self.local_id,
                                k_nodes,
                                self.server_cmd_tx.clone(),
                                self.swarm_tx.clone(),
                            );
                            
                            tokio::spawn(search.run());
                        }
                        None => {
                            info!("DhtManager command channel closed.");
                            break;
                        }
                    }
                }
                // TODO: Receive feedback from search tasks or incoming UDP queries (Phase 4)
            }
        }
    }
}
