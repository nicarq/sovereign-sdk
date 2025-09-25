use std::sync::OnceLock;

use lazy_static::lazy_static;

fn should_skip_guest_build() -> bool {
    matches!(
        std::env::var("SKIP_GUEST_BUILD")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("ligetron")
    )
}

/// Attempt to load the WASM file from an env var or a fallback path.
/// Fails fast if the file cannot be read when guest builds are expected.
fn load_wasm_env_or_path(env_key: &str, fallback_path: &str) -> &'static [u8] {
    static BUF: OnceLock<Vec<u8>> = OnceLock::new();
    BUF.get_or_init(|| {
        // 1) If an explicit env override is provided, use it regardless of SKIP_GUEST_BUILD.
        if let Ok(explicit) = std::env::var(env_key) {
            let path = explicit;
            match std::fs::read(&path) {
                Ok(bytes) if !bytes.is_empty() => return bytes,
                Ok(_) => panic!(
                    "Ligetron guest WASM at '{}' is empty. Please rebuild the guest for target `wasm32-wasi`.",
                    path
                ),
                Err(err) => panic!(
                    "Failed to load Ligetron guest WASM at '{}': {}. Ensure the guest has been built for the `wasm32-wasi` target (run `rustup target add wasm32-wasi` and rebuild).",
                    path,
                    err
                ),
            }
        }

        // 2) If we're explicitly skipping guest builds, return empty to allow non-proving modes.
        if should_skip_guest_build() {
            return Vec::new();
        }

        // 3) Fall back to the repo-local path.
        let candidate = fallback_path.to_string();
        match std::fs::read(&candidate) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => panic!(
                "Ligetron guest WASM at '{}' is empty. Please rebuild the guest for target `wasm32-wasi`.",
                candidate
            ),
            Err(err) => panic!(
                "Failed to load Ligetron guest WASM at '{}': {}. Ensure the guest has been built for the `wasm32-wasi` target (run `rustup target add wasm32-wasi` and rebuild).",
                candidate,
                err
            ),
        }
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
