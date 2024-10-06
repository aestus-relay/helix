ALTER TABLE block_submission
ADD COLUMN num_blobs integer DEFAULT 0,
ADD COLUMN blob_gas_used integer DEFAULT 0,
ADD COLUMN excess_blob_gas integer DEFAULT 0;