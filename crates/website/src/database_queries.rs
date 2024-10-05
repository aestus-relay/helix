use deadpool_postgres::tokio_postgres;
use async_trait::async_trait;
use helix_database::error::DatabaseError;
use crate::models::DeliveredPayload;
use tracing::{debug, error};
use helix_database::postgres::postgres_db_service::PostgresDatabaseService;
use helix_database::postgres::postgres_db_u256_parsing::{PostgresNumeric};
use helix_database::postgres::postgres_db_row_parsing::{
    parse_bytes_to_hash, parse_bytes_to_pubkey, parse_numeric_to_u256, FromRow, parse_rows
};

#[async_trait]
pub trait WebsiteDatabaseService: Send + Sync {
    async fn get_recent_delivered_payloads(&self, limit: i64) -> Result<Vec<DeliveredPayload>, DatabaseError>;
    async fn get_num_network_validators(&self) -> Result<i64, DatabaseError>;
    async fn get_num_registered_validators(&self) -> Result<i64, DatabaseError>;
    async fn get_num_delivered_payloads(&self) -> Result<i64, DatabaseError>;
}


impl FromRow for DeliveredPayload {
    fn from_row(row: &tokio_postgres::Row) -> Result<Self, DatabaseError> {
        debug!("Parsing DeliveredPayload from database row");
        Ok(DeliveredPayload {
            slot: row.get::<_, i32>("slot_number"),
            epoch: (row.get::<_, i32>("slot_number") / 32) as i32, // Calculate epoch directly
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
    async fn get_recent_delivered_payloads(&self, limit: i64) -> Result<Vec<DeliveredPayload>, DatabaseError> {
        debug!("Fetching recent delivered payloads with limit: {}", limit);
        let query = "
        SELECT
            block_submission.slot_number,
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
        ORDER BY block_submission.slot_number DESC
        LIMIT $1
        ";

        let rows = match self.pool.get().await?.query(query, &[&limit]).await {
            Ok(rows) => rows,
            Err(e) => {
                error!("Failed to fetch recent delivered payloads: {}", e);
                return Err(DatabaseError::Postgres(e));
            }
        };

        let payloads = parse_rows(rows)?;
        debug!("Fetched {} recent delivered payloads", payloads.len());
        Ok(payloads)
    }

    async fn get_num_network_validators(&self) -> Result<i64, DatabaseError> {
        debug!("Fetching number of network validators");
        let client = self.pool.get().await?;
        let row = match client.query_one("SELECT COUNT(*) FROM known_validators", &[]).await {
            Ok(row) => row,
            Err(e) => {
                error!("Failed to fetch number of network validators: {}", e);
                return Err(DatabaseError::Postgres(e));
            }
        };
        Ok(row.get::<usize, i64>(0))
    }

    async fn get_num_registered_validators(&self) -> Result<i64, DatabaseError> {
        debug!("Fetching number of registered validators");
        let client = self.pool.get().await?;
        let row = match client.query_one("SELECT COUNT(*) FROM validator_registrations", &[]).await {
            Ok(row) => row,
            Err(e) => {
                error!("Failed to fetch number of registered validators: {}", e);
                return Err(DatabaseError::Postgres(e));
            }
        };
        Ok(row.get::<usize, i64>(0))
    }

    async fn get_num_delivered_payloads(&self) -> Result<i64, DatabaseError> {
        debug!("Fetching number of delivered payloads");
        let client = self.pool.get().await?;
        let row = match client.query_one("SELECT COUNT(*) FROM delivered_payload", &[]).await {
            Ok(row) => row,
            Err(e) => {
                error!("Failed to fetch number of delivered payloads: {}", e);
                return Err(DatabaseError::Postgres(e));
            }
        };
        Ok(row.get::<usize, i64>(0))
    }
}