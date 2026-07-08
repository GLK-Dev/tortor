use std::time::{SystemTime, UNIX_EPOCH};

pub fn generate_peer_id() -> [u8; 20] {
    let mut peer_id = [b'0'; 20];
    let prefix = b"-TT0001-";
    peer_id[..prefix.len()].copy_from_slice(prefix);

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ u64::from(std::process::id());

    let tail = format!("{seed:012x}");
    peer_id[8..20].copy_from_slice(&tail.as_bytes()[..12]);
    peer_id
}

#[cfg(test)]
mod tests {
    use super::generate_peer_id;

    #[test]
    fn peer_id_has_expected_shape() {
        let id = generate_peer_id();
        assert_eq!(id.len(), 20);
        assert_eq!(&id[..8], b"-TT0001-");
    }
}
