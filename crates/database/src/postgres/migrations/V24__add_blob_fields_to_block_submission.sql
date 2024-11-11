ALTER TABLE block_submission
ADD COLUMN "num_blobs" int DEFAULT 0,
ADD COLUMN "blob_gas_used" int DEFAULT 0,
ADD COLUMN "excess_blob_gas" int DEFAULT 0;