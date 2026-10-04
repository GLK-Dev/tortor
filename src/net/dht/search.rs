use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{debug, info};

use crate::net::dht::krpc::{KrpcMessage, QueryArgs};
use crate::net::dht::routing::{decode_compact_nodes, decode_compact_nodes6, Contact, NodeId};
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
    /// Write token from the node's get_peers reply, needed for announce_peer.
    pub token: Vec<u8>,
}

pub struct DhtSearch {
    pub info_hash: NodeId,
    pub local_id: NodeId,
    pub short_list: Vec<SearchContact>,
    pub cmd_tx: mpsc::Sender<DhtCommand>,
    pub manager_tx: mpsc::Sender<crate::net::dht::actor::DhtManagerCommand>,
    pub swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
    pub announce_port: Option<u16>,
}

impl DhtSearch {
    pub fn new(
        info_hash: NodeId,
        local_id: NodeId,
        initial_nodes: Vec<Contact>,
        cmd_tx: mpsc::Sender<DhtCommand>,
        swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
        manager_tx: mpsc::Sender<crate::net::dht::actor::DhtManagerCommand>,
        announce_port: Option<u16>,
    ) -> Self {
        let mut short_list: Vec<SearchContact> = initial_nodes
            .into_iter()
            .map(|contact| SearchContact {
                contact,
                state: NodeState::Unqueried,
                token: Vec::new(),
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
            announce_port,
        }
    }

    pub async fn run(mut self) {
        let mut in_flight = JoinSet::new();
        const ALPHA: usize = 3;

        let mut tid_counter: u16 = 0;

        loop {
            while in_flight.len() < ALPHA {
                if let Some(idx) = self
                    .short_list
                    .iter()
                    .position(|sc| sc.state == NodeState::Unqueried)
                {
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

                    if self
                        .cmd_tx
                        .send(DhtCommand::SendQuery {
                            target: target_addr,
                            msg,
                            reply: reply_tx,
                        })
                        .await
                        .is_ok()
                    {
                        let timeout_future =
                            tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx);
                        in_flight.spawn(async move { (target_addr, timeout_future.await) });
                    }
                } else {
                    break;
                }
            }

            if in_flight.is_empty() {
                debug!(
                    "DHT Search for {:?} complete (no more in-flight or unqueried nodes).",
                    self.info_hash
                );
                self.announce_to_closest().await;
                break;
            }

            if let Some(Ok((node_addr, result))) = in_flight.join_next().await {
                if let Some(sc) = self
                    .short_list
                    .iter_mut()
                    .find(|sc| sc.contact.addr == node_addr)
                {
                    match result {
                        Ok(Ok(Ok(response))) => {
                            sc.state = NodeState::Queried;

                            if let Some(resp_args) = response.r {
                                sc.token = resp_args.token.clone();
                                let peers: Vec<SocketAddr> = resp_args
                                    .values
                                    .iter()
                                    .flat_map(|value| {
                                        crate::net::pex::decode_compact_peers(value.as_ref())
                                    })
                                    .collect();
                                if !peers.is_empty() {
                                    info!(
                                        "DHT found {} peers for {:?}",
                                        peers.len(),
                                        self.info_hash
                                    );
                                    let _ = self.swarm_tx.send(SwarmEvent::DhtPeersReceived(peers));
                                }

                                let mut found = decode_compact_nodes(&resp_args.nodes);
                                found.extend(decode_compact_nodes6(&resp_args.nodes6));
                                if !found.is_empty() {
                                    for contact in found.into_iter().filter(|c| {
                                        crate::net::dht::server::is_valid_node_addr(&c.addr)
                                    }) {
                                        let _ = self.manager_tx.send(crate::net::dht::actor::DhtManagerCommand::InsertNode(contact.clone())).await;
                                        if !self
                                            .short_list
                                            .iter()
                                            .any(|existing| existing.contact.id == contact.id)
                                        {
                                            self.short_list.push(SearchContact {
                                                contact,
                                                state: NodeState::Unqueried,
                                                token: Vec::new(),
                                            });
                                        }
                                    }
                                    self.short_list
                                        .sort_by_key(|c| c.contact.id.xor(&self.info_hash));
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

    /// Tells the closest nodes that gave us a token that we are a peer for this torrent.
    async fn announce_to_closest(&self) {
        let Some(port) = self.announce_port else {
            return;
        };

        let mut announced = 0usize;
        for sc in &self.short_list {
            if announced >= 8 {
                break;
            }
            if sc.state != NodeState::Queried || sc.token.is_empty() {
                continue;
            }

            let mut args = QueryArgs::new(self.local_id.0.to_vec());
            args.info_hash = self.info_hash.0.to_vec();
            args.port = Some(port);
            args.token = sc.token.clone();

            // The answer is not needed, so the receiving half is dropped right away.
            let (reply_tx, _reply_rx) = oneshot::channel();
            let sent = self
                .cmd_tx
                .send(DhtCommand::SendQuery {
                    target: sc.contact.addr,
                    msg: KrpcMessage::query(Vec::new(), "announce_peer", args),
                    reply: reply_tx,
                })
                .await;
            if sent.is_ok() {
                announced += 1;
            }
        }
        debug!(
            "DHT announced to {announced} nodes for {:?}",
            self.info_hash
        );
    }
}
