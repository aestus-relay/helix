CREATE TABLE "delivered_constraints" (
    "block_hash" bytea PRIMARY KEY,
    "slot_number" INTEGER NOT NULL,
    "num_constraints" INTEGER NOT NULL,
    "created_at" TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);