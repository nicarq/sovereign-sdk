use std::fs;
use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use sov_risc0_adapter::host::Risc0Host;
use sov_rollup_interface::zk::{CodeCommitment, ZkvmHost};

/// Simple proof generator for zk-poc's even-check guest.
#[derive(Parser, Debug)]
#[command(author, version, about = "Generate a zk-poc proof for a value", long_about = None)]
struct Args {
    /// Value to prove is even.
    #[arg(long)]
    value: u64,

    /// Output path for raw proof bytes (bincode of Proof::Full(Receipt)).
    #[arg(long, default_value = "zkpoc_proof.bin")]
    out: PathBuf,

    /// Output path for hex-encoded proof prefixed with 0x (for JSON import).
    #[arg(long, default_value = "zkpoc_proof.hex")]
    hex_out: PathBuf,

    /// Optional output for method id (32 bytes) as hex string (0x...).
    #[arg(long)]
    method_out: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let elf = zk_poc_risc0_methods::EVEN_ELF;
    if elf.is_empty() {
        anyhow::bail!("EVEN_ELF is empty; build the guest or unset SKIP_GUEST_BUILD");
    }

    let mut host = Risc0Host::new(elf);
    host.add_hint(args.value);

    // Compute method id for convenience
    let method_id_vec = host.code_commitment().encode();
    if let Some(path) = args.method_out.as_ref() {
        let hex = format!("0x{}", hex::encode(&method_id_vec));
        fs::write(path, hex)?;
    }

    // Generate proof bytes
    let proof = ZkvmHost::run(&mut host, true /* with_proof */)?;

    // Write raw proof
    {
        let mut f = fs::File::create(&args.out)?;
        f.write_all(&proof)?;
    }

    // Write hex proof (0x...)
    let hex_str = format!("0x{}", hex::encode(&proof));
    fs::write(&args.hex_out, &hex_str)?;

    println!("Generated proof for value {}:", args.value);
    println!("  raw  -> {}", args.out.display());
    println!("  hex  -> {}", args.hex_out.display());
    if let Some(path) = args.method_out.as_ref() {
        println!("  mid  -> {}", path.display());
    }

    Ok(())
}
