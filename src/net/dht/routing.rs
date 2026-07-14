use std::net::SocketAddr;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub [u8; 20]);

impl NodeId {
    pub fn xor(&self, other: &NodeId) -> NodeId {
        let mut result = [0u8; 20];
        for i in 0..20 {
            result[i] = self.0[i] ^ other.0[i];
        }
        NodeId(result)
    }

    pub fn leading_zeros(&self) -> usize {
        let mut count = 0;
        for &byte in &self.0 {
            if byte == 0 {
                count += 8;
            } else {
                count += byte.leading_zeros() as usize;
                break;
            }
        }
        count
    }

    pub fn bit_at(&self, index: usize) -> u8 {
        if index >= 160 {
            return 0;
        }
        let byte_index = index / 8;
        let bit_in_byte = 7 - (index % 8);
        (self.0[byte_index] >> bit_in_byte) & 1
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: NodeId,
    pub addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct KBucket {
    pub nodes: Vec<Contact>,
    pub prefix_len: usize,
}

impl KBucket {
    pub fn new(prefix_len: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(8),
            prefix_len,
        }
    }

    pub fn is_full(&self) -> bool {
        self.nodes.len() >= 8
    }

    pub fn insert(&mut self, contact: Contact) -> bool {
        if let Some(pos) = self.nodes.iter().position(|c| c.id == contact.id) {
            // Node exists, move to tail (most recently seen)
            let existing = self.nodes.remove(pos);
            self.nodes.push(existing);
            // We can optionally update the addr if it changed
            if let Some(last) = self.nodes.last_mut() {
                last.addr = contact.addr;
            }
            return true;
        }

        if !self.is_full() {
            self.nodes.push(contact);
            return true;
        }

        false
    }
}

pub struct RoutingTable {
    pub local_id: NodeId,
    pub buckets: Vec<KBucket>,
}

impl RoutingTable {
    pub fn new(local_id: NodeId) -> Self {
        Self {
            local_id,
            buckets: vec![KBucket::new(0)],
        }
    }

    pub fn insert(&mut self, contact: Contact) {
        if contact.id == self.local_id {
            return;
        }

        let mut bucket_idx = self.bucket_index(&contact.id);
        

        loop {
            let covers_local = self.bucket_covers_local(bucket_idx);
            let prefix_len = self.buckets[bucket_idx].prefix_len;

            if self.buckets[bucket_idx].insert(contact.clone()) {
                return;
            }

            if covers_local && prefix_len < 160 {
                self.split_bucket(bucket_idx);
                bucket_idx = self.bucket_index(&contact.id);
            } else {
                return;
            }
        }

    }

    fn bucket_index(&self, target: &NodeId) -> usize {
        let distance = self.local_id.xor(target);
        let zeros = distance.leading_zeros();
        
        // Find the bucket whose prefix matches.
        // In our simplified list-of-buckets model, we just iterate through.
        let mut target_idx = 0;
        for (i, bucket) in self.buckets.iter().enumerate() {
            // In a binary tree, leading_zeros exactly maps to the bucket depth we diverge at, 
            // but since buckets cover ranges, we can just find the one that fits.
            // But wait, the standard K-bucket way with a list is:
            if zeros >= bucket.prefix_len {
                target_idx = i;
            }
        }
        
        target_idx
    }

    fn bucket_covers_local(&self, idx: usize) -> bool {
        // A bucket covers local_id if its prefix matches local_id's prefix.
        // Since we split strictly along the local_id's path, the LAST bucket 
        // in our array always covers the local_id.
        idx == self.buckets.len() - 1
    }

    fn split_bucket(&mut self, idx: usize) {
        let bucket = &mut self.buckets[idx];
        let split_bit_idx = bucket.prefix_len;
        
        let mut new_bucket = KBucket::new(split_bit_idx + 1);
        bucket.prefix_len += 1;
        
        // The bucket currently at idx always represents the branch going AWAY from local_id.
        // The 
// ew_bucket represents the branch continuing TOWARDS local_id.
        // Wait, local_id's bit at split_bit_idx determines which one is which.
        let local_bit = self.local_id.bit_at(split_bit_idx);
        
        let mut nodes_to_keep = Vec::with_capacity(8);
        for node in bucket.nodes.drain(..) {
            let node_bit = node.id.bit_at(split_bit_idx);
            if node_bit == local_bit {
                new_bucket.nodes.push(node);
            } else {
                nodes_to_keep.push(node);
            }
        }
        bucket.nodes = nodes_to_keep;
        
        // The new bucket (covering local_id) is always added at the end of the list.
        self.buckets.push(new_bucket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leading_zeros() {
        let mut id1 = NodeId([0; 20]);
        assert_eq!(id1.leading_zeros(), 160);

        id1.0[0] = 0b0100_0000;
        assert_eq!(id1.leading_zeros(), 1);

        let mut id2 = NodeId([0; 20]);
        id2.0[19] = 1;
        assert_eq!(id2.leading_zeros(), 159);
    }

    #[test]
    fn test_bucket_split() {
        let local_id = NodeId([0; 20]);
        let mut table = RoutingTable::new(local_id);

        let addr = "127.0.0.1:6881".parse().unwrap();
        
        // Insert 8 nodes to fill the first bucket
        for i in 1..=8 {
            let mut id = [0u8; 20];
            id[19] = i;
            table.insert(Contact { id: NodeId(id), addr });
        }
        
        assert_eq!(table.buckets.len(), 1);
        assert_eq!(table.buckets[0].nodes.len(), 8);

        // 9th node should trigger a split
        let mut id9 = [0u8; 20];
        id9[0] = 0b1000_0000; // Bit 0 is 1, diverging from local_id (which is all 0)
        table.insert(Contact { id: NodeId(id9), addr });

        assert_eq!(table.buckets.len(), 2);
        
        // The first bucket (prefix_len=1) handles bit 0 == 1 (since local_id has bit 0 == 0)
        assert_eq!(table.buckets[0].nodes.len(), 1); // id9
        
        // The second bucket (prefix_len=1) handles bit 0 == 0
        assert_eq!(table.buckets[1].nodes.len(), 8); // the first 8 nodes
    }
}
