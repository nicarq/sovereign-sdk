//! Privacy helpers for midnight-privacy: derive commitments and build JSON calls.

use rand::RngCore;
use serde_json::json;
use sov_modules_api::clap;

/// Privacy-related helper subcommands.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum PrivacyWorkflow {
    /// Derive a random 32-byte commitment (hex). Useful for deposits.
    DeriveCommitment {
        /// Optional hex-encoded 32-byte seed to deterministically derive the commitment.
        #[clap(long)]
        seed_hex: Option<String>,
    },
    /// Build a JSON call for a Deposit, computing or injecting the commitment.
    BuildDepositJson {
        /// The module key in the runtime JSON envelope (e.g., "midnight-privacy").
        #[clap(long, default_value = "midnight-privacy")]
        module_key: String,
        /// Token id (bech32 string).
        #[clap(long)]
        token_id: String,
        /// Amount (u128 integer).
        #[clap(long)]
        amount: String,
        /// Optional hex-encoded 32-byte commitment; if omitted, a random one is generated.
        #[clap(long)]
        commitment_hex: Option<String>,
    },
}

impl PrivacyWorkflow {
    /// Executes the selected privacy workflow and prints the result to stdout.
    pub fn run(self) -> anyhow::Result<()> {
        match self {
            PrivacyWorkflow::DeriveCommitment { seed_hex } => {
                let commitment = if let Some(s) = seed_hex {
                    parse_hex_32(&s)?
                } else {
                    random_32()
                };
                println!("0x{}", hex::encode(commitment));
                Ok(())
            }
            PrivacyWorkflow::BuildDepositJson { module_key, token_id, amount, commitment_hex } => {
                let commitment = if let Some(h) = commitment_hex {
                    parse_hex_32(&h)?
                } else {
                    random_32()
                };
                let body = json!({
                    module_key: {
                        "deposit": {
                            "token_id": token_id,
                            "amount": amount.parse::<u128>()?,
                            "commitment": format!("0x{}", hex::encode(commitment)),
                        }
                    }
                });
                println!("{}", serde_json::to_string_pretty(&body)?);
                Ok(())
            }
        }
    }
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

fn parse_hex_32(s: &str) -> anyhow::Result<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(s)?;
    anyhow::ensure!(bytes.len() == 32, "expected 32-byte hex");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}
