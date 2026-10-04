//! UPnP port mapping so peers behind a NAT router can reach our listeners.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

const LEASE_SECS: u32 = 3600;
const RENEW_EVERY: Duration = Duration::from_secs(1800);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(600);
const DESCRIPTION: &str = "TorTor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortMapStatus {
    Disabled,
    Searching,
    /// Both ports are forwarded; `external_ip` is the router's WAN address when it told us.
    Mapped {
        external_ip: Option<IpAddr>,
    },
    Unavailable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug)]
pub enum MapError {
    /// The router only accepts permanent mappings (lease 0).
    LeaseUnsupported,
    Other(String),
}

/// What the maintenance loop needs from a router.
#[async_trait]
pub trait PortMapper: Send + Sync {
    async fn external_ip(&self) -> Result<IpAddr, String>;
    async fn add(&self, protocol: Protocol, port: u16, lease_secs: u32) -> Result<(), MapError>;
    async fn remove(&self, protocol: Protocol, port: u16) -> Result<(), String>;
}

/// Maps `port` for TCP and UDP, renews the mappings and removes them when
/// `shutdown` fires. Returns an error if the mappings could not be created.
pub async fn maintain(
    mapper: &dyn PortMapper,
    port: u16,
    status: &watch::Sender<PortMapStatus>,
    shutdown: &mut broadcast::Receiver<()>,
    renew_every: Duration,
) -> Result<(), String> {
    let mut lease = LEASE_SECS;
    loop {
        for protocol in [Protocol::Tcp, Protocol::Udp] {
            match mapper.add(protocol, port, lease).await {
                Ok(()) => {}
                Err(MapError::LeaseUnsupported) if lease != 0 => {
                    // Retry the same mapping as a permanent one.
                    lease = 0;
                    mapper
                        .add(protocol, port, 0)
                        .await
                        .map_err(|e| format!("{e:?}"))?;
                }
                Err(err) => return Err(format!("{protocol:?} {port}: {err:?}")),
            }
        }

        let external_ip = mapper.external_ip().await.ok();
        info!("UPnP: port {port} mapped (external address: {external_ip:?})");
        let _ = status.send(PortMapStatus::Mapped { external_ip });

        tokio::select! {
            _ = shutdown.recv() => {
                for protocol in [Protocol::Tcp, Protocol::Udp] {
                    if let Err(err) = mapper.remove(protocol, port).await {
                        debug!("UPnP: could not remove {protocol:?} mapping: {err}");
                    }
                }
                return Ok(());
            }
            _ = tokio::time::sleep(renew_every) => {}
        }
    }
}

struct IgdMapper {
    gateway: igd_next::aio::Gateway<igd_next::aio::tokio::Tokio>,
    local_ip: IpAddr,
}

fn igd_protocol(protocol: Protocol) -> igd_next::PortMappingProtocol {
    match protocol {
        Protocol::Tcp => igd_next::PortMappingProtocol::TCP,
        Protocol::Udp => igd_next::PortMappingProtocol::UDP,
    }
}

#[async_trait]
impl PortMapper for IgdMapper {
    async fn external_ip(&self) -> Result<IpAddr, String> {
        self.gateway
            .get_external_ip()
            .await
            .map_err(|e| e.to_string())
    }

    async fn add(&self, protocol: Protocol, port: u16, lease_secs: u32) -> Result<(), MapError> {
        self.gateway
            .add_port(
                igd_protocol(protocol),
                port,
                SocketAddr::new(self.local_ip, port),
                lease_secs,
                DESCRIPTION,
            )
            .await
            .map_err(|err| match err {
                igd_next::AddPortError::OnlyPermanentLeasesSupported => MapError::LeaseUnsupported,
                other => MapError::Other(other.to_string()),
            })
    }

