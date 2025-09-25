#![deny(missing_docs)]
//! # Ligetron Adapter
//!
//! This crate contains an adapter allowing Ligetron to be used as a proof system for
//! Sovereign SDK rollups.
//!
//! Ligetron is a lightweight, scalable zero-knowledge proof system that uses WASM as an
//! intermediate representation and leverages WebGPU for efficient proof generation.

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use sov_rollup_interface::zk::{CodeCommitment, CryptoSpec, ZkVerifier};
#[cfg(feature = "native")]
use anyhow::Context;
use thiserror::Error;
#[cfg(feature = "native")]
use std::time::{SystemTime, UNIX_EPOCH};

/// Safely truncate output to prevent secret spill in logs
#[cfg(feature = "native")]
fn clip_output(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}… [truncated {} chars]", &s[..max], s.len() - max)
    } else {
        s.to_owned()
    }
}

/// Run verifier binary with config, trying privacy-safe CLI compatibility modes
#[cfg(feature = "native")]
fn run_verifier_with_config(
    bin: &str,
    cwd: &std::path::Path,
    config_json: &str,
) -> anyhow::Result<std::process::Output> {
    use std::io::Write;

    // 1) Try stdin first (privacy-safe)
    if let Ok(mut child) = std::process::Command::new(bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(cwd)
        .spawn()
    {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(config_json.as_bytes());
        }
        if let Ok(output) = child.wait_with_output() {
            if !output.stdout.is_empty() || !output.stderr.is_empty() || output.status.success() {
                return Ok(output);
            }
        }
    }

    // 2) Fallback: --config <tempfile> with 0600 perms (privacy-safe)
    let mut cfg = tempfile::NamedTempFile::new_in(cwd)
        .context("Failed to create temp config file")?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(cfg.path(), std::fs::Permissions::from_mode(0o600))
            .context("Failed to set config file permissions")?;
    }
    
    cfg.write_all(config_json.as_bytes())
        .context("Failed to write config to temp file")?;
    let cfg_path = cfg.path().to_path_buf();

    let output = std::process::Command::new(bin)
        .arg("--config")
        .arg(&cfg_path)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("Failed to run verifier with --config");
    
    if let Ok(output) = output {
        return Ok(output); // file is removed when cfg drops
    }

    // 3) Final fallback: pass JSON directly as argument (if not too large)
    if config_json.len() < 32768 { // Avoid hitting command line limits
        let output = std::process::Command::new(bin)
            .arg(config_json)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .context("Failed to run verifier with JSON argument")?;
        
        return Ok(output);
    }

    anyhow::bail!("All methods to pass config to {} failed", bin)
}

pub mod crypto;
pub mod guest;
#[cfg(feature = "native")]
pub mod host;

use crate::crypto::{LigetronPublicKey, LigetronSignature};

// Re-export key types
pub use guest::LigetronGuest;
#[cfg(feature = "native")]
pub use host::LigetronHost;

/// Typed argument for Ligetron CLI, matching the JSON schema
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LigetronArg {
    /// String argument: `{"str": "value"}`
    Str(String),
    /// 64-bit integer argument: `{"i64": 42}`
    I64(i64),
    /// Hex-encoded bytes argument: `{"hex": "0xdeadbeef"}`
    Hex(Vec<u8>),
}

/// 32-byte method ID: `sha256(program_wasm || packing_le)`.
/// This uniquely identifies a Ligetron program configuration and serves as the code commitment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LigetronMethodId([u8; 32]);

impl CodeCommitment for LigetronMethodId {
    type DecodeError = LigetronMethodIdError;

    fn encode(&self) -> Vec<u8> {
        self.0.to_vec()
    }

    fn decode(data: &[u8]) -> Result<Self, Self::DecodeError> {
        let array: [u8; 32] = data.try_into()
            .map_err(|_| LigetronMethodIdError::InvalidLength { found: data.len() })?;
        Ok(Self(array))
    }
}

