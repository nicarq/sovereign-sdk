use anyhow::Context;

/// Deserialize the output of the metrics syscall
pub fn deserialize_metrics_call(serialized: &[u8]) -> anyhow::Result<(String, u64, u64)> {
    let null_pos = serialized
        .iter()
        .position(|&b| b == 0)
        .context("Could not find separator in provided bytes")?;

    let (name_bytes, metric_bytes_with_null) = serialized.split_at(null_pos);
    let name = std::str::from_utf8(name_bytes)
        .context("Invalid UTF-8 in name")?
        .to_owned();

    let cycles_bytes = &metric_bytes_with_null[1..9]; // Skip the null terminator
    let cycles = u64::from_le_bytes(cycles_bytes.try_into()?); // Convert bytes back into usize
                                                               // Upper bound so we don't panice if more things
    let free_heap_bytes = &metric_bytes_with_null[9..17];
    let free_heap = u64::from_le_bytes(free_heap_bytes.try_into()?);
    Ok((name, cycles, free_heap))
}
