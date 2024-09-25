use deadpool_postgres::tokio_postgres;
use std::sync::Arc;
use async_trait::async_trait;
use helix_database::error::DatabaseError;
use crate::models::DeliveredPayload;
use helix_database::postgres::postgres_db_service::PostgresDatabaseService;
use helix_database::postgres::postgres_db_u256_parsing::{PostgresNumeric};
use helix_database::postgres::postgres_db_row_parsing::{
    parse_i32_to_u64, parse_bytes_to_hash, parse_bytes_to_pubkey, parse_numeric_to_u256, FromRow, parse_rows
};
use helix_common::bellatrix::ByteVector;
use ethereum_consensus::primitives::U256;


#[async_trait]
pub trait WebsiteDatabaseService: Send + Sync {
    async fn get_recent_delivered_payloads(&self, limit: i32) -> Result<Vec<DeliveredPayload>, DatabaseError>;
    async fn get_num_network_validators(&self) -> Result<i64, DatabaseError>;
    async fn get_num_registered_validators(&self) -> Result<i64, DatabaseError>;
    async fn get_latest_slot(&self) -> Result<i32, DatabaseError>;
    async fn get_num_delivered_payloads(&self) -> Result<i64, DatabaseError>;
}
impl FromRow for DeliveredPayload {
    fn from_row(row: &tokio_postgres::Row) -> Result<Self, DatabaseError> {
        Ok(DeliveredPayload {
            slot: row.get::<_, i32>("slot_number"),
            epoch: row.get::<_,i32>("epoch"),
            parent_hash: parse_bytes_to_hash::<32>(row.get::<_, &[u8]>("parent_hash"))?,
            block_hash: parse_bytes_to_hash::<32>(row.get::<_, &[u8]>("block_hash"))?,
            builder_pubkey: parse_bytes_to_pubkey(row.get::<_, &[u8]>("builder_pubkey"))?,
            proposer_pubkey: parse_bytes_to_pubkey(row.get::<_, &[u8]>("proposer_pubkey"))?,
            proposer_fee_recipient: parse_bytes_to_hash::<20>(row.get::<_, &[u8]>("proposer_fee_recipient"))?,
            gas_limit: row.get::<_, i32>("gas_limit"),
            gas_used: row.get::<_, i32>("gas_used"),
            value: parse_numeric_to_u256(row.get::<_, PostgresNumeric>("value")),
            num_txs: row.get::<_, i32>("num_txs"),
            block: row.get::<_, i32>("block_number"),
        })
    }
}


#[async_trait]
impl WebsiteDatabaseService for PostgresDatabaseService {
    async fn get_recent_delivered_payloads(&self, limit: i32) -> Result<Vec<DeliveredPayload>, DatabaseError> {
        let query = "
        SELECT
            block_submission.slot_number,
            slot.epoch,
            block_submission.parent_hash,
            block_submission.block_hash,
            block_submission.builder_pubkey,
            block_submission.proposer_pubkey,
            block_submission.proposer_fee_recipient,
            block_submission.gas_limit,
            block_submission.gas_used,
            block_submission.value,
            block_submission.num_txs,
            block_submission.block_number
        FROM
            delivered_payload
        INNER JOIN
            block_submission ON block_submission.block_hash = delivered_payload.block_hash
        LEFT JOIN
            slot ON slot.number = block_submission.slot_number
        ORDER BY block_submission.slot_number DESC
        LIMIT $1
        ";

        let rows = self.pool.get().await?
            .query(query, &[&limit])
            .await?;

        parse_rows(rows)
    }

    async fn get_num_network_validators(&self) -> Result<i64, DatabaseError> {
        let client = self.pool.get().await?;
        let row = client
            .query_one("SELECT COUNT(*) FROM known_validators", &[])
            .await?;
        Ok(row.get::<usize, i64>(0))
    }

    async fn get_num_registered_validators(&self) -> Result<i64, DatabaseError> {
        let client = self.pool.get().await?;
        let row = client
            .query_one("SELECT COUNT(*) FROM validator_registrations", &[])
            .await?;
        Ok(row.get::<usize, i64>(0))
    }

    async fn get_latest_slot(&self) -> Result<i32, DatabaseError> {
        let client = self.pool.get().await?;
        let row = client
            .query_one("SELECT MAX(number) FROM slot", &[])
            .await?;
        Ok(row.get::<usize, i32>(0))
    }

    async fn get_num_delivered_payloads(&self) -> Result<i64, DatabaseError> {
        let client = self.pool.get().await?;
        let row = client
            .query_one("SELECT COUNT(*) FROM delivered_payload", &[])
            .await?;
        Ok(row.get::<usize, i64>(0))
    }
}