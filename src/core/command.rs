use std::net::SocketAddr;

#[derive(Debug, Clone, Copy)]
pub enum CoreCommand {
    ProbePeer(SocketAddr),
}