    async fn remove(&self, protocol: Protocol, port: u16) -> Result<(), String> {
        self.gateway
            .remove_port(igd_protocol(protocol), port)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Finds the router, maps `port` and keeps the mapping alive until shutdown.
/// Without a UPnP router the search is repeated every ten minutes.
pub async fn run_upnp(
    port: u16,
    status: Arc<watch::Sender<PortMapStatus>>,
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        let _ = status.send(PortMapStatus::Searching);

        let outcome = discover_and_maintain(port, &status, &mut shutdown).await;
        match outcome {
            Ok(()) => return,
            Err(reason) => {
                warn!("UPnP unavailable: {reason}");
                let _ = status.send(PortMapStatus::Unavailable(reason));
            }
        }

        tokio::select! {
            _ = shutdown.recv() => return,
            _ = tokio::time::sleep(RETRY_AFTER_FAILURE) => {}
        }
    }
}

async fn discover_and_maintain(
    port: u16,
    status: &watch::Sender<PortMapStatus>,
    shutdown: &mut broadcast::Receiver<()>,
) -> Result<(), String> {
    let options = igd_next::SearchOptions {
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let gateway = igd_next::aio::tokio::search_gateway(options)
        .await
        .map_err(|e| format!("no UPnP gateway found ({e})"))?;

    // The local address that routes towards the gateway is the one to forward to.
    let probe = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    probe
        .connect(gateway.addr)
        .await
        .map_err(|e| e.to_string())?;
    let local_ip = probe.local_addr().map_err(|e| e.to_string())?.ip();

    let mapper = IgdMapper { gateway, local_ip };
    maintain(&mapper, port, status, shutdown, RENEW_EVERY).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockMapper {
        calls: Mutex<Vec<String>>,
        permanent_only: bool,
    }

    #[async_trait]
    impl PortMapper for MockMapper {
        async fn external_ip(&self) -> Result<IpAddr, String> {
            Ok("203.0.113.7".parse().unwrap())
        }

        async fn add(&self, protocol: Protocol, port: u16, lease: u32) -> Result<(), MapError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("add {protocol:?} {port} lease={lease}"));
            if self.permanent_only && lease != 0 {
                return Err(MapError::LeaseUnsupported);
            }
            Ok(())
        }

        async fn remove(&self, protocol: Protocol, port: u16) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("remove {protocol:?} {port}"));
            Ok(())
        }
    }

    async fn run(mapper: &MockMapper, renew: Duration, run_for: Duration) -> Vec<String> {
        let (status_tx, mut status_rx) = watch::channel(PortMapStatus::Searching);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);

        let stopper = tokio::spawn(async move {
            tokio::time::sleep(run_for).await;
            let _ = shutdown_tx.send(());
        });
        maintain(mapper, 6881, &status_tx, &mut shutdown_rx, renew)
            .await
            .unwrap();
        stopper.await.unwrap();

        assert_eq!(
            *status_rx.borrow_and_update(),
            PortMapStatus::Mapped {
                external_ip: Some("203.0.113.7".parse().unwrap())
            }
        );
        mapper.calls.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn maps_both_protocols_and_removes_them_on_shutdown() {
        let mapper = MockMapper::default();
        let calls = run(&mapper, Duration::from_secs(60), Duration::from_millis(50)).await;
        assert_eq!(
            calls,
            vec![
                "add Tcp 6881 lease=3600",
                "add Udp 6881 lease=3600",
                "remove Tcp 6881",
                "remove Udp 6881"
            ]
        );
    }

    #[tokio::test]
    async fn falls_back_to_permanent_leases_and_renews() {
        let mapper = MockMapper {
            permanent_only: true,
            ..Default::default()
        };
        let calls = run(
            &mapper,
            Duration::from_millis(30),
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(calls[0], "add Tcp 6881 lease=3600");
        assert_eq!(calls[1], "add Tcp 6881 lease=0");
        assert!(calls.contains(&"add Udp 6881 lease=0".to_string()));
        // At least one renewal happened before shutdown.
        assert!(calls.iter().filter(|c| c.starts_with("add Tcp")).count() >= 3);
        assert_eq!(calls.last().unwrap(), "remove Udp 6881");
    }

    #[tokio::test]
    async fn real_errors_are_reported() {
        struct Failing;
        #[async_trait]
        impl PortMapper for Failing {
            async fn external_ip(&self) -> Result<IpAddr, String> {
                Err("no".into())
            }
            async fn add(&self, _: Protocol, _: u16, _: u32) -> Result<(), MapError> {
                Err(MapError::Other("conflict".into()))
            }
            async fn remove(&self, _: Protocol, _: u16) -> Result<(), String> {
                Ok(())
            }
        }
        let (status_tx, _rx) = watch::channel(PortMapStatus::Searching);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
        let result = maintain(
            &Failing,
            6881,
            &status_tx,
            &mut shutdown_rx,
            Duration::from_secs(1),
        )
        .await;
        assert!(result.unwrap_err().contains("conflict"));
    }
}