/// An error that can occur when converting a byte vector to a `LigetronMethodId`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LigetronMethodIdError {
    /// The input was not 32 bytes long.
    #[error("LigetronMethodId must be 32 bytes long, but the input was {found} bytes long")]
    InvalidLength {
        /// The length of the input.
        found: usize,
    },
}

/// The cryptographic primitives provided by Ligetron.
/// Uses ed25519 for signatures and SHA-256 for hashing, consistent with other adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LigetronCryptoSpec;

impl CryptoSpec for LigetronCryptoSpec {
    #[cfg(feature = "native")]
    type PrivateKey = crate::crypto::private_key::LigetronPrivateKey;
    type PublicKey = LigetronPublicKey;
    type Hasher = sha2::Sha256;
    type Signature = LigetronSignature;

    fn sovereign_admin_pubkey() -> Self::PublicKey {
        let admin_pubkey_bytes: [u8; 32] = [
            0xf1, 0xac, 0x96, 0xb6, 0xad, 0x3c, 0xd6, 0xbd, 0xda, 0xf2, 0xc2, 0x3f, 0x08, 0x9d,
            0xe7, 0x3a, 0x68, 0x16, 0xf8, 0x92, 0xc1, 0xaf, 0x34, 0x5d, 0xf7, 0x0f, 0x9a, 0x57,
            0x3a, 0x86, 0xba, 0xcb,
        ];

        // This will panic if the bytes are invalid, which is fine for a hardcoded constant
        let pub_key = ed25519_dalek::VerifyingKey::from_bytes(&admin_pubkey_bytes)
            .expect("Invalid admin public key bytes");

        LigetronPublicKey { pub_key }
    }
}

/// The proof package that contains everything needed for verification.
/// This avoids having to guess local file paths and ensures reproducible verification.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LigetronProofPackage {
    /// Package format version for future compatibility
    pub version: u16,
    /// The complete WASM program bytes
    pub program_wasm: Vec<u8>,
    /// The packing parameter used for proof generation (must match for verification)
    pub packing: u32,
    /// Optional hint for shader path location
    pub shader_path_hint: Option<String>,
    /// 1-based indices of private arguments (Ligetron uses 1-based indexing)
    pub private_indices_1based: Vec<usize>,
    /// Full typed args exactly as used by the prover (for reproducible verification)
    pub args: Vec<LigetronArg>,
    /// The bincode-encoded public values (journal)
    pub journal_bytes: Vec<u8>,
    /// The raw proof data from Ligetron
    pub proof_bytes: Vec<u8>,
}

/// A verifier for Ligetron proofs.
#[derive(Default, Clone)]
pub struct LigetronVerifier;

#[cfg(feature = "native")]
impl ZkVerifier for LigetronVerifier {
    type CodeCommitment = LigetronMethodId;
    type CryptoSpec = LigetronCryptoSpec;
    type Error = anyhow::Error;

