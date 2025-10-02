//! ZK-POC helpers: generate proofs and build JSON calls.

use serde_json::json;
use sov_modules_api::clap;
use sov_rollup_interface::zk::ZkvmHost;

/// ZK-POC-related helper subcommands.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum ZkPocWorkflow {
    /// Build a JSON call for `set_value`, generating the required RISC0 proof.
    /// Prints a JSON envelope you can pass to `sov-cli transactions import from-string`.
    BuildSetValueJson {
        /// The module key in the runtime JSON envelope (e.g., "zk-poc").
        #[clap(long, default_value = "zk-poc")]
        module_key: String,
        /// The value to set (must be even; the guest enforces it).
        #[clap(long)]
        value: u64,
        /// Optional path to write the JSON output to (pretty-printed).
        /// If omitted, prints to stdout.
        #[clap(long)]
        out: Option<std::path::PathBuf>,
    },
}

impl ZkPocWorkflow {
    /// Executes the selected ZK-POC workflow and prints the result to stdout.
    pub fn run(self) -> anyhow::Result<()> {
        match self {
            ZkPocWorkflow::BuildSetValueJson { module_key, value, out } => {
                // Use the embedded RISC0 guest ELF to prove evenness of `value` and emit a JSON call.
                let elf = zk_poc_risc0_methods::EVEN_ELF;
                anyhow::ensure!(
                    !elf.is_empty(),
                    "ZK-POC guest ELF is empty. Ensure the RISC0 toolchain is installed or unset SKIP_GUEST_BUILD."
                );

                let mut host = sov_risc0_adapter::host::Risc0Host::new(elf);
                host.add_hint(value);
                // Generate a real RISC0 receipt (serialized Proof<Receipt>)
                let proof_bytes = ZkvmHost::run(&mut host, true)?;

                let body = json!({
                    module_key: {
                        "set_value": {
                            "value": value,
                            // UniversalWallet accepts hex for Vec<u8> by default.
                            "proof": format!("0x{}", hex::encode(proof_bytes)),
                        }
                    }
                });
                let text = serde_json::to_string_pretty(&body)?;
                if let Some(path) = out {
                    std::fs::write(&path, text)?;
                    eprintln!("Wrote zk-poc set_value JSON to {}", path.display());
                } else {
                    println!("{}", text);
                }
                Ok(())
            }
        }
    }
}
