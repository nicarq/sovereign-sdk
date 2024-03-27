use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};
fn main() {
    let is_risczero_installed = Command::new("cargo")
        .args(["risczero", "help"])
        .status()
        .unwrap_or(ExitStatus::from_raw(1)); // If we can't execute the command, assume risczero isn't installed since duplicate install attempts are no-ops.

    if !is_risczero_installed.success() {
        // If installation fails, just exit silently. The user can try again.
        let _ = Command::new("cargo")
            .args(["install", "cargo-risczero"])
            .status();
    }

    let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_else(|_| "0".to_string());
    if skip_guest_build == "1" {
        println!("cargo:rustc-cfg=skip_guest_build");
    }
}