    fn verify<T: DeserializeOwned>(
        serialized_proof: &[u8],
        code_commitment: &Self::CodeCommitment,
    ) -> Result<T, Self::Error> {
        use sov_rollup_interface::zk::Proof;
        
        let proof: Proof<LigetronProofPackage, Vec<u8>> = bincode::deserialize(serialized_proof)?;

        match proof {
            Proof::PublicData(journal) => {
                // Parity with Risc0: PublicData means "no cryptographic verification"
                // Callers expecting a real proof should not pass this here.
                Ok(bincode::deserialize(&journal)?)
            }
            Proof::Full(pkg) => {
                // Existing verification code path

        // 1. Check package version compatibility
        if pkg.version != 1 {
            anyhow::bail!("Unsupported proof package version: {}. Expected version 1.", pkg.version);
        }

        // 2. Verify method ID matches SHA-256 of program bytes + packing
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&pkg.program_wasm);
        // Include packing in the commitment since it affects the proof
        hasher.update(&pkg.packing.to_le_bytes());
        let computed_hash = hasher.finalize();
        
        if computed_hash.as_slice() != &code_commitment.0 {
            anyhow::bail!("Ligetron method ID mismatch: expected {:?}, got {:?}", 
                         code_commitment.0, computed_hash.as_slice());
        }

        // 2. Create a unique per-run directory for the verifier and write required files
        let run_dir = tempfile::Builder::new().prefix("ligetron-verify-").tempdir()?;
        let run_path = run_dir.path();
        let _run_id = {
            let pid = std::process::id();
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
            format!("{}-{}", pid, ts)
        }; // For future use in config file naming
        let program_path = run_path.join("program.wasm");
        let proof_path = run_path.join("proof.data");
        
        std::fs::write(&program_path, &pkg.program_wasm)?;
        std::fs::write(&proof_path, &pkg.proof_bytes)?;

        // 3. Validation checks
        anyhow::ensure!(pkg.args.len() >= 1, "args must include digest at index 0");
        anyhow::ensure!(
            !pkg.private_indices_1based.contains(&1),
            "Digest (arg #1) must be public"
        );

        // 4. Compute journal digest for verification and bind it to arg[0]
        let mut hasher = Sha256::new();
        hasher.update(&pkg.journal_bytes);
        let journal_digest = hasher.finalize().to_vec();

        // Convert private indices to HashSet for efficient lookup
        let private_indices: std::collections::HashSet<usize> = 
            pkg.private_indices_1based.iter().copied().collect();

        // 5. Reconstruct args with private arguments zeroed and digest bound
        let mut verification_args = Vec::new();
        
        for (i, arg) in pkg.args.iter().enumerate() {
            let is_private = private_indices.contains(&(i + 1)); // Convert to 1-based
            
            if i == 0 {
                // Force arg[0] to the recomputed digest, ignore whatever came in package
                verification_args.push(LigetronArg::Hex(journal_digest.clone()));
                continue;
            }
            
            if is_private {
                // Replace private args with zeroed values of same type/length
                match arg {
                    LigetronArg::Hex(bytes) => {
                        let zero_bytes = vec![0u8; bytes.len()];
                        verification_args.push(LigetronArg::Hex(zero_bytes));
                    }
                    LigetronArg::Str(s) => {
                        let zero_str = "0".repeat(s.len());
                        verification_args.push(LigetronArg::Str(zero_str));
                    }
                    LigetronArg::I64(_) => {
                        verification_args.push(LigetronArg::I64(0));
                    }
                }
            } else {
                // Keep public args as-is
                verification_args.push(arg.clone());
            }
        }
        
        // Convert to JSON for CLI
        let json_args: Vec<serde_json::Value> = verification_args.iter()
            .map(|arg| match arg {
                LigetronArg::Str(s) => serde_json::json!({"str": s}),
                LigetronArg::I64(i) => serde_json::json!({"i64": i}),
                LigetronArg::Hex(bytes) => serde_json::json!({"hex": format!("0x{}", hex::encode(bytes))}),
            })
            .collect();

        // 6. Build verifier JSON configuration
        let verifier_config = serde_json::json!({
            "program": program_path.to_string_lossy(),
            "shader-path": pkg.shader_path_hint.as_deref().unwrap_or("./shader"),
            "packing": pkg.packing,
            "private-indices": pkg.private_indices_1based,
            "args": json_args
        });

        // 7. Run webgpu_verifier and check exit status
        let verifier_bin = std::env::var("LIGETRON_VERIFIER")
            .unwrap_or_else(|_| "webgpu_verifier".to_string());
        
        let output = run_verifier_with_config(&verifier_bin, run_path, &verifier_config.to_string())
            .context("Failed to run Ligetron verifier")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            
            // Safely clip stdout/stderr to prevent secret spill
            let (stdout_show, stderr_show) = if std::env::var("LIGETRON_LOG_FULL_IO").is_ok() {
                (stdout.to_string(), stderr.to_string())
            } else {
                (clip_output(&stdout, 2048), clip_output(&stderr, 2048))
            };
            
            anyhow::bail!(
                "Ligetron verification failed. Exit code: {:?}\nStdout: {}\nStderr: {}", 
                output.status.code(), stdout_show, stderr_show
            );
        }

                // 7. Deserialize and return the journal as type T
                Ok(bincode::deserialize(&pkg.journal_bytes)?)
            }
        }
    }
}

