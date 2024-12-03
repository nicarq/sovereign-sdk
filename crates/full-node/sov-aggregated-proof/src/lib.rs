use borsh::from_slice;
use risc0_zkp::core::digest::Digest;
use risc0_zkvm::Receipt;
use std::fs;

fn read_receipt(file_path: &str) -> anyhow::Result<Receipt> {
    let data: Vec<u8> = fs::read(file_path).unwrap();
    Ok(from_slice::<Receipt>(&data)?)
}

pub fn check_receipts(file_paths: Vec<&str>, method_id: Digest) -> anyhow::Result<()> {
    let receipts = file_paths
        .into_iter()
        .map(|file_path| read_receipt(&file_path))
        .collect::<Result<Vec<_>, _>>()?;

    for receipt in receipts {
        receipt.verify(method_id).map_err(|e| anyhow::anyhow!(e))?;
    }
    Ok(())
}
