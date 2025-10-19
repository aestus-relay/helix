use helix_types::BidTrace;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeliveredPayload {
    pub bid_trace: BidTrace,
    pub block_number: u64,
    pub epoch: u64,
    pub num_txs: usize,
    pub num_blobs: usize,
    pub blob_gas_used: u64,
    pub excess_blob_gas: u64,
}