#[cfg(not(feature = "native"))]
impl ZkVerifier for LigetronVerifier {
    type CodeCommitment = LigetronMethodId;
    type CryptoSpec = LigetronCryptoSpec;
    type Error = anyhow::Error;

    fn verify<T: DeserializeOwned>(
        _serialized_proof: &[u8],
        _code_commitment: &Self::CodeCommitment,
    ) -> Result<T, Self::Error> {
        // Verification is not supported in non-native environments
        // since it requires file system access and external binaries
        anyhow::bail!("Ligetron verification is only supported with the 'native' feature")
    }
}

/// The Ligetron zkVM implementation.
#[derive(Debug, Clone, Default, PartialEq, Eq, schemars::JsonSchema)]
pub struct Ligetron;

impl sov_rollup_interface::zk::Zkvm for Ligetron {
    type Verifier = LigetronVerifier;
    type Guest = crate::guest::LigetronGuest;

    #[cfg(feature = "native")]
    type Host = crate::host::LigetronHost<'static>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use sov_rollup_interface::crypto::PublicKey;

    #[test]
    fn test_sovereign_admin_pubkey() {
        let pub_key = LigetronCryptoSpec::sovereign_admin_pubkey();
        let credential_id = pub_key.credential_id();
        assert_eq!(
            credential_id.to_string(),
            "0xf1ac96b6ad3cd6bddaf2c23f089de73a6816f892c1af345df70f9a573a86bacb"
        );
    }

    #[test]
    fn test_method_id_codec_roundtrip() {
        let raw_data = [1u8; 32];
        let method_id = LigetronMethodId(raw_data);
        let encoded = method_id.encode();
        let decoded = LigetronMethodId::decode(&encoded).expect("Encoding should be valid");
        assert_eq!(decoded.0, raw_data);

        // Test error case with wrong length
        let bad_data = vec![1u8; 31];
        assert!(matches!(
            LigetronMethodId::decode(&bad_data),
            Err(LigetronMethodIdError::InvalidLength { found: 31 })
        ));
    }

