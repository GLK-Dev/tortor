use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrpcMessage {
    #[serde(with = "serde_bytes")]
    pub t: Vec<u8>,
    pub y: String,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<QueryArgs>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<ResponseArgs>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<Vec<serde_bencode::value::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryArgs {
    #[serde(with = "serde_bytes")]
    pub id: Vec<u8>,
    
    #[serde(with = "serde_bytes", default)]
    pub target: Vec<u8>,
    
    #[serde(with = "serde_bytes", default)]
    pub info_hash: Vec<u8>,
    
    #[serde(default)]
    pub port: Option<u16>,
    
    #[serde(with = "serde_bytes", default)]
    pub token: Vec<u8>,
    
    #[serde(default)]
    pub implied_port: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseArgs {
    #[serde(with = "serde_bytes")]
    pub id: Vec<u8>,
    
    #[serde(with = "serde_bytes", default)]
    pub nodes: Vec<u8>,
    
    #[serde(default)]
    pub values: Vec<serde_bytes::ByteBuf>,
    
    #[serde(with = "serde_bytes", default)]
    pub token: Vec<u8>,
}

impl KrpcMessage {
    pub fn new_ping_query(tid: Vec<u8>, node_id: Vec<u8>) -> Self {
        Self {
            t: tid,
            y: "q".to_string(),
            q: Some("ping".to_string()),
            a: Some(QueryArgs {
                id: node_id,
                target: vec![],
                info_hash: vec![],
                port: None,
                token: vec![],
                implied_port: None,
            }),
            r: None,
            e: None,
        }
    }
}
