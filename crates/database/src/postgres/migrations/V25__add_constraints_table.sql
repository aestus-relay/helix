CREATE TABLE "delivered_constraints" (
    "slot" INTEGER PRIMARY KEY,
    "num_constraints" INTEGER NOT NULL,
    "created_at" TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);