    #[test]
    fn test_proof_package_serialization() {
        let package = LigetronProofPackage {
            version: 1,
            program_wasm: vec![1, 2, 3, 4],
            packing: 8192,
            shader_path_hint: Some("./shader".to_string()),
            private_indices_1based: vec![2, 3], // Args 2 and 3 are private (1-based)
            args: vec![
                LigetronArg::Hex(vec![0xde, 0xad, 0xbe, 0xef]), // Public digest
                LigetronArg::Hex(vec![0x01, 0x02, 0x03, 0x04]), // Private chunk 1
                LigetronArg::Hex(vec![0x05, 0x06, 0x07, 0x08]), // Private chunk 2
            ],
            journal_bytes: vec![5, 6, 7, 8],
            proof_bytes: vec![9, 10, 11, 12],
        };

        let serialized = bincode::serialize(&package).expect("Serialization should work");
        let deserialized: LigetronProofPackage = bincode::deserialize(&serialized)
            .expect("Deserialization should work");
        
        assert_eq!(package.version, deserialized.version);
        assert_eq!(package.program_wasm, deserialized.program_wasm);
        assert_eq!(package.packing, deserialized.packing);
        assert_eq!(package.shader_path_hint, deserialized.shader_path_hint);
        assert_eq!(package.private_indices_1based, deserialized.private_indices_1based);
        assert_eq!(package.args, deserialized.args);
        assert_eq!(package.journal_bytes, deserialized.journal_bytes);
        assert_eq!(package.proof_bytes, deserialized.proof_bytes);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_packing_affects_method_id() {
        use crate::host::LigetronHost;
        use sov_rollup_interface::zk::ZkvmHost;

        let wasm_bytes = b"fake wasm program";
        
        // Create hosts with different packing values
        let host1 = LigetronHost::new(wasm_bytes).with_packing(4096);
        let host2 = LigetronHost::new(wasm_bytes).with_packing(8192);
        
        // Get method IDs (code commitments)
        let method_id1 = host1.code_commitment();
        let method_id2 = host2.code_commitment();
        
        // Different packing should yield different method IDs
        assert_ne!(method_id1, method_id2, 
            "Different packing values should produce different method IDs to prevent proof replay attacks");
        
        // Same packing should yield same method ID
        let host3 = LigetronHost::new(wasm_bytes).with_packing(4096);
        let method_id3 = host3.code_commitment();
        assert_eq!(method_id1, method_id3, 
            "Same WASM and packing should produce identical method IDs");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_hint_round_trip() {
        use crate::host::LigetronHost;
        use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};

        let wasm_bytes = b"fake wasm program";
        let mut host = LigetronHost::new(wasm_bytes);
        
        // Add various hint types
        let hint1 = 42u64;
        let hint2 = "test string".to_string();
        let hint3 = vec![1u8, 2, 3, 4, 5];
        
        host.add_hint(&hint1);
        host.add_hint(&hint2);
        host.add_hint(&hint3);
        
        // Simulate execution to get the guest
        let guest = host.simulate_with_hints();
        
        // Read hints back from guest in same order
        let read_hint1: u64 = guest.read_from_host();
        let read_hint2: String = guest.read_from_host();
        let read_hint3: Vec<u8> = guest.read_from_host();
        
        // Verify round-trip
        assert_eq!(hint1, read_hint1);
        assert_eq!(hint2, read_hint2);
        assert_eq!(hint3, read_hint3);
    }

    #[test]
    fn test_digest_binding_validation() {
        use sha2::{Digest, Sha256};
        use sov_rollup_interface::zk::Proof;
        
        // Test that validation checks work correctly with Proof::Full
        let real_journal = b"test journal data";
        
        // Test 1: Empty args should fail
        let pkg_empty_args = LigetronProofPackage {
            version: 1,
            program_wasm: b"fake_wasm".to_vec(),
            packing: 42,
            shader_path_hint: None,
            args: vec![], // Empty args
            private_indices_1based: vec![],
            journal_bytes: real_journal.to_vec(),
            proof_bytes: b"fake_proof".to_vec(),
        };
        
        let mut hasher = Sha256::new();
        hasher.update(&pkg_empty_args.program_wasm);
        hasher.update(&pkg_empty_args.packing.to_le_bytes());
        let method_id = LigetronMethodId(hasher.finalize().into());
        
        // Use Proof::Full to trigger validation (PublicData bypasses validation)
        let proof_envelope: Proof<LigetronProofPackage, Vec<u8>> = Proof::Full(pkg_empty_args);
        let serialized = bincode::serialize(&proof_envelope).unwrap();
        let result = LigetronVerifier::verify::<Vec<u8>>(&serialized, &method_id);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("args must include digest"));
        
        // Test 2: Digest marked as private should fail
        let pkg_private_digest = LigetronProofPackage {
            version: 1,
            program_wasm: b"fake_wasm".to_vec(),
            packing: 42,
            shader_path_hint: None,
            args: vec![
                LigetronArg::Hex(vec![0u8; 32]), // Digest
            ],
            private_indices_1based: vec![1], // Mark digest as private (should fail)
            journal_bytes: real_journal.to_vec(),
            proof_bytes: b"fake_proof".to_vec(),
        };
        
        let proof_envelope2: Proof<LigetronProofPackage, Vec<u8>> = Proof::Full(pkg_private_digest);
        let serialized = bincode::serialize(&proof_envelope2).unwrap();
        let result = LigetronVerifier::verify::<Vec<u8>>(&serialized, &method_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Digest (arg #1) must be public"));
    }
}
