use anyhow::{bail, Context, Result};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: [u8; 20],
    pub name: Option<String>,
    pub trackers: Vec<String>,
}

/// Parses a `magnet:?xt=urn:btih:<hash>&dn=<name>&tr=<tracker>...` link. The
/// hash may be 40 hex characters or 32 base32 characters.
pub fn parse(uri: &str) -> Result<Magnet> {
    let url = Url::parse(uri.trim()).context("not a valid magnet URI")?;
    if url.scheme() != "magnet" {
        bail!("not a magnet link");
    }

    let mut info_hash = None;
    let mut name = None;
    let mut trackers = Vec::new();

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "xt" => {
                if let Some(hash) = value.strip_prefix("urn:btih:") {
                    info_hash = Some(decode_info_hash(hash)?);
                }
            }
            "dn" => name = Some(value.into_owned()),
            "tr" => trackers.push(value.into_owned()),
            _ => {}
        }
    }

    Ok(Magnet {
        info_hash: info_hash.context("magnet link has no urn:btih info hash")?,
        name,
        trackers,
    })
}

fn decode_info_hash(hash: &str) -> Result<[u8; 20]> {
    let bytes = match hash.len() {
        40 => hex::decode(hash).context("info hash is not valid hex")?,
        32 => decode_base32(hash).context("info hash is not valid base32")?,
        other => bail!("unsupported info hash length {other}"),
    };
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("info hash must be 20 bytes"))
}

fn decode_base32(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for c in input.chars() {
        let value = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "08ada5a7a6183aae1e09d831df6748d566095a10";

    #[test]
    fn parses_hex_magnet_with_name_and_trackers() {
        let uri = format!(
            "magnet:?xt=urn:btih:{HEX}&dn=Sintel%20Movie&tr=udp%3A%2F%2Ftracker.example%3A1337&tr=http%3A%2F%2Ft.example%2Fannounce"
        );
        let magnet = parse(&uri).unwrap();
        assert_eq!(hex::encode(magnet.info_hash), HEX);
        assert_eq!(magnet.name.as_deref(), Some("Sintel Movie"));
        assert_eq!(
            magnet.trackers,
            vec!["udp://tracker.example:1337", "http://t.example/announce"]
        );
    }

    #[test]
    fn base32_and_hex_hashes_agree() {
        // base32 of the 20 bytes behind HEX
        let bytes = hex::decode(HEX).unwrap();
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let mut encoded = String::new();
        let (mut buffer, mut bits) = (0u32, 0u32);
        for byte in bytes {
            buffer = (buffer << 8) | byte as u32;
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                encoded.push(alphabet[((buffer >> bits) & 31) as usize] as char);
            }
            buffer &= (1 << bits) - 1;
        }
        assert_eq!(encoded.len(), 32);

        let magnet = parse(&format!("magnet:?xt=urn:btih:{}", encoded.to_lowercase())).unwrap();
        assert_eq!(hex::encode(magnet.info_hash), HEX);
    }

    #[test]
    fn rejects_bad_links() {
        assert!(parse("http://example.com").is_err());
        assert!(parse("magnet:?dn=nothing").is_err());
        assert!(parse("magnet:?xt=urn:btih:1234").is_err());
        assert!(parse(&format!("magnet:?xt=urn:btih:{}", "zz".repeat(20))).is_err());
        assert!(parse("not a uri").is_err());
    }
}
