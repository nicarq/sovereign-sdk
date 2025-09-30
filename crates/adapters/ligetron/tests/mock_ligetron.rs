//! Mock Ligetron prover and verifier for testing
//! 
//! This module provides mock implementations that simulate the behavior of
//! webgpu_prover and webgpu_verifier without requiring the actual binaries.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
// use std::process::Command;
use tempfile::TempDir;

/// Mock Ligetron environment for testing
pub struct MockLigetronEnv {
    temp_dir: TempDir,
    mock_prover_path: PathBuf,
    mock_verifier_path: PathBuf,
    shader_path: PathBuf,
}

impl MockLigetronEnv {
    /// Create a new mock Ligetron environment
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path();
        
        // Create mock binary paths with proper extensions for Windows
        let mock_prover_path = if cfg!(windows) {
            base_path.join("mock_webgpu_prover.cmd")
        } else {
            base_path.join("mock_webgpu_prover")
        };
        let mock_verifier_path = if cfg!(windows) {
            base_path.join("mock_webgpu_verifier.cmd")
        } else {
            base_path.join("mock_webgpu_verifier")
        };
        let shader_path = base_path.join("shader");
        
        // Create shader directory
        fs::create_dir_all(&shader_path)?;
        
        // Create mock prover script
        Self::create_mock_prover(&mock_prover_path)?;
        Self::create_mock_verifier(&mock_verifier_path)?;
        
