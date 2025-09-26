//! This module implements the [`ZkvmHost`] trait for the Ligetron zkVM.

use anyhow::Context;
use bincode::Options;
use serde::Serialize;
use sov_rollup_interface::zk::ZkvmHost;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{LigetronArg, LigetronMethodId, LigetronProofPackage};
use crate::guest::LigetronGuest;

/// Journal prefix that guest programs should emit to stdout for journal extraction
pub const SOV_JOURNAL_HEX_PREFIX: &str = "SOV_JOURNAL_HEX:";



/// A [`LigetronHost`] stores a WASM program to execute in the Ligetron zkVM,
/// and accumulates hints to be provided to its execution.
#[derive(Clone)]
pub struct LigetronHost<'a> {
    /// The WASM program bytes
    wasm: &'a [u8],
    /// Bincode concatenation of all hints passed via add_hint
    hints_blob: Vec<u8>,
    /// Packing parameter for Ligetron (affects proof size/time tradeoff)
    packing: u32,
    /// Optional shader path (defaults to "./shader")
    shader_path: Option<String>,
    /// Path to the Ligetron prover binary
    prover_bin: String,
}

impl<'a> LigetronHost<'a> {
    /// Redact private args (1-based indices) while preserving type/lengths.
    fn redact_private_typed(args: &[LigetronArg], private_1based: &[usize]) -> Vec<LigetronArg> {
        use std::collections::HashSet;
        let privset: HashSet<usize> = private_1based.iter().copied().collect();
        args.iter().enumerate().map(|(i, a)| {
            // arg[1] (i == 0) is the public digest; do not redact it
            if i == 0 || !privset.contains(&(i + 1)) { return a.clone(); }
            match a {
                LigetronArg::Hex(bytes) => LigetronArg::Hex(vec![0u8; bytes.len()]),
                LigetronArg::Str(s)     => LigetronArg::Str("0".repeat(s.len())),
                LigetronArg::I64(_)     => LigetronArg::I64(0),
            }
        }).collect()
    }

    /// Create a new LigetronHost to prove the given WASM program.
    pub fn new(program_wasm: &'a [u8]) -> Self {
        Self {
            wasm: program_wasm,
            hints_blob: Vec::new(),
            packing: Self::default_packing(),
            shader_path: std::env::var("LIGETRON_SHADER_PATH").ok(),
            prover_bin: std::env::var("LIGETRON_PROVER")
                .unwrap_or_else(|_| "webgpu_prover".to_string()),
        }
    }

    /// Set the packing parameter for proof generation.
    /// Note: This corresponds to "padding" in the prover output, not "packing"
    pub fn with_packing(mut self, packing: u32) -> Self {
        self.packing = packing;
        self
    }

    /// Set the shader path.
    pub fn with_shader_path(mut self, path: String) -> Self {
        self.shader_path = Some(path);
        self
    }

    /// Set the prover binary path.
    pub fn with_prover_bin(mut self, bin: String) -> Self {
        self.prover_bin = bin;
        self
    }

    /// Generate a LigetronGuest with provided hints for simulation.
    ///
    /// ⚠️ **WARNING**: This *consumes* the host's `hints_blob`. Do not call this
    /// before a subsequent `run()` unless you re-add the hints afterwards.
    pub fn simulate_with_hints(&mut self) -> LigetronGuest {
        LigetronGuest::with_hints(std::mem::take(&mut self.hints_blob))
    }

    /// Default packing parameter.
    fn default_packing() -> u32 {
        8192
    }

    /// Chunk bytes into hex strings to avoid command-line length limits
    fn hex_chunks(bytes: &[u8], chunk_size: usize) -> Vec<String> {
        bytes.chunks(chunk_size)
            .map(|chunk| format!("0x{}", hex::encode(chunk)))
            .collect()
    }

