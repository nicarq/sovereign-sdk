use std::sync::OnceLock;

use lazy_static::lazy_static;

/// Attempt to load the WASM file from an env var or a fallback path. If the file is empty or
/// cannot be read, a warning is printed and an empty vector is returned.
fn load_wasm_env_or_path(env_key: &str, fallback_path: &str) -> &'static [u8] {
    static BUF: OnceLock<Vec<u8>> = OnceLock::new();
    BUF.get_or_init(|| {
        let candidate = std::env::var(env_key).unwrap_or_else(|_| fallback_path.to_string());
        let wasm = std::fs::read(&candidate).unwrap_or_default();
        if wasm.is_empty() {
            println!(
                "Warning: WASM file not found or empty at '{}' (env {} or fallback '{}')",
                candidate, env_key, fallback_path
            );
        }
        wasm
    })
    .as_slice()
}

// Initialize the Ligetron guest WASMs. Load from env if provided, else from local path.
lazy_static! {
    pub static ref LIGETRON_GUEST_MOCK_WASM: &'static [u8] = load_wasm_env_or_path(
        "LIGETRON_WASM_MOCK",
        &format!("{}/guest-mock/wasm/program.wasm", env!("CARGO_MANIFEST_DIR"))
    );
    pub static ref LIGETRON_GUEST_CELESTIA_WASM: &'static [u8] = load_wasm_env_or_path(
        "LIGETRON_WASM_CELESTIA",
        &format!("{}/guest-celestia/wasm/program.wasm", env!("CARGO_MANIFEST_DIR"))
    );
}
