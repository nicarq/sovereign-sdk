use anyhow::Result;

fn main() -> Result<()> {
    // Re-run build script if any of these files change
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=sov_journal.h");
    
    // Set environment variables that might be useful for the adapter
    if let Ok(prover_path) = std::env::var("LIGETRON_PROVER") {
        println!("cargo:rustc-env=LIGETRON_PROVER_DEFAULT={}", prover_path);
    }
    
    if let Ok(verifier_path) = std::env::var("LIGETRON_VERIFIER") {
        println!("cargo:rustc-env=LIGETRON_VERIFIER_DEFAULT={}", verifier_path);
    }
    
    if let Ok(shader_path) = std::env::var("LIGETRON_SHADER_PATH") {
        println!("cargo:rustc-env=LIGETRON_SHADER_PATH_DEFAULT={}", shader_path);
    }
    
    // Check if we should skip guest build for Ligetron
    if sov_zkvm_utils::should_skip_guest_build("ligetron") {
        println!("cargo:rustc-cfg=skip_guest_build");
    }
    
    Ok(())
}
