use std::process::Command;

use sp1_build::{build_program_with_args, BuildArgs};

fn should_skip_guest_build() -> bool {
    match std::env::var("SKIP_GUEST_BUILD")
        .as_ref()
        .map(|arg0: &String| String::as_str(arg0))
    {
        Ok("1") | Ok("true") | Ok("sp1") => true,
        Ok("0") | Ok("false") | Ok(_) | Err(_) => false,
    }
}

fn parse_version_string(string: &str) -> anyhow::Result<String> {
    let version = string
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse version string"))?
        .split('-')
        .next()
        .unwrap();
    Ok(version.to_string())
}

fn main() -> anyhow::Result<()> {
    println!("cargo::rerun-if-env-changed=SKIP_GUEST_BUILD");
    println!("cargo::rerun-if-env-changed=OUT_DIR");

    if should_skip_guest_build() {
        println!("cargo:warning=Skipping sp1 guest build");
        return Ok(());
    }

    let native_version_cmd = Command::new("rustc")
        .arg("--version")
        .output()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    if !native_version_cmd.status.success() {
        anyhow::bail!(
            "Failed to get native rustc version: {:?}",
            native_version_cmd
        );
    }

    let sp1_cmd_output = Command::new("rustc")
        .env("RUSTUP_TOOLCHAIN", "succinct")
        .arg("--version")
        .output()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    if !sp1_cmd_output.status.success() {
        anyhow::bail!("Failed to get SP1 rustc version. Is SP1 installed? If not you can install it with the `sp1up` tool. Try running:\n\n   curl -L https://sp1.succinct.xyz | bash && sp1up\n\nOutput: {:?}", sp1_cmd_output);
    }

    let sp1_rustc_version = parse_version_string(&String::from_utf8_lossy(&sp1_cmd_output.stdout))?;
    let native_rustc_version =
        parse_version_string(&String::from_utf8_lossy(&native_version_cmd.stdout))?;

    if sp1_rustc_version != native_rustc_version {
        anyhow::bail!(
            "SP1 rustc version {} does not match native rustc version {}. Please \
            update your SP1 toolchain or use a rust-toolchain.toml file to force your \
            native compiler to the correct version.\n\n   To install a specific version of the SP1 \
            rust toolchain, use the command `sp1up -C {{commit}}` where {{commit}} is a release commit hash.\n   You can find a \
            list of available versions at https://github.com/succinctlabs/sp1/releases\
            For reproducible builds, use `sp1up -C <commit>` with a specific commit hash.\n",
            sp1_rustc_version,
            native_rustc_version
        );
    }

    let mut features = vec![];

    if cfg!(feature = "bench") {
        features.push("bench".to_string());
    }

    build_program_with_args(
        "./guest-mock",
        BuildArgs {
            features: features.clone(),
            ..Default::default()
        },
    );
    build_program_with_args(
        "./guest-celestia",
        BuildArgs {
            features,
            ..Default::default()
        },
    );

    Ok(())
}