        Ok(Self {
            temp_dir,
            mock_prover_path,
            mock_verifier_path,
            shader_path,
        })
    }
    
    /// Set environment variables to use mock binaries
    pub fn setup_env(&self) {
        std::env::set_var("LIGETRON_PROVER", &self.mock_prover_path);
        std::env::set_var("LIGETRON_VERIFIER", &self.mock_verifier_path);
        std::env::set_var("LIGETRON_SHADER_PATH", &self.shader_path);
    }
    
    /// Get the temporary directory path
    pub fn temp_path(&self) -> &Path {
        self.temp_dir.path()
    }
    
    /// Create a mock prover that simulates webgpu_prover behavior
    fn create_mock_prover(path: &Path) -> std::io::Result<()> {
        let script_content = if cfg!(windows) {
            // Windows batch script
            r#"@echo off
setlocal enabledelayedexpansion

REM Parse argument as either a file path or inline JSON
set "arg=%~1"
if exist "%arg%" (
    for /f "usebackq delims=" %%A in ("%arg%") do (
        set "json_arg=%%A"
    )
) else (
    set "json_arg=%arg%"
)

REM Extract program path (simplified parsing)
for /f "tokens=2 delims=:" %%a in ('echo !json_arg! ^| findstr "program"') do (
    set "program_path=%%a"
    set "program_path=!program_path:"=!"
    set "program_path=!program_path:,=!"
    set "program_path=!program_path: =!"
)

REM Check if program file exists
if not exist "!program_path!" (
    echo ERROR: Program file not found: !program_path! >&2
    exit /b 1
)

REM Simulate journal output
echo SOV_JOURNAL_HEX:deadbeef42424242

REM Create mock proof.data file
echo Mock proof data > proof.data

echo Mock Ligetron prover completed successfully
exit /b 0
"#
        } else {
            // Unix shell script
            r#"#!/bin/bash
set -e

# Parse argument as either a file path or inline JSON
ARG="$1"
if [ -f "$ARG" ]; then
    JSON_ARG="$(cat "$ARG")"
else
    JSON_ARG="$ARG"
fi

# Extract program path (simplified JSON parsing)
PROGRAM_PATH=$(echo "$JSON_ARG" | grep -o '"program":"[^"]*"' | cut -d'"' -f4)

if [ -z "$PROGRAM_PATH" ]; then
    echo "ERROR: Could not extract program path from JSON" >&2
    exit 1
fi

# Check if program file exists
if [ ! -f "$PROGRAM_PATH" ]; then
    echo "ERROR: Program file not found: $PROGRAM_PATH" >&2
    exit 1
fi

# Extract args to determine if we should output journal
ARGS=$(echo "$JSON_ARG" | grep -o '"args":\[[^]]*\]')

# Simulate journal output (this would normally come from the WASM program)
echo "SOV_JOURNAL_HEX:deadbeef42424242"

# Create mock proof.data file
echo "Mock proof data for testing" > proof.data

echo "Mock Ligetron prover completed successfully" >&2
exit 0
"#
        };
        
        fs::write(path, script_content)?;
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms)?;
        }
        
        Ok(())
    }
    
    /// Create a mock verifier that simulates webgpu_verifier behavior
    fn create_mock_verifier(path: &Path) -> std::io::Result<()> {
        let script_content = if cfg!(windows) {
            // Windows batch script
            r#"@echo off
setlocal enabledelayedexpansion

REM Parse argument as either a file path or inline JSON
set "arg=%~1"
if exist "%arg%" (
    for /f "usebackq delims=" %%A in ("%arg%") do (
        set "json_arg=%%A"
    )
) else (
    set "json_arg=%arg%"
)

REM Extract program path
for /f "tokens=2 delims=:" %%a in ('echo !json_arg! ^| findstr "program"') do (
    set "program_path=%%a"
    set "program_path=!program_path:"=!"
    set "program_path=!program_path:,=!"
    set "program_path=!program_path: =!"
)

REM Check if program and proof files exist
if not exist "!program_path!" (
    echo ERROR: Program file not found: !program_path! >&2
    exit /b 1
)

if not exist "proof.data" (
    echo ERROR: Proof file not found: proof.data >&2
    exit /b 1
)

echo Mock Ligetron verifier completed successfully
exit /b 0
"#
        } else {
            // Unix shell script
            r#"#!/bin/bash
set -e

# Parse argument as either a file path or inline JSON
ARG="$1"
if [ -f "$ARG" ]; then
    JSON_ARG="$(cat "$ARG")"
else
    JSON_ARG="$ARG"
fi

# Extract program path
PROGRAM_PATH=$(echo "$JSON_ARG" | grep -o '"program":"[^"]*"' | cut -d'"' -f4)

if [ -z "$PROGRAM_PATH" ]; then
    echo "ERROR: Could not extract program path from JSON" >&2
    exit 1
fi

# Check if program file exists
if [ ! -f "$PROGRAM_PATH" ]; then
    echo "ERROR: Program file not found: $PROGRAM_PATH" >&2
    exit 1
fi

# Check if proof file exists
if [ ! -f "proof.data" ]; then
    echo "ERROR: Proof file not found: proof.data" >&2
    exit 1
fi

echo "Mock Ligetron verifier completed successfully" >&2
exit 0
"#
        };
        
        fs::write(path, script_content)?;
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms)?;
        }
        
        Ok(())
    }
}

/// Test helper for creating mock WASM programs
pub fn create_mock_wasm_program(content: &str) -> Vec<u8> {
    // Create a fake WASM binary with the content as a comment
    let mut wasm = vec![
        0x00, 0x61, 0x73, 0x6d, // WASM magic number
        0x01, 0x00, 0x00, 0x00, // WASM version
    ];
    
    // Add content as bytes (this isn't valid WASM, but it's fine for testing)
    wasm.extend_from_slice(content.as_bytes());
    wasm
}

/// Advanced mock that can simulate different behaviors based on input
pub struct AdvancedMockLigetron {
    behaviors: HashMap<String, MockBehavior>,
}

#[derive(Clone, serde::Serialize)]
pub struct MockBehavior {
    pub should_succeed: bool,
    pub journal_hex: String,
    pub proof_data: Vec<u8>,
    pub stderr_output: String,
}

impl Default for MockBehavior {
    fn default() -> Self {
        Self {
            should_succeed: true,
            journal_hex: "deadbeef".to_string(),
            proof_data: b"mock proof data".to_vec(),
            stderr_output: "Mock completed successfully".to_string(),
        }
    }
}

