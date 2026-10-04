use serde::{Deserialize, Serialize};
use serde_bencode::value::Value;

// Struct fields are declared in alphabetical order because bencode dictionaries
// must have sorted keys and serde_bencode writes fields in declaration order.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrpcMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<QueryArgs>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<Vec<Value>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<ResponseArgs>,

    #[serde(with = "serde_bytes")]
    pub t: Vec<u8>,

    pub y: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryArgs {
    #[serde(with = "serde_bytes")]
    pub id: Vec<u8>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implied_port: Option<u8>,

    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub info_hash: Vec<u8>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub target: Vec<u8>,

    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub token: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseArgs {
    #[serde(with = "serde_bytes")]
    pub id: Vec<u8>,

    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<u8>,

    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub token: Vec<u8>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<serde_bytes::ByteBuf>,
}

impl QueryArgs {
    pub fn new(id: Vec<u8>) -> Self {
        Self {
            id,
            implied_port: None,
            info_hash: vec![],
            port: None,
            target: vec![],
            token: vec![],
        }
    }
}

impl KrpcMessage {
    pub fn new_ping_query(tid: Vec<u8>, node_id: Vec<u8>) -> Self {
        Self::query(tid, "ping", QueryArgs::new(node_id))
    }

    pub fn query(tid: Vec<u8>, name: &str, args: QueryArgs) -> Self {
        Self {
            a: Some(args),
            e: None,
            q: Some(name.to_string()),
            r: None,
            t: tid,
            y: "q".to_string(),
        }
    }

    pub fn response(tid: Vec<u8>, args: ResponseArgs) -> Self {
        Self {
            a: None,
            e: None,
            q: None,
            r: Some(args),
            t: tid,
            y: "r".to_string(),
        }
    }

    pub fn error(tid: Vec<u8>, code: i64, message: &str) -> Self {
        Self {
            a: None,
            e: Some(vec![
                Value::Int(code),
                Value::Bytes(message.as_bytes().to_vec()),
            ]),
            q: None,
            r: None,
            t: tid,
            y: "e".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_keys_are_sorted_on_the_wire() {
        let mut args = QueryArgs::new(vec![1; 20]);
        args.info_hash = vec![2; 20];
        let msg = KrpcMessage::query(b"aa".to_vec(), "get_peers", args);
        let bytes = serde_bencode::to_bytes(&msg).unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let positions: Vec<usize> = ["1:a", "1:q", "1:t", "1:y"]
            .iter()
            .map(|key| text.find(key).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(text.find("2:id").unwrap() < text.find("9:info_hash").unwrap());
    }

    #[test]
    fn response_roundtrip_skips_empty_fields() {
        let msg = KrpcMessage::response(
            b"xy".to_vec(),
            ResponseArgs {
                id: vec![3; 20],
                nodes: vec![],
                token: b"tok".to_vec(),
                values: vec![serde_bytes::ByteBuf::from(vec![1, 2, 3, 4, 0x1A, 0xE1])],
            },
        );
        let bytes = serde_bencode::to_bytes(&msg).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("5:nodes"));
        let parsed: KrpcMessage = serde_bencode::from_bytes(&bytes).unwrap();
        let r = parsed.r.unwrap();
        assert_eq!(r.token, b"tok");
        assert_eq!(r.values.len(), 1);
        assert_eq!(parsed.y, "r");
    }

    #[test]
    fn error_message_has_code_and_text() {
        let bytes =
            serde_bencode::to_bytes(&KrpcMessage::error(b"e1".to_vec(), 204, "Method Unknown"))
                .unwrap();
        let parsed: KrpcMessage = serde_bencode::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.y, "e");
        assert_eq!(parsed.e.unwrap().len(), 2);
    }
}
