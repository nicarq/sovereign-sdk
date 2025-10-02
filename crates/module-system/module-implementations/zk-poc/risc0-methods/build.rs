use std::collections::HashMap;

use sov_zkvm_utils::should_skip_guest_build;

fn main() {
    println!("cargo::rerun-if-env-changed=SKIP_GUEST_BUILD");
    println!("cargo::rerun-if-env-changed=OUT_DIR");

    if should_skip_guest_build("risc0") {
        println!("cargo:warning=Skipping risc0 guest build for zk-poc");
        let out_dir = std::env::var_os("OUT_DIR").unwrap();
        let out_dir = std::path::Path::new(&out_dir);
        let methods_path = out_dir.join("methods.rs");

        let contents = r#"
            pub const EVEN_PATH: &str = "";
            pub const EVEN_ELF: &[u8] = b"";
        "#;

        std::fs::write(methods_path, contents).expect("Failed to write mock methods");
    } else {
        let mut guest_pkg_to_options = HashMap::new();
        let features = sov_zkvm_utils::collect_features(&["bench", "bincode"], &["native"]);
        let guest_options = risc0_build::GuestOptionsBuilder::default()
            .features(features)
            .build()
            .unwrap();
        guest_pkg_to_options.insert("zk-poc-risc0-guest-even", guest_options);

        risc0_build::embed_methods_with_options(guest_pkg_to_options);
    }
}