impl AdvancedMockLigetron {
    pub fn new() -> Self {
        Self {
            behaviors: HashMap::new(),
        }
    }
    
    /// Configure behavior for a specific program (identified by content hash)
    pub fn configure_program_behavior(&mut self, program_content: &[u8], behavior: MockBehavior) {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(program_content);
        let hash = hex::encode(hasher.finalize());
        self.behaviors.insert(hash, behavior);
    }
    
    /// Create mock binaries with advanced behavior
    pub fn create_advanced_mocks(&self, base_path: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
        let prover_path = base_path.join("advanced_mock_prover");
        let verifier_path = base_path.join("advanced_mock_verifier");
        
        // Serialize behaviors to JSON for the mock scripts
        let behaviors_json = serde_json::to_string(&self.behaviors)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        
        let behaviors_file = base_path.join("mock_behaviors.json");
        fs::write(&behaviors_file, behaviors_json)?;
        
        // Create advanced prover script
        let prover_script = format!(r#"#!/bin/bash
set -e

BEHAVIORS_FILE="{}"
JSON_ARG="$1"

# Extract program path
PROGRAM_PATH=$(echo "$JSON_ARG" | grep -o '"program":"[^"]*"' | cut -d'"' -f4)

if [ ! -f "$PROGRAM_PATH" ]; then
    echo "ERROR: Program file not found: $PROGRAM_PATH" >&2
    exit 1
fi

# Calculate program hash
PROGRAM_HASH=$(sha256sum "$PROGRAM_PATH" | cut -d' ' -f1)

# Look up behavior (simplified - in practice you'd use jq)
# For now, just use default behavior
echo "SOV_JOURNAL_HEX:deadbeef42424242"
echo "Mock proof data" > proof.data

echo "Advanced mock prover completed" >&2
"#, behaviors_file.display());
        
        fs::write(&prover_path, prover_script)?;
        
        let verifier_script = format!(r#"#!/bin/bash
set -e

JSON_ARG="$1"

# Extract program path
PROGRAM_PATH=$(echo "$JSON_ARG" | grep -o '"program":"[^"]*"' | cut -d'"' -f4)

if [ ! -f "$PROGRAM_PATH" ] || [ ! -f "proof.data" ]; then
    echo "ERROR: Required files not found" >&2
    exit 1
fi

echo "Advanced mock verifier completed" >&2
"#);
        
        fs::write(&verifier_path, verifier_script)?;
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&prover_path, &verifier_path] {
                let mut perms = fs::metadata(path)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(path, perms)?;
            }
        }
        
        Ok((prover_path, verifier_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_mock_env_creation() {
        let env = MockLigetronEnv::new().unwrap();
        
        assert!(env.mock_prover_path.exists());
        assert!(env.mock_verifier_path.exists());
        assert!(env.shader_path.exists());
    }
    
    #[test]
    fn test_mock_wasm_program() {
        let wasm = create_mock_wasm_program("test program");
        
        // Should start with WASM magic number
        assert_eq!(&wasm[0..4], &[0x00, 0x61, 0x73, 0x6d]);
        assert_eq!(&wasm[4..8], &[0x01, 0x00, 0x00, 0x00]);
        
        // Should contain our test content
        let content_start = 8;
        let content = &wasm[content_start..];
        assert_eq!(content, b"test program");
    }
    
    #[test]
    fn test_advanced_mock_configuration() {
        let mut mock = AdvancedMockLigetron::new();
        let program = b"test program";
        
        let behavior = MockBehavior {
            should_succeed: false,
            journal_hex: "custom_journal".to_string(),
            proof_data: b"custom proof".to_vec(),
            stderr_output: "Custom error".to_string(),
        };
        
        mock.configure_program_behavior(program, behavior.clone());
        
        // Verify behavior was stored
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(program);
        let hash = hex::encode(hasher.finalize());
        
        assert!(mock.behaviors.contains_key(&hash));
    }
}
