#!/usr/bin/env bash
set -euo pipefail

# Simple helper to run the demo-rollup with the Ligetron zkVM.
#
# Usage examples:
#   bash run_ligetron.sh                            # mock DA, jmt, execute (default)
#   bash run_ligetron.sh --da celestia              # celestia DA
#   bash run_ligetron.sh --storage nomt             # NOMT storage
#   bash run_ligetron.sh --mode prove               # generate proof
#   LIGETRON_PROVER=/path/prover bash run_ligetron.sh  # override binaries via env

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

DA_LAYER="mock"      # mock | celestia
STORAGE="jmt"        # jmt | nomt
MODE="execute"       # skip | execute | prove

while [[ $# -gt 0 ]]; do
  case "$1" in
    --da)
      DA_LAYER="$2"; shift 2 ;;
    --storage)
      STORAGE="$2"; shift 2 ;;
    --mode)
      MODE="$2"; shift 2 ;;
    -h|--help)
      cat <<EOF
Run demo-rollup with Ligetron zkVM

Options:
  --da <mock|celestia>       Data availability layer (default: mock)
  --storage <jmt|nomt>       Storage type (default: jmt)
  --mode <skip|execute|prove> Prover mode (default: execute)

Environment overrides:
  LIGETRON_PROVER, LIGETRON_VERIFIER, LIGETRON_SHADER_PATH
  LIGETRON_WASM_MOCK, LIGETRON_WASM_CELESTIA

Examples:
  bash run_ligetron.sh
  bash run_ligetron.sh --da celestia --storage nomt --mode prove
EOF
      exit 0 ;;
    *)
      echo "Unknown arg: $1" >&2; exit 1 ;;
  esac
done

# Default locations for Ligetron binaries if not provided by env.
: "${LIGETRON_PROVER:=$REPO_ROOT/crates/adapters/ligetron/test_binaries/webgpu_prover}"
: "${LIGETRON_VERIFIER:=$REPO_ROOT/crates/adapters/ligetron/test_binaries/webgpu_verifier}"
: "${LIGETRON_SHADER_PATH:=$REPO_ROOT/crates/adapters/ligetron/shader}"

export LIGETRON_PROVER LIGETRON_VERIFIER LIGETRON_SHADER_PATH
export SOV_PROVER_MODE="$MODE"

# Speed up builds: we don't need risc0/sp1 guests for Ligetron runs
export SKIP_GUEST_BUILD=1

# Enable Rust backtraces unless the caller overrides it
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

echo "==> Ligetron settings"
echo "    PROVER:   $LIGETRON_PROVER"
echo "    VERIFIER: $LIGETRON_VERIFIER"
echo "    SHADERS:  $LIGETRON_SHADER_PATH"
echo "    MODE:     $SOV_PROVER_MODE"
echo "    BACKTRACE:$RUST_BACKTRACE"
echo "    DA:       $DA_LAYER"
echo "    STORAGE:  $STORAGE"

# Build
echo "==> Building sov-demo-rollup (skip guest builds)"
cargo build -p sov-demo-rollup

# Run
BIN="$REPO_ROOT/target/debug/sov-demo-rollup"
echo "==> Launching: $BIN --zkvm ligetron --da-layer $DA_LAYER --storage $STORAGE"
exec "$BIN" --zkvm ligetron --da-layer "$DA_LAYER" --storage "$STORAGE"
