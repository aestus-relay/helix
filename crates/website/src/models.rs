use serde::{Serialize, Deserialize};
use deadpool_postgres::tokio_postgres::Row;
use deadpool_postgres::tokio_postgres::Error;
use helix_common::bellatrix::ByteVector;
use ethereum_consensus::primitives::{U256, BlsPublicKey};

#[derive(Debug, Serialize, Deserialize)]
pub struct NumRegisteredValidators {
    pub num_validators: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeliveredPayload {
    pub slot: i32,
    pub epoch: i32,
    pub parent_hash: ByteVector<32>,
    pub block_hash: ByteVector<32>,
    pub builder_pubkey: BlsPublicKey,
    pub proposer_pubkey: BlsPublicKey,
    pub proposer_fee_recipient: ByteVector<20>,
    pub gas_limit: i32,
    pub gas_used: i32,
    pub value: U256,
    pub num_txs: i32,
    pub block: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NumPayloads {
    pub num_payloads: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LatestSlot {
    pub slot: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkValidator {
    pub public_key: Vec<u8>,
    pub index: i64,
}

impl TryFrom<Row> for NetworkValidator {
    type Error = Error;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        Ok(Self {
            public_key: row.get("public_key"),
            index: row.get("index"),
        })
    }
}
