-- Create table for sequencer leader election
-- This table maintains exactly one row containing the current leader node ID and heartbeat timestamp
CREATE TABLE IF NOT EXISTS sequencer_leader (
    -- Singleton constraint - only one row allowed in this table
    singleton INTEGER GENERATED ALWAYS AS (1) STORED UNIQUE,
    -- Node ID of the current leader (UUID v7)
    node_id UUID NOT NULL,
    -- Timestamp when the leader last sent a heartbeat
    last_updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Primary key on the singleton to enforce single row
    PRIMARY KEY (singleton)
);

-- Create NOTIFY trigger for leader election table changes
CREATE OR REPLACE FUNCTION notify_leader_changes()
RETURNS TRIGGER AS $$
BEGIN
    -- Send notification with node_id and timestamp for INSERT and UPDATE operations
    PERFORM pg_notify('leader_changes',
        NEW.node_id::text || ',' ||
        EXTRACT(EPOCH FROM NEW.last_updated)::text
    );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER leader_changes_trigger
    AFTER INSERT OR UPDATE ON sequencer_leader
    FOR EACH ROW
    EXECUTE FUNCTION notify_leader_changes();