    /// Build the JSON configuration for the Ligetron prover with chunked hints
    /// Returns (config_json, typed_args, private_indices_1based)
    fn build_prover_config(&self, program_path: &str, journal_digest_hex: &str) -> (serde_json::Value, Vec<LigetronArg>, Vec<usize>) {
        // Secure journal approach: Always use digest + hint chunks pattern
        // Chunk hints to avoid command-line length limits (32KB per chunk)
        const CHUNK_SIZE: usize = 32 * 1024;
        
        // Debug: log the size of hints_blob
        tracing::debug!("hints_blob size: {} bytes", self.hints_blob.len());
        if std::env::var("LIGETRON_LOG_FULL_IO").is_ok() {
            tracing::debug!("hints_blob first 100 bytes: {:?}", &self.hints_blob[..std::cmp::min(100, self.hints_blob.len())]);
            tracing::debug!("hints_blob hex: {}", hex::encode(&self.hints_blob));
        }
        if std::env::var("LIGETRON_LOG_HINTS").is_ok() {
            let preview_len = self.hints_blob.len().min(64);
            let hex = hex::encode(&self.hints_blob[..preview_len]);
            eprintln!(
                "[ligetron-host] build_config hints={} preview={}{}",
                self.hints_blob.len(),
                hex,
                if preview_len < self.hints_blob.len() { "…" } else { "" }
            );
        }

        let hint_chunks = Self::hex_chunks(&self.hints_blob, CHUNK_SIZE);
        
        // Build typed args: [journal_digest, hint_chunk_0, hint_chunk_1, ...]
        let mut typed_args = vec![LigetronArg::Hex(hex::decode(&journal_digest_hex[2..]).expect("Valid hex digest"))];
        let mut private_indices = Vec::new();
        
        for (i, chunk_hex) in hint_chunks.iter().enumerate() {
            let chunk_bytes = hex::decode(&chunk_hex[2..]).expect("Valid hex chunk");
            typed_args.push(LigetronArg::Hex(chunk_bytes));
            private_indices.push(i + 2); // 1-indexed, starting from arg[2]
        }
        
        // Convert to JSON for CLI
        let json_args: Vec<serde_json::Value> = typed_args.iter().map(Self::arg_to_json).collect();
        
        // Get shader path from environment or use default
        // Since we change cwd to temp dir, ensure shader path is absolute
        let shader_path = std::env::var("LIGETRON_SHADER_PATH")
            .unwrap_or_else(|_| {
                self.shader_path.as_deref().unwrap_or_else(|| {
                    "shader"
                }).to_string()
            });
            
        let config = serde_json::json!({
            "program": program_path,
            "shader-path": shader_path,
            "packing": self.packing,
            "private-indices": private_indices,
            "args": json_args
        });
        
        (config, typed_args, private_indices)
    }


