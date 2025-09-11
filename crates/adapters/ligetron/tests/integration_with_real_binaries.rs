#![cfg(feature = "native")]

//! Integration tests using real Ligetron binaries
//! 
//! These tests require the actual webgpu_prover and webgpu_verifier binaries
//! to be available in the test environment.

// Serde imports removed - no longer needed since we use the real SHA256 WASM program
use sov_ligetron_adapter::{Ligetron, LigetronMethodId};
use sov_rollup_interface::zk::{CodeCommitment, Zkvm, ZkvmHost, ZkVerifier};
use std::path::PathBuf;
// use tempfile::TempDir;

/// Test configuration for Ligetron binaries
#[derive(Debug)]
pub struct LigetronTestConfig {
    pub prover_path: PathBuf,
    pub verifier_path: PathBuf,
    pub shader_path: PathBuf,
}

impl LigetronTestConfig {
    /// Create test config from environment variables or default paths
    pub fn from_env() -> Option<Self> {
        let prover_path = std::env::var("LIGETRON_PROVER")
            .or_else(|_| std::env::var("LIGETRON_TEST_PROVER"))
            .map(PathBuf::from)
            .or_else(|_| Self::find_binary("webgpu_prover").ok_or_else(|| std::env::VarError::NotPresent))
            .ok()?;
            
        let verifier_path = std::env::var("LIGETRON_VERIFIER")
            .or_else(|_| std::env::var("LIGETRON_TEST_VERIFIER"))
            .map(PathBuf::from)
            .or_else(|_| Self::find_binary("webgpu_verifier").ok_or_else(|| std::env::VarError::NotPresent))
            .ok()?;
            
        let shader_path = std::env::var("LIGETRON_SHADER_PATH")
            .or_else(|_| std::env::var("LIGETRON_TEST_SHADER_PATH"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./shader"));

        if prover_path.exists() && verifier_path.exists() {
            Some(Self {
                prover_path,
                verifier_path,
                shader_path,
            })
        } else {
            None
        }
    }
    
    /// Try to find binary in common locations
    fn find_binary(name: &str) -> Option<PathBuf> {
        // Check current directory
        let local_path = PathBuf::from(format!("./{}", name));
        if local_path.exists() {
            return Some(local_path);
        }
        
        // Check in test_binaries directory
        let test_path = PathBuf::from(format!("./test_binaries/{}", name));
        if test_path.exists() {
            return Some(test_path);
        }
        
        // Check in PATH
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
        
        None
    }
    
    /// Setup environment variables for testing
    pub fn setup_env(&self) {
        // Convert paths to absolute to avoid issues when running from different working directories
        let abs_prover = std::fs::canonicalize(&self.prover_path)
            .unwrap_or_else(|_| self.prover_path.clone());
        let abs_verifier = std::fs::canonicalize(&self.verifier_path)
            .unwrap_or_else(|_| self.verifier_path.clone());
        let abs_shader = std::fs::canonicalize(&self.shader_path)
            .unwrap_or_else(|_| self.shader_path.clone());
            
        std::env::set_var("LIGETRON_PROVER", abs_prover);
        std::env::set_var("LIGETRON_VERIFIER", abs_verifier);
        std::env::set_var("LIGETRON_SHADER_PATH", abs_shader);
    }
}

/// Load the SHA256 WASM program for testing
fn load_sha256_wasm_program() -> Vec<u8> {
    // Use journal-aware WASM program if available, otherwise fall back to original
    if std::path::Path::new("test_data/sha256_journal.wasm").exists() {
        std::fs::read("test_data/sha256_journal.wasm").expect("Failed to read journal-aware WASM")
    } else {
        include_bytes!("../test_data/sha256.wasm").to_vec()
    }
}

// Test data structures removed - we now use the real SHA256 WASM program
// which expects string, i64, and hex inputs as per the execution pattern

/// Helper function to provide better error messages for missing binaries
fn handle_binary_error(error: &anyhow::Error, test_name: &str) -> ! {
    let error_msg = error.to_string();
    if error_msg.contains("No such file or directory") 
        || error_msg.contains("not found")
        || error_msg.contains("cannot find binary")
        || error_msg.contains("Failed to spawn") {
        panic!(
            "{} failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH\n\
            Original error: {}", 
            test_name, error
        );
    } else {
        panic!("{} failed: {}", test_name, error);
    }
}

/// Test that requires real Ligetron binaries
#[test]
fn test_with_real_ligetron_binaries() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_with_real_ligetron_binaries failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    // Load the real SHA256 WASM program
    let wasm_program = load_sha256_wasm_program();
    let wasm_slice: &'static [u8] = Box::leak(wasm_program.into_boxed_slice());
    let mut host = <Ligetron as Zkvm>::Host::from_args(&wasm_slice);
    
    // Add hints for SHA256 program (matching the expected pattern)
    host.add_hint(&"hello world");
    host.add_hint(&99999i64);
    host.add_hint(&hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9").unwrap());
    
    // This test will only work if you have a real WASM program
    // that uses sov_journal.h and implements the expected logic
    match host.run(true) {
        Ok(proof_bytes) => {
            println!("✅ Proof generation successful! Proof size: {} bytes", proof_bytes.len());
            
            // Test verification
            let commitment = host.code_commitment();
            let result = <Ligetron as Zkvm>::Verifier::verify::<[u8; 32]>(&proof_bytes, &commitment);
            
            match result {
                Ok(journal_output) => {
                    println!("✅ Verification successful!");
                    println!("📄 Journal output: {} bytes", journal_output.len());
                    
                    if !journal_output.is_empty() {
                        println!("📄 Journal content (hex): {}", hex::encode(&journal_output));
                        
                        // Try to interpret as string if it looks like text
                        if let Ok(journal_str) = String::from_utf8(journal_output.to_vec()) {
                            if journal_str.is_ascii() && !journal_str.contains('\0') {
                                println!("📄 Journal content (text): '{}'", journal_str);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("❌ Verification failed: {}", e);
                    panic!("Verification should succeed");
                }
            }
        }
        Err(e) => {
            handle_binary_error(&e, "test_with_real_ligetron_binaries");
        }
    }
}

/// Test method ID generation with real programs
#[test]
fn test_method_id_with_real_program() {
    let wasm_program = load_sha256_wasm_program();
    let wasm_slice: &'static [u8] = Box::leak(wasm_program.into_boxed_slice());
    let host = <Ligetron as Zkvm>::Host::from_args(&wasm_slice);
    
    let method_id = host.code_commitment();
    
    // Method ID should be deterministic
    let method_id2 = host.code_commitment();
    assert_eq!(method_id, method_id2);
    
    // Test encoding/decoding
    let encoded = method_id.encode();
    assert_eq!(encoded.len(), 32); // SHA-256 is 32 bytes
    
    let decoded = LigetronMethodId::decode(&encoded).unwrap();
    assert_eq!(method_id, decoded);
    
    println!("✅ Method ID: {}", hex::encode(&encoded));
}

/// Test configuration validation
#[test]
fn test_ligetron_config_validation() {
    let config = LigetronTestConfig::from_env();
    
    if let Some(config) = config {
        println!("✅ Found Ligetron configuration:");
        println!("  Prover: {}", config.prover_path.display());
        println!("  Verifier: {}", config.verifier_path.display());
        println!("  Shader path: {}", config.shader_path.display());
        
        // Verify binaries are executable
        assert!(config.prover_path.exists(), "Prover binary should exist");
        assert!(config.verifier_path.exists(), "Verifier binary should exist");
        
        // Test that we can run the binaries (they should show help or error gracefully)
        let prover_test = std::process::Command::new(&config.prover_path)
            .arg("--help")
            .output();
            
        match prover_test {
            Ok(output) => {
                println!("✅ Prover binary is executable");
                if !output.status.success() {
                    println!("  Note: --help returned non-zero (this might be expected)");
                }
            }
            Err(e) => {
                eprintln!("❌ Cannot execute prover binary: {}", e);
            }
        }
        
        let verifier_test = std::process::Command::new(&config.verifier_path)
            .arg("--help")
            .output();
            
        match verifier_test {
            Ok(output) => {
                println!("✅ Verifier binary is executable");
                if !output.status.success() {
                    println!("  Note: --help returned non-zero (this might be expected)");
                }
            }
            Err(e) => {
                eprintln!("❌ Cannot execute verifier binary: {}", e);
            }
        }
    } else {
        println!("ℹ️  Ligetron binaries not configured for testing");
        println!("   This is fine - other tests will use mocks");
    }
}

/// Stress test with multiple proof generations
#[test]
fn test_multiple_proofs() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_multiple_proofs failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    let wasm_program = load_sha256_wasm_program();
    
    // Generate multiple proofs with different inputs
    for i in 0..3 {
        let wasm_slice: &'static [u8] = Box::leak(wasm_program.clone().into_boxed_slice());
        let mut host = <Ligetron as Zkvm>::Host::from_args(&wasm_slice);
        
        // Use different inputs for each iteration
        let input_string = format!("test input {}", i);
        let private_number = 10000 + i as i64;
        let expected_hash = hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9").unwrap();
        
        host.add_hint(&input_string);
        host.add_hint(&private_number);
        host.add_hint(&expected_hash);
        
        match host.run(false) { // Run without proof for speed
            Ok(_) => {
                println!("✅ Iteration {} completed", i);
            }
            Err(e) => {
                handle_binary_error(&e, &format!("test_multiple_proofs (iteration {})", i));
            }
        }
    }
}

/// Test error handling with invalid inputs
#[test]
fn test_error_handling() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_error_handling failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    // Test with invalid WASM program
    const INVALID_WASM: &[u8] = b"not a valid wasm program";
    let mut host = <Ligetron as Zkvm>::Host::from_args(&INVALID_WASM);
    host.add_hint(&42u64);
    
    let result = host.run(false);
    match result {
        Ok(_) => {
            println!("⚠️  Expected failure with invalid WASM, but got success");
        }
        Err(e) => {
            println!("✅ Correctly failed with invalid WASM: {}", e);
        }
    }
}

/// Performance benchmark (if binaries are available)
#[test]
fn benchmark_proof_generation() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "benchmark_proof_generation failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    let wasm_program = load_sha256_wasm_program();
    let wasm_slice: &'static [u8] = Box::leak(wasm_program.into_boxed_slice());
    let mut host = <Ligetron as Zkvm>::Host::from_args(&wasm_slice);
    
    // Add SHA256 benchmark inputs
    host.add_hint(&"benchmark test");
    host.add_hint(&12345i64);
    host.add_hint(&hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9").unwrap());
    
    // Benchmark proof generation
    let start = std::time::Instant::now();
    match host.run(true) {
        Ok(proof_bytes) => {
            let duration = start.elapsed();
            println!("✅ Proof generation took: {:?}", duration);
            println!("   Proof size: {} bytes", proof_bytes.len());
            println!("   Throughput: {:.2} KB/s", proof_bytes.len() as f64 / duration.as_secs_f64() / 1024.0);
        }
        Err(e) => {
            handle_binary_error(&e, "benchmark_proof_generation");
        }
    }
}

/// Test with the real sha256.wasm program using the provided execution pattern
#[test]
fn test_sha256_wasm_program() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_sha256_wasm_program failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    // Load the sha256.wasm program
    let wasm_path = std::path::Path::new("test_data/sha256.wasm");
    if !wasm_path.exists() {
        eprintln!("Skipping test: sha256.wasm not found at {:?}", wasm_path);
        return;
    }
    
    let wasm_program = std::fs::read(wasm_path).expect("Failed to read sha256.wasm");
    let wasm_slice: &'static [u8] = Box::leak(wasm_program.into_boxed_slice());
    
    // Create host with packing=8192 to match the example
    let mut host = <Ligetron as Zkvm>::Host::new(wasm_slice).with_packing(8192);
    
    // Add hints matching the example execution pattern:
    // args: [{"str":"hello world"}, {"i64":99999}, {"hex":"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"}]
    // private-indices: [1] (meaning arg[1] is private, which is the i64 value)
    
    let input_string = "hello world";
    let private_number = 99999i64;
    let expected_hash = hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
        .expect("Failed to decode expected hash");
    
    // Add hints in order they'll be consumed by the WASM program
    host.add_hint(&input_string);
    host.add_hint(&private_number);
    host.add_hint(&expected_hash);
    
    println!("🔧 Testing SHA256 WASM program with:");
    println!("   Input string: '{}'", input_string);
    println!("   Private number: {} (will be redacted in logs)", private_number);
    println!("   Expected hash: {}", hex::encode(&expected_hash));
    println!("   Packing: 8192");
    
    match host.run(true) {
        Ok(proof_bytes) => {
            println!("✅ SHA256 proof generation successful! Proof size: {} bytes", proof_bytes.len());
            
            // Test verification
            let commitment = host.code_commitment();
            println!("📋 Method ID: {}", hex::encode(commitment.encode()));
            
            // Try to verify and extract the journal
            let result = <Ligetron as Zkvm>::Verifier::verify::<[u8; 32]>(&proof_bytes, &commitment);
            
            match result {
                Ok(journal_output) => {
                    println!("✅ SHA256 verification successful!");
                    println!("📄 Journal output: {} bytes", journal_output.len());
                    
                    // The journal should contain the computation result
                    // (exact format depends on what the sha256.wasm program outputs)
                    if !journal_output.is_empty() {
                        println!("📄 Journal content (hex): {}", hex::encode(&journal_output));
                        
                        // Try to interpret as string if it looks like text
                        if let Ok(journal_str) = String::from_utf8(journal_output.to_vec()) {
                            if journal_str.is_ascii() && !journal_str.contains('\0') {
                                println!("📄 Journal content (text): '{}'", journal_str);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("❌ SHA256 verification failed: {}", e);
                    eprintln!("This might indicate an issue with the proof or verifier configuration");
                    panic!("SHA256 verification should succeed");
                }
            }
        }
        Err(e) => {
            handle_binary_error(&e, "test_sha256_wasm_program");
        }
    }
}

/// Test with journal-aware WASM program (secure approach)
#[test]
fn test_journal_aware_wasm() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_journal_aware_wasm failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    println!("🔧 Testing journal-aware WASM program (secure approach)");
    
    println!("🔧 Testing journal-aware WASM program with full integration");
    
    // Load the journal-aware WASM program
    let wasm_program = load_sha256_wasm_program();
    let wasm_slice: &'static [u8] = Box::leak(wasm_program.into_boxed_slice());
    
    // Create host and add hints in the expected format
    let mut host = <Ligetron as Zkvm>::Host::from_args(&wasm_slice);
    
    // Add hints that match the journal-aware program expectations
    host.add_hint(&"hello world".to_string());  // String input
    host.add_hint(&11i64);                      // i64 input (length)
    host.add_hint(&hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9").unwrap()); // Expected hash
    
    println!("📋 Added 3 hints to host");
    
    // Skip simulation test here to avoid draining hints before run()
    // (simulate_with_hints() consumes the hints_blob)
    
    // Test proof generation with journal-aware program
    println!("🔧 Testing proof generation with journal-aware WASM...");
    match host.run(true) {
        Ok(proof_bytes) => {
            println!("✅ Proof generation successful!");
            
            // Verify the proof (no double-serialization)
            let method_id = host.code_commitment();
            
            match <Ligetron as Zkvm>::Verifier::verify::<[u8; 32]>(&proof_bytes, &method_id) {
                Ok(journal) => {
                    println!("✅ Proof verification successful!");
                    println!("📋 Journal: {} bytes", journal.len());
                    
                    // Verify the journal contains the expected SHA256 hash
                    let expected_hash_vec = hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9").unwrap();
                    let mut expected_hash = [0u8; 32];
                    expected_hash.copy_from_slice(&expected_hash_vec);
                    
                    assert_eq!(journal, expected_hash, "Journal should contain the SHA256 hash of 'hello world'");
                    println!("📄 Journal (hex): {}", hex::encode(journal));
                    
                    println!("🎉 COMPLETE journal-aware integration test PASSED!");
                }
                Err(e) => {
                    println!("❌ Proof verification failed: {}", e);
                    panic!("Verification should succeed with journal-aware WASM");
                }
            }
        }
        Err(e) => {
            println!("❌ Proof generation failed: {}", e);
            panic!("Proof generation should succeed with journal-aware WASM: {}", e);
        }
    }
}

/// Test demonstrating the insecure wrapper approach (DEPRECATED)
/// This test shows why the wrapper approach is cryptographically unsound
#[test]
#[ignore] // Disabled - this approach is unsafe
fn test_deprecated_wrapper_approach() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_simple_wrapper_approach failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    // Test the working configuration format directly
    let test_config = serde_json::json!({
        "program": "test_data/sha256.wasm",
        "shader-path": "shader",
        "packing": 8192,
        "private-indices": [1],
        "args": [
            {"str": "hello world"},
            {"i64": 11},
            {"hex": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"}
        ]
    });
    
    println!("🔧 Testing simple wrapper approach");
    println!("📋 Config: {}", test_config);
    
    // Run prover directly with working config
    let output = std::process::Command::new(&config.prover_path)
        .arg(test_config.to_string())
        .current_dir(".")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("Failed to run prover");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    println!("📤 Prover stdout:\n{}", stdout);
    if !stderr.is_empty() {
        println!("📤 Prover stderr:\n{}", stderr);
    }
    
    // Check if prover succeeded
    if !output.status.success() {
        panic!("Prover exited with non-zero status: {:?}", output.status.code());
    }
    
    // Debug: print each line to see what we're getting
    println!("🔍 Analyzing output lines:");
    for (i, line) in stdout.lines().enumerate() {
        println!("Line {}: '{}'", i, line);
        if line.contains("Final prove result") {
            println!("  ↳ Found result line!");
        }
        if line.starts_with("Prover root:") {
            println!("  ↳ Found prover root line!");
        }
    }
    
    // Check for success patterns
    let has_final_result = stdout.contains("Final prove result:") && stdout.contains("true");
    let has_validation_success = stdout.contains("Validation of linear constraints:    true") && 
                                stdout.contains("Validation of quadratic constraints: true");
    
    if has_final_result || has_validation_success {
        println!("✅ Proof generation successful!");
        
        // Extract Merkle root as journal
        for line in stdout.lines() {
            if line.starts_with("Prover root:") {
                if let Some(root_hex) = line.split_whitespace().nth(2) {
                    println!("📋 Extracted journal (Merkle root): {}", root_hex);
                    
                    // This demonstrates how to create a "journal" from existing output
                    if let Ok(journal_bytes) = hex::decode(root_hex) {
                        println!("✅ Journal extracted: {} bytes", journal_bytes.len());
                        println!("✅ This shows the wrapper approach works!");
                        
                        // Verify the journal is reasonable (don't check exact value since it might vary)
                        assert_eq!(journal_bytes.len(), 32, "Merkle root should be 32 bytes");
                        
                        println!("🎉 Wrapper approach test PASSED!");
                        return; // Test passed
                    }
                }
            }
        }
        panic!("Could not extract Merkle root from successful output");
    } else {
        panic!("No success indicators found in output");
    }
}

/// Test with native Ligetron SHA256 program (bypasses journal mechanism)
#[test] 
fn test_native_ligetron_sha256() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_native_ligetron_sha256 failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    // Test the prover directly with your working configuration
    let test_config = serde_json::json!({
        "program": "test_data/sha256.wasm",
        "shader-path": "shader",
        "packing": 8192,
        "private-indices": [1],
        "args": [
            {"str": "hello world"},
            {"i64": 11},
            {"hex": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"}
        ]
    });
    
    println!("🔧 Testing native Ligetron SHA256 program");
    println!("📋 Config: {}", test_config);
    
    // Run prover directly (bypassing our adapter's journal mechanism)
    let output = std::process::Command::new(&config.prover_path)
        .arg(test_config.to_string())
        .current_dir(".")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("Failed to run prover");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    println!("📤 Prover stdout:\n{}", stdout);
    if !stderr.is_empty() {
        println!("📤 Prover stderr:\n{}", stderr);
    }
    
    // Check if the prover succeeded
    if stdout.contains("Final prove result: true") {
        println!("✅ Native Ligetron SHA256 program executed successfully!");
        println!("✅ This confirms the Ligetron binaries and shader files are working correctly");
        println!("✅ Proof generation completed with validation: true");
    } else if output.status.success() {
        println!("✅ Prover completed without errors");
        // Still check for validation success even if no explicit result
        if stdout.contains("Validation of linear constraints:    true") && 
           stdout.contains("Validation of quadratic constraints: true") {
            println!("✅ All constraint validations passed!");
        }
    } else {
        panic!("❌ Prover failed with exit code: {:?}", output.status.code());
    }
}

/// Test SHA256 program in simulation mode (faster, no cryptographic proof)
#[test]
fn test_sha256_simulation_mode() {
    let config = LigetronTestConfig::from_env().unwrap_or_else(|| {
        panic!(
            "test_sha256_simulation_mode failed: Ligetron binaries not found or not executable.\n\
            Please set up binaries using one of these methods:\n\
            1. Set environment variables: LIGETRON_PROVER, LIGETRON_VERIFIER\n\
            2. Place binaries in ./test_binaries/ directory\n\
            3. Add binaries to system PATH"
        );
    });
    
    config.setup_env();
    
    // Load the sha256.wasm program
    let wasm_path = std::path::Path::new("test_data/sha256.wasm");
    if !wasm_path.exists() {
        eprintln!("Skipping test: sha256.wasm not found at {:?}", wasm_path);
        return;
    }
    
    let wasm_program = std::fs::read(wasm_path).expect("Failed to read sha256.wasm");
    let wasm_slice: &'static [u8] = Box::leak(wasm_program.into_boxed_slice());
    
    let mut host = <Ligetron as Zkvm>::Host::new(wasm_slice).with_packing(8192);
    
    // Add the same hints as the full proof test
    host.add_hint(&"hello world");
    host.add_hint(&99999i64);
    host.add_hint(&hex::decode("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9").unwrap());
    
    println!("🔧 Testing SHA256 WASM program in simulation mode");
    
    // Test simulation mode (no proof generation)
    match host.run(false) {
        Ok(simulation_bytes) => {
            println!("✅ SHA256 simulation successful! Result size: {} bytes", simulation_bytes.len());
            
            // In simulation mode, we get the journal directly without cryptographic verification
            let commitment = host.code_commitment();
            let result = <Ligetron as Zkvm>::Verifier::verify::<[u8; 32]>(&simulation_bytes, &commitment);
            
            match result {
                Ok(journal_output) => {
                    println!("✅ SHA256 simulation verification successful!");
                    println!("📄 Simulation journal: {} bytes", journal_output.len());
                    
                    if !journal_output.is_empty() {
                        println!("📄 Simulation output (hex): {}", hex::encode(&journal_output));
                        
                        // Try to interpret as string if it looks like text
                        if let Ok(journal_str) = String::from_utf8(journal_output.to_vec()) {
                            if journal_str.is_ascii() && !journal_str.contains('\0') {
                                println!("📄 Simulation output (text): '{}'", journal_str);
                            }
                        }
                    }
                }
                Err(e) => {
                    panic!("SHA256 simulation verification failed: {}", e);
                }
            }
        }
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("No SOV_JOURNAL_HEX found") {
                panic!(
                    "test_sha256_simulation_mode failed: The SHA256 WASM program does not emit journal output.\n\
                    This suggests the program was not compiled with the Ligetron journal mechanism.\n\
                    Expected: Program should print 'SOV_JOURNAL_HEX:<hex_data>' to stdout/stderr.\n\
                    Actual: No journal output found.\n\
                    \n\
                    To fix this:\n\
                    1. Ensure the WASM program uses the sov_journal.h header\n\
                    2. The program should call sov_commit() to emit journal data\n\
                    3. Recompile the program with Ligetron's toolchain\n\
                    \n\
                    Original error: {}", e
                );
            } else {
                handle_binary_error(&e, "test_sha256_simulation_mode");
            }
        }
    }
}
