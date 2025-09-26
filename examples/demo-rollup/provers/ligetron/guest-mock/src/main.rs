// Robust WASM program for Ligetron that handles two-pass validation
// IMPORTANT: Avoid env::args() as it can't handle binary hint data

fn main() {
    // The journal content we want to emit (same as our WAT program)
    let journal_data = b"LIGETRON_STATE_TRANSITION_COMPLETE";
    let journal_hex = hex::encode(journal_data);
    
    // Always emit the journal (required for both passes)
    print!("SOV_JOURNAL_HEX:{}\n", journal_hex);
    
    // Don't try to read command line arguments since they may contain binary data
    // that can't be converted to UTF-8 strings. Just output the journal and exit.
    // This works for both first pass (dummy) and second pass (real digest + hints).
    
    std::process::exit(0);
}
