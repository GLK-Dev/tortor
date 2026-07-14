use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{debug, info};

use crate::net::dht::krpc::{KrpcMessage, QueryArgs};
use crate::net::dht::routing::{Contact, NodeId};
use crate::net::dht::server::DhtCommand;
use crate::net::swarm::SwarmEvent;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeState {
    Unqueried,
    InFlight,
    Queried,
    Failed,
}

#[derive(Clone, Debug)]
pub struct SearchContact {
    pub contact: Contact,
    pub state: NodeState,
}

pub struct DhtSearch {
    pub info_hash: NodeId,
    pub local_id: NodeId,
    pub short_list: Vec<SearchContact>,
    pub cmd_tx: mpsc::Sender<DhtCommand>,
    pub manager_tx: mpsc::Sender<crate::net::dht::actor::DhtManagerCommand>,
    pub swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
}

impl DhtSearch {
    pub fn new(
        info_hash: NodeId,
        local_id: NodeId,
        initial_nodes: Vec<Contact>,
        cmd_tx: mpsc::Sender<DhtCommand>,
        swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
        manager_tx: mpsc::Sender<crate::net::dht::actor::DhtManagerCommand>,
    ) -> Self {
        let mut short_list: Vec<SearchContact> = initial_nodes
            .into_iter()
            .map(|contact| SearchContact {
                contact,
                state: NodeState::Unqueried,
            })
            .collect();
            
        short_list.sort_by_key(|sc| sc.contact.id.xor(&info_hash));

        Self {
            info_hash,
            local_id,
            short_list,
            cmd_tx,
            swarm_tx,
            manager_tx,
        }
    }

    pub async fn run(mut self) {
        let mut in_flight = JoinSet::new();
        const ALPHA: usize = 3;
        const K: usize = 8;
        
        let mut tid_counter: u16 = 0;

        loop {
            while in_flight.len() < ALPHA {
                if let Some(idx) = self.short_list.iter().position(|sc| sc.state == NodeState::Unqueried) {
                    self.short_list[idx].state = NodeState::InFlight;
                    let target_contact = self.short_list[idx].contact.clone();
                    
                    tid_counter = tid_counter.wrapping_add(1);
                    let tid = tid_counter.to_be_bytes().to_vec();
                    
                    let mut msg = KrpcMessage::new_ping_query(tid, self.local_id.0.to_vec());
                    msg.q = Some("get_peers".to_string());
                    if let Some(args) = &mut msg.a {
                        args.info_hash = self.info_hash.0.to_vec();
                    }
                    
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let target_addr = target_contact.addr;
                    
                    if self.cmd_tx.send(DhtCommand::SendQuery {
                        target: target_addr,
                        msg,
                        reply: reply_tx,
                    }).await.is_ok() {
                        let timeout_future = tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx);
                        in_flight.spawn(async move {
                            (target_contact.id, timeout_future.await)
                        });
                    }
                } else {
                    break;
                }
            }

            if in_flight.is_empty() {
                debug!("DHT Search for {:?} complete (no more in-flight or unqueried nodes).", self.info_hash);
                break;
            }

            if let Some(Ok((node_id, result))) = in_flight.join_next().await {
                if let Some(sc) = self.short_list.iter_mut().find(|sc| sc.contact.id == node_id) {
                    match result {
                        Ok(Ok(Ok(response))) => {
                            sc.state = NodeState::Queried;
                            
                            if let Some(resp_args) = response.r {
                                if !resp_args.values.is_empty() {
                                    let mut peers = Vec::new();
                                    for peer_buf in resp_args.values {
                                        peers.extend(crate::net::pex::decode_compact_ipv4(peer_buf.as_ref()));
                                    }
                                    if !peers.is_empty() {
                                        info!("DHT found {} peers for {:?}", peers.len(), self.info_hash);
                                        let _ = self.swarm_tx.send(SwarmEvent::DhtPeersReceived(peers));
                                    }
                                }
                                
                                if !resp_args.nodes.is_empty() {
                                    for chunk in resp_args.nodes.chunks_exact(26) {
                                        let mut id = [0u8; 20];
                                        id.copy_from_slice(&chunk[0..20]);
                                        let addrs_part = crate::net::pex::decode_compact_ipv4(&chunk[20..26]);
                                        if let Some(addr) = addrs_part.into_iter().next() {
                                            let contact = Contact { id: NodeId(id), addr };
                                            
                                            let _ = self.manager_tx.send(crate::net::dht::actor::DhtManagerCommand::InsertNode(contact.clone())).await;
                                            if !self.short_list.iter().any(|existing| existing.contact.id == contact.id) {
                                                self.short_list.push(SearchContact {
                                                    contact,
                                                    state: NodeState::Unqueried,
                                                });
                                            }
                                        }
                                    }
                                    self.short_list.sort_by_key(|c| c.contact.id.xor(&self.info_hash));
                                    self.short_list.truncate(50);
                                }
                            }
                        }
                        _ => {
                            sc.state = NodeState::Failed;
                        }
                    }
                }
            }
        }
    }
}