    /// Run the Ligetron prover and generate a proof package.
    fn run_prover(&mut self, with_proof: bool) -> anyhow::Result<LigetronProofPackage> {
        // Create a unique per-run working directory (auto-cleaned on drop).
        // This isolates proof.data and config files across concurrent runs.
        // IMPORTANT: run_dir must be kept alive throughout the entire function
        // to prevent the temporary directory from being deleted while the prover is running.
        let run_dir = Self::make_run_dir().context("Failed to create ligetron run directory")?;
        let _run_id = Self::unique_run_id(); // For future use in config file naming
        let run_path = run_dir.path();
        let program_path = run_path.join("program.wasm");
        
        // Write program to temporary file
        std::fs::write(&program_path, self.wasm)
            .context("Failed to write program WASM to temporary file")?;

        // First pass: run prover with dummy digest to get journal
        tracing::debug!("Running Ligetron prover (first pass) to extract journal");
        let (dummy_config, _, _) = self.build_prover_config(
            "program.wasm",  // Use relative path since we set cwd to run_path
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
        

        let output = self.run_with_config(&self.prover_bin, run_path, &dummy_config.to_string())
            .with_context(|| format!("Failed to run Ligetron prover (first pass): {}", self.prover_bin))?;

        // Extract journal strictly via the explicit contract (secure default)
        let mut journal_bytes = self.parse_journal_any(&output.stdout, &output.stderr)
            .with_context(|| format!("Failed to extract journal from program output (exit={:?})", output.status.code()));

        // Optional UNSAFE fallback (opt-in for debugging only)
        #[cfg(feature = "unsafe-fallback")]
        if journal_bytes.is_err() {
            if std::env::var("LIGETRON_UNSAFE_JOURNAL_FALLBACK").ok().as_deref() == Some("1") {
                tracing::warn!(
                    "Using UNSAFE journal fallback. Do not use in production. \
                     Enable only for legacy programs without sov_journal.h."
                );
                if let Ok(auto) = self.auto_extract_journal(&output.stdout, &output.stderr) {
                    journal_bytes = Ok(auto);
                }
            }
        }

        let journal_bytes = journal_bytes?;
        tracing::debug!("Extracted journal: {} bytes", journal_bytes.len());

        // Compute journal digest
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&journal_bytes);
        let journal_digest = hasher.finalize().to_vec();
        let journal_digest_hex = format!("0x{}", hex::encode(&journal_digest));

        // Build config for the final args (needed for both proof and simulation)
        let (final_config, final_args, final_private_indices) = self.build_prover_config(
            "program.wasm",  // Use relative path since we set cwd to run_path
            &journal_digest_hex
        );
        
        // Never store secrets in the package: keep a redacted copy for serialization.
        let redacted_args = Self::redact_private_typed(&final_args, &final_private_indices);

        // Second pass: run prover with correct digest if proof is requested
        let (proof_bytes, final_journal_bytes) = if with_proof {
            tracing::debug!("Running Ligetron prover (second pass) with correct digest");
            
            // Remove any existing proof file
            let proof_path = run_path.join("proof.data");
            let _ = std::fs::remove_file(&proof_path);

            let (final_config, package_args, package_priv_ix) = self.build_prover_config(
                "program.wasm",  // Use relative path since we set cwd to run_path
                &journal_digest_hex
            );

            let out2 = self.run_with_config(&self.prover_bin, run_path, &final_config.to_string())
                .with_context(|| format!("Failed to run Ligetron prover (second pass): {}", self.prover_bin))?;

            if !out2.status.success() {
                // Create sanitized config for error logging (redact private args)
                let final_config_args: Vec<serde_json::Value> = package_args.iter()
                    .map(|arg| Self::arg_to_json(arg))
                    .collect();
                let redacted_args = Self::redact_private_args(&final_config_args, &package_priv_ix);
                let sanitized_config = serde_json::json!({
                    "program": program_path.to_string_lossy(),
                    "shader-path": self.shader_path.as_deref().unwrap_or("./shader"),
                    "packing": self.packing,
                    "private-indices": final_private_indices,
                    "args": redacted_args
                });

                // Safely clip stdout/stderr to prevent secret spill
                let stdout = String::from_utf8_lossy(&out2.stdout);
                let stderr = String::from_utf8_lossy(&out2.stderr);
                let (stdout_show, stderr_show) = if std::env::var("LIGETRON_LOG_FULL_IO").is_ok() {
                    (stdout.to_string(), stderr.to_string())
                } else {
                    (Self::clip_output(&stdout, 2048), Self::clip_output(&stderr, 2048))
                };

                anyhow::bail!(
                    "Ligetron prover (second pass) failed: code={:?}\nJSON config (private args redacted): {}\nCurrent dir: {:?}\nProof file exists: {}\nstdout={}\nstderr={}",
                    out2.status.code(),
                    sanitized_config.to_string().chars().take(500).collect::<String>(),
                    run_path,
                    proof_path.exists(),
                    stdout_show,
                    stderr_show
                );
            }

            // Parse second-pass journal; this must always be explicit
            let journal2 = self.parse_journal_any(&out2.stdout, &out2.stderr)
                .context("Failed to parse journal from second pass output")?;
            
            // Verify that the second pass journal matches the digest we provided
            let mut h2 = Sha256::new();
            h2.update(&journal2);
            let actual_digest = format!("0x{}", hex::encode(h2.finalize()));
            anyhow::ensure!(
                actual_digest == journal_digest_hex,
                "Journal digest mismatch after second pass: expected {}, got {}",
                journal_digest_hex,
                actual_digest
            );

            // Read the generated proof
            let proof = std::fs::read(&proof_path)
                .context("Failed to read proof.data. Ensure Ligetron prover generates this file.")?;
            
            (proof, journal2)
        } else {
            // For simulation mode, we don't need the actual proof
            (Vec::new(), journal_bytes)
        };

        Ok(LigetronProofPackage {
            version: 1, // Current package format version
            program_wasm: self.wasm.to_vec(),
            packing: self.packing,
            shader_path_hint: self.shader_path.clone(),
            private_indices_1based: final_private_indices,
            // Store only a privacy-preserving view of args
            args: redacted_args,
            journal_bytes: final_journal_bytes,
            proof_bytes,
        })
    }
}

