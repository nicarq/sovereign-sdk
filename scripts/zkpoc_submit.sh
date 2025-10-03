#!/usr/bin/env bash
set -euo pipefail

# This script:
#  1) Generates a zk-poc proof for a value using the RISC Zero host
#  2) Creates a JSON transaction importing that proof
#  3) Submits the batch to a running demo rollup via sov-cli
#
# Prerequisites:
#  - The demo rollup is running (mock or celestia)
#  - The sov-cli binary is built (cargo build -p sov-demo-rollup)
#  - zk-poc method_id in the rollup genesis matches the generated ELF's code commitment
#  - The wallet has a key with funds (see demo README)
#
# Usage:
#   scripts/zkpoc_submit.sh [--value 100] [--genesis-dir <path>] [--chain-id 4321] [--sequencer <addr>] \
#       [--proof-bin <path>] [--proof-hex <path>] [--tx-json <path>] [--sov-cli <path>]

VALUE=100
GENESIS_DIR="examples/test-data/genesis/demo/mock"
CHAIN_ID=4321
SEQUENCER=""
API_URL=${API_URL:-http://127.0.0.1:12346}
KEY_NICKNAME=${KEY_NICKNAME:-zkpoc-demo}
KEY_FILE_DEFAULT="examples/test-data/keys/token_deployer_private_key.json"
PROOF_BIN="zkpoc_proof.bin"
PROOF_HEX="zkpoc_proof.hex"
TX_JSON="zkpoc_tx.json"
SOV_CLI="target/debug/sov-cli"

while [[ $# -gt 0 ]]; do
  case $1 in
    --value) VALUE=$2; shift 2;;
    --genesis-dir) GENESIS_DIR=$2; shift 2;;
    --chain-id) CHAIN_ID=$2; shift 2;;
    --sequencer) SEQUENCER=$2; shift 2;;
    --proof-bin) PROOF_BIN=$2; shift 2;;
    --proof-hex) PROOF_HEX=$2; shift 2;;
    --tx-json) TX_JSON=$2; shift 2;;
    --sov-cli) SOV_CLI=$2; shift 2;;
    *) echo "Unknown arg: $1"; exit 1;;
  esac
done

# Derive project root (this script is run from repo root ideally)
ROOT_DIR=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
cd "$ROOT_DIR"

if [[ -z "$SEQUENCER" ]]; then
  if command -v jq >/dev/null 2>&1 && [[ -f "$GENESIS_DIR/sequencer_registry.json" ]]; then
    SEQUENCER=$(jq -r '.sequencer_config.seq_rollup_address' "$GENESIS_DIR/sequencer_registry.json")
  else
    # Fallback to the default demo address
    SEQUENCER="sov1lzkjgdaz08su3yevqu6ceywufl35se9f33kztu5cu2spja5hyyf"
  fi
fi

echo "Generating proof for value=$VALUE ..."
RISC0_PROVER=${RISC0_PROVER:-ipc} RISC0_DEV_MODE=${RISC0_DEV_MODE:-0} \
  cargo run -q -p zk-poc --features native --bin gen_proof -- \
    --value "$VALUE" --out "$PROOF_BIN" --hex-out "$PROOF_HEX" 1>&2

if [[ ! -f "$PROOF_HEX" ]]; then
  echo "Failed to generate proof hex at $PROOF_HEX" >&2
  exit 1
fi

# Convert proof bytes to JSON array (Vec<u8>) since CLI expects a sequence for bytes
if command -v hexdump >/dev/null 2>&1; then
  BYTES_CSV=$(hexdump -v -e '1/1 "%u,"' "$PROOF_BIN")
  BYTES_CSV=${BYTES_CSV%,}
else
  # Fallback using od/awk
  BYTES_CSV=$(od -An -t u1 -v "$PROOF_BIN" | tr -s ' ' '\n' | sed '/^$/d' | paste -sd, -)
fi

echo "Creating transaction JSON at $TX_JSON ..."
cat > "$TX_JSON" <<JSON
{
  "set_value": {
    "value": $VALUE,
    "proof": [${BYTES_CSV}]
  }
}
JSON

if [[ ! -x "$SOV_CLI" ]]; then
  echo "Building sov-cli ..."
  cargo build -q -p sov-demo-rollup
fi

echo "Setting REST API URL to $API_URL ..."
"$SOV_CLI" node set-url "$API_URL"

echo "Importing transaction into wallet ..."
"$SOV_CLI" transactions import from-file zk-poc \
  --chain-id "$CHAIN_ID" --max-fee 100000000 \
  --path "$TX_JSON"

echo "Importing signing key (nickname: $KEY_NICKNAME) ..."
# Use absolute path for key file to avoid stale relative paths in wallet state
KEY_PATH_ABS=$(cd "$ROOT_DIR" && python3 - << PY
import os
path=os.path.abspath("${KEY_FILE_DEFAULT}")
print(path)
PY
)
"$SOV_CLI" keys import --nickname "$KEY_NICKNAME" --path "$KEY_PATH_ABS" --skip-if-present || true

echo "Waiting for node to be synced with DA before submitting..."
ATTEMPTS=0
MAX_ATTEMPTS=${MAX_SYNC_WAIT_ATTEMPTS:-60}
SLEEP_SECS=${SYNC_WAIT_SLEEP_SECS:-5}
while true; do
  STATUS_JSON=$(curl -sS "$API_URL/rollup/sync-status" || echo "")
  if [[ -n "$STATUS_JSON" ]]; then
    if echo "$STATUS_JSON" | grep -q '"Synced"'; then
      echo "Node is synced: $STATUS_JSON"
      break
    fi
    if echo "$STATUS_JSON" | grep -q '"Syncing"'; then
      if command -v jq >/dev/null 2>&1; then
        SDH=$(echo "$STATUS_JSON" | jq -r '.Syncing.synced_da_height // .synced_da_height // 0')
        TDH=$(echo "$STATUS_JSON" | jq -r '.Syncing.target_da_height // .target_da_height // 0')
        echo "Syncing... synced_da_height=$SDH target_da_height=$TDH"
      else
        echo "Syncing... $STATUS_JSON"
      fi
    else
      echo "Unknown sync status: $STATUS_JSON"
    fi
  else
    echo "Could not fetch sync status from $API_URL/rollup/sync-status"
  fi
  ATTEMPTS=$((ATTEMPTS+1))
  if [[ $ATTEMPTS -ge $MAX_ATTEMPTS ]]; then
    echo "Timed out waiting for node to sync; proceeding to submit anyway..."
    break
  fi
  sleep "$SLEEP_SECS"
done

echo "Submitting batch using account by-nickname $KEY_NICKNAME ..."
"$SOV_CLI" node submit-batch --wait-for-processing by-nickname "$KEY_NICKNAME"

echo "Done."
