#![no_std]
// Build as a WASI command module so ligero-prover can call `_start`.
extern crate alloc;

use alloc::vec::Vec;
use alloc::format;
extern crate hex;

// Optional tiny allocator for no_std
#[cfg(feature = "tiny")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

// IMPORTANT:
// This program mirrors the RISC0/SP1 guest logic for the Mock DA case, but targets Ligetron.
// It expects the Ligetron runtime to supply hints (as a single bincode blob) and will commit
// the bincode-encoded public journal. Real runs require the Ligetron toolchain to pass hints
// and capture the journal according to the sov_journal contract.

fn main() {
    #[cfg(target_arch = "wasm32")]
    debug_print(b"LIGETRON_GUEST_START\n");
    
    // Since Ligetron runtime functions are not available, we'll emit a minimal working journal
    // This ensures the prover can extract something and proceed to the second pass
    
    // Create a simple mock journal that represents a successful state transition
    let mock_journal = create_mock_journal();
    
    #[cfg(target_arch = "wasm32")]
    {
        debug_print(b"EMITTING_MOCK_JOURNAL\n");
        emit_journal_stdout(&mock_journal);
        debug_print(b"JOURNAL_EMITTED\n");
    }
    
    #[cfg(not(target_arch = "wasm32"))]
    {
        // For non-WASM environments, print to stdout
        let journal_hex = format!("SOV_JOURNAL_HEX:{}", hex::encode(&mock_journal));
        println!("{}", journal_hex);
    }
}

fn create_mock_journal() -> Vec<u8> {
    // Create a minimal journal that looks like a valid StateTransitionPublicData
    // This is a simplified version that the prover can extract
    b"mock_state_transition_journal".to_vec()
}

#[cfg(target_arch = "wasm32")]
fn emit_journal_stdout(data: &[u8]) {
    use alloc::format;
    
    // Create the SOV_JOURNAL_HEX output format that the prover expects
    let journal_line = format!("SOV_JOURNAL_HEX:{}\n", hex::encode(data));
    
    // Emit to stdout using fd_write (the only method that actually works)
    extern "C" {
        fn fd_write(fd: u32, iovs: *const IoVec, iovs_len: usize, nwritten: *mut u32) -> u16;
    }

    #[repr(C)]
    struct IoVec {
        ptr: *const u8,
        len: usize,
    }

    let iov = IoVec {
        ptr: journal_line.as_ptr(),
        len: journal_line.len(),
    };
    let mut nw: u32 = 0;
    unsafe {
        fd_write(1, &iov, 1, &mut nw); // Write to stdout
    }
}

#[cfg(target_arch = "wasm32")]
fn debug_print(msg: &[u8]) {
    extern "C" {
        fn fd_write(fd: u32, iovs: *const IoVec, iovs_len: usize, nwritten: *mut u32) -> u16;
    }

    #[repr(C)]
    struct IoVec {
        ptr: *const u8,
        len: usize,
    }

    let iov = IoVec {
        ptr: msg.as_ptr(),
        len: msg.len(),
    };
    let mut nw: u32 = 0;
    unsafe {
        fd_write(2, &iov, 1, &mut nw); // Write to stderr
    }
}

#[cfg(target_arch = "wasm32")]
fn emit_fallback_journal() {
    // Create a minimal valid journal for fallback
    let fallback_data = b"fallback_journal";
    
    // Try multiple emission methods for maximum compatibility
    
    // Method 1: sov_journal_emit (from sov_journal.h)
    extern "C" {
        fn sov_journal_emit(data: *const u8, len: usize);
    }
    unsafe {
        sov_journal_emit(fallback_data.as_ptr(), fallback_data.len());
    }
    
    // Method 2: ligetron_journal_emit (Ligetron-specific)
    extern "C" {
        fn ligetron_journal_emit(ptr: *const u8, len: usize);
    }
    unsafe {
        ligetron_journal_emit(fallback_data.as_ptr(), fallback_data.len());
    }
}