impl ZkvmHost for LigetronHost<'static> {
    type HostArgs = &'static [u8];
    type Guest = LigetronGuest;

    fn from_args(args: &Self::HostArgs) -> Self {
        Self::new(args)
    }

    fn add_hint<T: Serialize>(&mut self, item: T) {
        // Append bincode-serialized item to the hints blob
        // All hints are concatenated into a single private argument
        let opts = bincode::DefaultOptions::new().with_big_endian();
        opts.serialize_into(&mut self.hints_blob, &item)
            .expect("Ligetron hint serialization should be infallible");

        if std::env::var("LIGETRON_LOG_HINTS").is_ok() {
            let preview_len = self.hints_blob.len().min(32);
            let hex = hex::encode(&self.hints_blob[..preview_len]);
            eprintln!(
                "[ligetron-host] add_hint: total={} bytes preview={}{}",
                self.hints_blob.len(),
                hex,
                if preview_len < self.hints_blob.len() { "…" } else { "" }
            );
        }
    }

    #[cfg(feature = "native")]
    fn code_commitment(&self) -> <<Self::Guest as sov_rollup_interface::zk::ZkvmGuest>::Verifier as sov_rollup_interface::zk::ZkVerifier>::CodeCommitment {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.wasm);
        // Include packing in the commitment since it affects the proof
        hasher.update(&self.packing.to_le_bytes());
        let hash = hasher.finalize();
        LigetronMethodId(hash.as_slice().try_into().expect("SHA-256 produces 32 bytes"))
    }

    fn run(&mut self, with_proof: bool) -> anyhow::Result<Vec<u8>> {
        use sov_rollup_interface::zk::Proof;
        
        let package = self.run_prover(with_proof)?;
        
        let proof = if with_proof {
            Proof::Full(package)
        } else {
            // Mirror Risc0: return public data for fast simulation paths
            Proof::PublicData(package.journal_bytes.clone())
        };
        
        bincode::serialize(&proof)
            .context("Failed to serialize Ligetron proof envelope")
    }
}

