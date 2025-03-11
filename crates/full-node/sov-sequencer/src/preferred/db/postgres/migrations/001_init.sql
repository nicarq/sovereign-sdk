CREATE TABLE IF NOT EXISTS txs (
	sequence_number BIGINT NOT NULL,
	batch_index BIGINT NOT NULL,
	hash BYTEA NOT NULL,
	data BYTEA NOT NULL,
	PRIMARY KEY (sequence_number, batch_index)
);

CREATE TABLE IF NOT EXISTS blobs (
	sequence_number BIGINT PRIMARY KEY,
	borsh_value BYTEA NOT NULL
);

CREATE TABLE IF NOT EXISTS in_progress_batch (
	-- See <https://stackoverflow.com/a/72358001>.
	singleton INTEGER GENERATED ALWAYS AS (1) STORED UNIQUE,
	sequence_number BIGINT NOT NULL,
	borsh_value BYTEA NOT NULL
);
