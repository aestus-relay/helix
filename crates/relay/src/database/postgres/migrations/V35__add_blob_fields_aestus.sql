ALTER TABLE block_submission
ADD COLUMN num_blobs INTEGER,
ADD COLUMN blob_gas_used INTEGER,
ADD COLUMN excess_blob_gas INTEGER;