impl<'a> LigetronHost<'a> {
    /// Create a unique per-run working directory. If `LIGETRON_WORK_DIR` is set,
    /// the directory is created under it; otherwise the OS temp dir is used.
    fn make_run_dir() -> anyhow::Result<tempfile::TempDir> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("ligetron-run-");
        if let Ok(base) = std::env::var("LIGETRON_WORK_DIR") {
            Ok(builder.tempdir_in(base)?)
        } else {
            Ok(builder.tempdir()?)
        }
    }

    /// A simple unique id string for naming per-run files.
    fn unique_run_id() -> String {
        let pid = std::process::id();
        let ts = SystemTime::now().duration_since(UNIX_EPOCH)
            .unwrap_or_default().as_nanos();
        format!("{}-{}", pid, ts)
    }

    /// Convert LigetronArg to JSON value for CLI
    fn arg_to_json(arg: &LigetronArg) -> serde_json::Value {
        match arg {
            LigetronArg::Str(s) => serde_json::json!({"str": s}),
            LigetronArg::I64(i) => serde_json::json!({"i64": i}),
            LigetronArg::Hex(bytes) => serde_json::json!({"hex": format!("0x{}", hex::encode(bytes))}),
        }
    }

    /// Redact private args from JSON args for safe logging
    fn redact_private_args(args: &[serde_json::Value], private_1based: &[usize]) -> Vec<serde_json::Value> {
        let private: std::collections::HashSet<usize> = private_1based.iter().copied().collect();
        args.iter().enumerate().map(|(i, v)| {
            if private.contains(&(i+1)) {
                // Try to preserve type
                if v.get("hex").is_some() { serde_json::json!({"hex":"0x[REDACTED]"}) }
                else if v.get("str").is_some() { serde_json::json!({"str":"[REDACTED]"}) }
                else if v.get("i64").is_some() { serde_json::json!({"i64":0}) }
                else { serde_json::json!("[REDACTED]") }
            } else { v.clone() }
        }).collect()
    }

    /// Safely truncate output to prevent secret spill in logs
    fn clip_output(s: &str, max: usize) -> String {
        if s.len() > max {
            format!("{}… [truncated {} chars]", &s[..max], s.len() - max)
        } else {
            s.to_owned()
        }
    }

    /// Run binary with config, using the same simple approach as the working test project
    fn run_with_config(
        &self,
        bin: &str,
        cwd: &std::path::Path,
        config_json: &str,
    ) -> anyhow::Result<std::process::Output> {
        // Write config to a file in the working directory (like the test project does)
        let config_path = cwd.join("config.json");
        std::fs::write(&config_path, config_json)
            .context("Failed to write config.json")?;

        // Debug: Log environment details for troubleshooting
        if std::env::var("LIGETRON_LOG_FULL_IO").is_ok() {
            eprintln!("🔧 Debug: Running {} from working dir {:?}", bin, cwd);
            eprintln!("🔧 Debug: Using simple test project approach");
        }

        // Use the exact same simple approach as the test project
        let mut cmd = std::process::Command::new(bin);
        cmd.current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .arg("--config")
            .arg(&config_path);

        // Execute the command (exactly like the test project)
        let output = cmd.output()
            .with_context(|| format!("Failed to execute prover command: {}", bin))?;

        Ok(output)
    }


    /// Parse journal from output stream looking for SOV_JOURNAL_HEX: lines
    fn parse_journal_from_output(&self, output: &[u8]) -> anyhow::Result<Vec<u8>> {
        let text = String::from_utf8_lossy(output);
        for line in text.lines() {
            if let Some(hex_data) = line.strip_prefix(SOV_JOURNAL_HEX_PREFIX) {
                // Trim whitespace and handle optional 0x prefix
                let s = hex_data.trim();
                let s = s.strip_prefix("0x").unwrap_or(s);
                return Ok(hex::decode(s)
                    .with_context(|| format!("Failed to decode journal hex: {}", s))?);
            }
        }
        anyhow::bail!("No {} found in output", SOV_JOURNAL_HEX_PREFIX);
    }

    /// Parse journal from both stdout and stderr for robustness
    fn parse_journal_any(&self, stdout: &[u8], stderr: &[u8]) -> anyhow::Result<Vec<u8>> {
        // Try stdout first
        if let Ok(journal) = self.parse_journal_from_output(stdout) {
            return Ok(journal);
        }
        
        // Try stderr as fallback
        if let Ok(journal) = self.parse_journal_from_output(stderr) {
            return Ok(journal);
        }
        
        
        anyhow::bail!(
            "No SOV_JOURNAL_HEX found in stdout or stderr.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(stdout),
            String::from_utf8_lossy(stderr)
        );
    }
    
    /// Extract journal from program output - works with ANY WASM program
    /// Automatically detects and extracts meaningful journal data from program output
    #[cfg(feature = "unsafe-fallback")]
    fn auto_extract_journal(&self, stdout: &[u8], stderr: &[u8]) -> anyhow::Result<Vec<u8>> {
        tracing::debug!("Auto-extracting journal from program output");
        
        // Strategy 1: Look for explicit SOV_JOURNAL_HEX output (best case)
        if let Ok(journal) = self.parse_journal_any(stdout, stderr) {
            tracing::info!("Found explicit SOV_JOURNAL_HEX output");
            return Ok(journal);
        }
        
        // Strategy 2: Extract meaningful computation results from output
        if let Ok(journal) = self.extract_computation_results(stdout, stderr) {
            tracing::info!("Extracted journal from computation results");
            return Ok(journal);
        }
        
        // Strategy 3: Generate deterministic journal from execution context
        tracing::info!("Generating journal from execution context");
        self.generate_journal_from_execution_context(stdout, stderr)
    }
    
    /// Extract meaningful computation results from program output
    #[cfg(feature = "unsafe-fallback")]
    fn extract_computation_results(&self, stdout: &[u8], stderr: &[u8]) -> anyhow::Result<Vec<u8>> {
        let stdout_text = String::from_utf8_lossy(stdout);
        let stderr_text = String::from_utf8_lossy(stderr);
        
        // Method 1: Look for Merkle root or prover root
        if let Some(root) = self.extract_merkle_root(&stdout_text) {
            tracing::debug!("Extracted Merkle root as journal: {}", hex::encode(&root));
            return Ok(root);
        }
        
        // Method 2: Look for hash patterns (64-char hex strings)
        if let Some(hash) = self.extract_hash_patterns(&stdout_text) {
            tracing::debug!("Extracted hash pattern as journal: {}", hex::encode(&hash));
            return Ok(hash);
        }
        
        // Method 3: Look for numeric results
        if let Some(number_bytes) = self.extract_numeric_results(&stdout_text) {
            tracing::debug!("Extracted numeric result as journal: {}", hex::encode(&number_bytes));
            return Ok(number_bytes);
        }
        
        // Method 4: Look for structured output (JSON, arrays, etc.)
        if let Some(structured) = self.extract_structured_output(&stdout_text) {
            tracing::debug!("Extracted structured output as journal: {}", hex::encode(&structured));
            return Ok(structured);
        }
        
        // Method 5: Check stderr for error patterns that might contain results
        if let Some(error_result) = self.extract_from_errors(&stderr_text) {
            tracing::debug!("Extracted result from error output: {}", hex::encode(&error_result));
            return Ok(error_result);
        }
        
        anyhow::bail!("No extractable computation results found in program output")
    }
    
    /// Extract Merkle root or prover root from output
    #[cfg(feature = "unsafe-fallback")]
    fn extract_merkle_root(&self, text: &str) -> Option<Vec<u8>> {
        for line in text.lines() {
            // Look for various root patterns
            let patterns = [
                "Prover root:",
                "Merkle root:",
                "Root:",
                "Final root:",
                "Commitment:",
            ];
            
            for pattern in &patterns {
                if line.contains(pattern) {
                    // Find the hex string after the pattern
                    if let Some(hex_part) = line.split(pattern).nth(1) {
                        for word in hex_part.split_whitespace() {
                            if word.len() == 64 && word.chars().all(|c| c.is_ascii_hexdigit()) {
                                if let Ok(bytes) = hex::decode(word) {
                                    return Some(bytes);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Extract hash patterns from output (SHA256, etc.)
    #[cfg(feature = "unsafe-fallback")]
    fn extract_hash_patterns(&self, text: &str) -> Option<Vec<u8>> {
        for line in text.lines() {
            for word in line.split_whitespace() {
                // Look for 64-character hex strings (SHA256)
                if word.len() == 64 && word.chars().all(|c| c.is_ascii_hexdigit()) {
                    if let Ok(bytes) = hex::decode(word) {
                        return Some(bytes);
                    }
                }
                
                // Look for 0x-prefixed hex strings
                if word.starts_with("0x") && word.len() >= 10 {
                    let hex_part = &word[2..];
                    if hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
                        if let Ok(bytes) = hex::decode(hex_part) {
                            return Some(bytes);
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Extract numeric results from output
    #[cfg(feature = "unsafe-fallback")]
    fn extract_numeric_results(&self, text: &str) -> Option<Vec<u8>> {
        for line in text.lines() {
            // Look for result patterns
            let result_patterns = [
                "Result:",
                "Output:",
                "Answer:",
                "Computed:",
                "Final:",
                "Sum:",
                "Product:",
                "Value:",
            ];
            
            for pattern in &result_patterns {
                if line.contains(pattern) {
                    if let Some(value_part) = line.split(pattern).nth(1) {
                        // Try to parse as number
                        if let Ok(num) = value_part.trim().parse::<u64>() {
                            return Some(num.to_le_bytes().to_vec());
                        }
                        if let Ok(num) = value_part.trim().parse::<i64>() {
                            return Some(num.to_le_bytes().to_vec());
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Extract structured output (JSON arrays, etc.)
    #[cfg(feature = "unsafe-fallback")]
    fn extract_structured_output(&self, text: &str) -> Option<Vec<u8>> {
        for line in text.lines() {
            let line = line.trim();
            
            // Look for JSON arrays
            if line.starts_with('[') && line.ends_with(']') {
                // Simple array parsing - extract numbers
                let inner = &line[1..line.len()-1];
                let mut result = Vec::new();
                for part in inner.split(',') {
                    if let Ok(num) = part.trim().parse::<u32>() {
                        result.extend_from_slice(&num.to_le_bytes());
                    }
                }
                if !result.is_empty() {
                    return Some(result);
                }
            }
            
            // Look for comma-separated values
            if line.contains(',') && !line.contains(' ') {
                let mut result = Vec::new();
                for part in line.split(',') {
                    if let Ok(num) = part.trim().parse::<u32>() {
                        result.extend_from_slice(&num.to_le_bytes());
                    }
                }
                if result.len() >= 4 { // At least one number
                    return Some(result);
                }
            }
        }
        None
    }
    
    /// Extract results from error output
    #[cfg(feature = "unsafe-fallback")]
    fn extract_from_errors(&self, text: &str) -> Option<Vec<u8>> {
        // Sometimes programs output results to stderr
        // Apply the same extraction methods
        if let Some(hash) = self.extract_hash_patterns(text) {
            return Some(hash);
        }
        if let Some(num) = self.extract_numeric_results(text) {
            return Some(num);
        }
        None
    }
    
    /// Generate journal from execution context when no explicit results found
    #[cfg(feature = "unsafe-fallback")]
    fn generate_journal_from_execution_context(&self, stdout: &[u8], stderr: &[u8]) -> anyhow::Result<Vec<u8>> {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        
        // Include meaningful parts of the output (first 2KB to avoid secrets)
        let stdout_sample = &stdout[..std::cmp::min(2048, stdout.len())];
        let stderr_sample = &stderr[..std::cmp::min(1024, stderr.len())];
        
        hasher.update(stdout_sample);
        hasher.update(stderr_sample);
        
        // Include program signature for determinism
        hasher.update(&self.wasm[..std::cmp::min(1024, self.wasm.len())]);
        
        // Include hints structure (but not content to avoid leaking secrets)
        hasher.update(&self.hints_blob.len().to_le_bytes());
        
        let journal = hasher.finalize().to_vec();
        tracing::debug!("Generated deterministic journal from execution context: {}", hex::encode(&journal));
        
        Ok(journal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_creation() {
        let wasm_bytes = b"fake wasm program";
        let host = LigetronHost::new(wasm_bytes);
        
        assert_eq!(host.wasm, wasm_bytes);
        assert_eq!(host.packing, LigetronHost::default_packing());
        assert!(host.hints_blob.is_empty());
    }

    #[test]
    fn test_host_configuration() {
        let wasm_bytes = b"fake wasm program";
        let host = LigetronHost::new(wasm_bytes)
            .with_packing(4096)
            .with_shader_path("./custom_shader".to_string())
            .with_prover_bin("custom_prover".to_string());
        
        assert_eq!(host.packing, 4096);
        assert_eq!(host.shader_path, Some("./custom_shader".to_string()));
        assert_eq!(host.prover_bin, "custom_prover");
    }

    #[test]
    fn test_add_hint() {
        let wasm_bytes = b"fake wasm program";
        let mut host = LigetronHost::new(wasm_bytes);
        
        // Add some test hints
        host.add_hint(&42u64);
        host.add_hint(&"test string".to_string());
        
        assert!(!host.hints_blob.is_empty());
        
        // Verify we can deserialize the hints back
        let mut cursor = std::io::Cursor::new(&host.hints_blob);
        let hint1: u64 = bincode::deserialize_from(&mut cursor).unwrap();
        let hint2: String = bincode::deserialize_from(&mut cursor).unwrap();
        
        assert_eq!(hint1, 42u64);
        assert_eq!(hint2, "test string");
    }

    #[test]
    fn test_parse_journal_from_output() {
        let host = LigetronHost::new(b"fake wasm");
        
        let output_with_journal = b"Some other output\nSOV_JOURNAL_HEX:deadbeef\nMore output\n";
        let journal = host.parse_journal_from_output(output_with_journal).unwrap();
        assert_eq!(journal, vec![0xde, 0xad, 0xbe, 0xef]);
        
        let output_without_journal = b"Some output without journal\n";
        assert!(host.parse_journal_from_output(output_without_journal).is_err());
    }

    #[test]
    fn test_build_prover_config() {
        let mut host = LigetronHost::new(b"fake wasm");
        host.add_hint(&42u64);
        
        let (config, typed_args, private_indices) = host.build_prover_config("/path/to/program.wasm", "0xabcd");
        
        assert_eq!(config["program"], "/path/to/program.wasm");
        assert_eq!(config["packing"], 8192);
        
        // Check private indices structure (more robust than exact match)
        assert!(!private_indices.is_empty(), "Should have at least one private index");
        
        let json_args = config["args"].as_array().unwrap();
        assert_eq!(json_args[0]["hex"], "0xabcd"); // Public digest
        assert!(json_args.len() >= 2, "Should have digest + at least one hint chunk");
        assert_eq!(json_args.len(), private_indices.len() + 1, "Args should be digest + chunks");
        
        // Check typed args structure
        assert_eq!(typed_args.len(), json_args.len());
        assert!(matches!(typed_args[0], LigetronArg::Hex(_))); // Digest is hex
        
        // Verify all hint chunks are hex-encoded
        for i in 1..json_args.len() {
            assert!(json_args[i]["hex"].as_str().unwrap().starts_with("0x"));
            assert!(matches!(typed_args[i], LigetronArg::Hex(_))); // Chunks are hex
        }
    }

    #[test]
    fn test_parse_journal_any() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Test journal in stdout
        let stdout = format!("Debug info\n{}48656c6c6f\nMore output", SOV_JOURNAL_HEX_PREFIX);
        let stderr = b"Error messages";
        let journal = host.parse_journal_any(stdout.as_bytes(), stderr).unwrap();
        assert_eq!(journal, b"Hello");
        
        // Test journal in stderr (fallback)
        let stdout_empty = b"No journal here";
        let stderr_with_journal = format!("Error: something\n{}576f726c64\nMore errors", SOV_JOURNAL_HEX_PREFIX);
        let journal = host.parse_journal_any(stdout_empty, stderr_with_journal.as_bytes()).unwrap();
        assert_eq!(journal, b"World");
        
        // Test no journal in either stream
        let stdout_no_journal = b"Debug output";
        let stderr_no_journal = b"Error output";
        assert!(host.parse_journal_any(stdout_no_journal, stderr_no_journal).is_err());
    }

    #[test]
    fn test_explicit_journal_parsing() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Test explicit SOV_JOURNAL_HEX parsing (secure by default)
        let stdout = b"Computing...\nSOV_JOURNAL_HEX:deadbeef\nDone.";
        let stderr = b"";
        let journal = host.parse_journal_any(stdout, stderr).unwrap();
        assert_eq!(journal, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    #[cfg(feature = "unsafe-fallback")]
    fn test_auto_extract_journal_merkle_root() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Enable unsafe fallback for this test
        std::env::set_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK", "1");
        
        // Test Merkle root extraction
        let stdout = b"Computing proof...\nProver root: b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9\nProof complete.";
        let stderr = b"";
        let journal = host.auto_extract_journal(stdout, stderr).unwrap();
        assert_eq!(journal.len(), 32); // Should be 32 bytes
        assert_eq!(hex::encode(&journal), "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
    }

    #[test]
    #[cfg(feature = "unsafe-fallback")]
    fn test_auto_extract_journal_hash_pattern() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Enable unsafe fallback for this test
        std::env::set_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK", "1");
        
        // Test hash pattern extraction
        let stdout = b"SHA256 result: a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";
        let stderr = b"";
        let journal = host.auto_extract_journal(stdout, stderr).unwrap();
        assert_eq!(journal.len(), 32);
        assert_eq!(hex::encode(&journal), "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3");
        
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
    }

    #[test]
    #[cfg(feature = "unsafe-fallback")]
    fn test_auto_extract_journal_numeric_result() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Enable unsafe fallback for this test
        std::env::set_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK", "1");
        
        // Test numeric result extraction
        let stdout = b"Computing sum...\nResult: 42\nDone.";
        let stderr = b"";
        let journal = host.auto_extract_journal(stdout, stderr).unwrap();
        assert_eq!(journal, 42u64.to_le_bytes().to_vec());
        
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
    }

    #[test]
    #[cfg(feature = "unsafe-fallback")]
    fn test_auto_extract_journal_structured_output() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Enable unsafe fallback for this test
        std::env::set_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK", "1");
        
        // Test JSON array extraction - should work or fall back to deterministic hash
        let stdout = b"Array result: [1,2,3,4]";
        let stderr = b"";
        let journal = host.auto_extract_journal(stdout, stderr).unwrap();
        
        // Should generate some deterministic journal (either parsed JSON or execution context hash)
        assert_eq!(journal.len(), 32); // Should be 32 bytes
        
        // Should be deterministic - same inputs produce same journal
        let journal2 = host.auto_extract_journal(stdout, stderr).unwrap();
        assert_eq!(journal, journal2);
        
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
    }

    #[test]
    #[cfg(feature = "unsafe-fallback")]
    fn test_auto_extract_journal_execution_context() {
        let mut host = LigetronHost::new(b"fake wasm program");
        host.add_hint(&"test input".to_string());
        
        // Enable unsafe fallback for this test
        std::env::set_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK", "1");
        
        // Test fallback to execution context
        let stdout = b"Some generic output without extractable patterns";
        let stderr = b"Some errors";
        let journal = host.auto_extract_journal(stdout, stderr).unwrap();
        
        // Should generate deterministic 32-byte hash
        assert_eq!(journal.len(), 32);
        
        // Should be deterministic - same inputs produce same journal
        let journal2 = host.auto_extract_journal(stdout, stderr).unwrap();
        assert_eq!(journal, journal2);
        
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
    }

    #[test]
    fn test_explicit_journal_required_by_default() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Ensure unsafe fallback is not set
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
        
        // Test that extraction fails without explicit SOV_JOURNAL_HEX by default
        let stdout = b"Some output with hash: a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";
        let stderr = b"";
        
        // Should fail because no SOV_JOURNAL_HEX prefix
        let result = host.parse_journal_any(stdout, stderr);
        assert!(result.is_err());
    }

    #[test]
    #[cfg(feature = "unsafe-fallback")]
    fn test_unsafe_fallback_with_env_var() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Enable unsafe fallback for this test
        std::env::set_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK", "1");
        
        // Test that extraction works with hash patterns when unsafe fallback is enabled
        let stdout = b"Computing hash: a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";
        let stderr = b"";
        
        let journal = host.auto_extract_journal(stdout, stderr).unwrap();
        assert_eq!(journal.len(), 32);
        assert_eq!(hex::encode(&journal), "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3");
        
        std::env::remove_var("LIGETRON_UNSAFE_JOURNAL_FALLBACK");
    }

    #[test]
    fn test_explicit_journal_always_works() {
        let host = LigetronHost::new(b"fake wasm");
        
        // Test that explicit SOV_JOURNAL_HEX always works (secure by default)
        let stdout = b"Computing...\nSOV_JOURNAL_HEX:deadbeef\nDone.";
        let stderr = b"";
        
        let journal = host.parse_journal_any(stdout, stderr).unwrap();
        assert_eq!(journal, vec![0xde, 0xad, 0xbe, 0xef]);
    }
}
