#![no_std]
// Build as a WASI command module so ligero-prover can call `_start`.
extern crate alloc;

use const_rollup_config::{ROLLUP_BATCH_NAMESPACE_RAW, ROLLUP_PROOF_NAMESPACE_RAW};
use demo_stf::runtime::Runtime;
use demo_stf::StfVerifier;
use sov_address::MultiAddressEvm;
use sov_celestia_adapter::types::Namespace;
use sov_celestia_adapter::verifier::{CelestiaSpec, CelestiaVerifier};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Zk;
use sov_modules_stf_blueprint::StfBlueprint;
use sov_ligetron_adapter::guest::LigetronGuest;
use sov_ligetron_adapter::Ligetron;
use sov_rollup_interface::da::DaVerifier;
use sov_state::ZkStorage;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
extern crate hex;

#[cfg(feature = "tiny")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

// The rollup stores its data in the namespace b"sov-test" on Celestia
const ROLLUP_BATCH_NAMESPACE: Namespace = Namespace::const_v0(ROLLUP_BATCH_NAMESPACE_RAW);
const ROLLUP_PROOF_NAMESPACE: Namespace = Namespace::const_v0(ROLLUP_PROOF_NAMESPACE_RAW);

fn main() {
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:START\n");
    
    // Use proper Ligetron runtime functions to read hints from args[2..N]
    fn read_hints_from_ligetron() -> Vec<u8> {
        // Ligetron runtime functions (provided by the prover)
        extern "C" {
            fn ligetron_num_args() -> usize;
            fn ligetron_arg_len(i: usize) -> usize;  // 1-based indexing
            fn ligetron_arg_copy(i: usize, out: *mut u8, out_len: usize) -> bool;
        }

        unsafe {
            let num_args = ligetron_num_args();
            #[cfg(target_arch = "wasm32")]
            {
                // Debug: print number of args
                let mut debug_msg = [b'D', b'E', b'B', b'U', b'G', b':', b'N', b'U', b'M', b'_', b'A', b'R', b'G', b'S', b':', 0, 0, 0, 0, b'\n'];
                let num_hex = format!("{:04x}", num_args);
                let hex_bytes = num_hex.as_bytes();
                if hex_bytes.len() >= 4 {
                    debug_msg[15] = hex_bytes[0];
                    debug_msg[16] = hex_bytes[1]; 
                    debug_msg[17] = hex_bytes[2];
                    debug_msg[18] = hex_bytes[3];
                }
                dbg(&debug_msg);
            }
            
            if num_args <= 1 {
                return Vec::new(); // No hints
            }

            // Calculate total size of hints (args[2..N] in 1-based indexing)
            let mut total_size = 0;
            for i in 2..=num_args {
                total_size += ligetron_arg_len(i);
            }

            #[cfg(target_arch = "wasm32")]
            {
                // Debug: print total hints size
                let mut debug_msg = [b'D', b'E', b'B', b'U', b'G', b':', b'H', b'I', b'N', b'T', b'S', b'_', b'S', b'I', b'Z', b'E', b':', 0, 0, 0, 0, 0, 0, 0, 0, b'\n'];
                let size_hex = format!("{:08x}", total_size);
                let hex_bytes = size_hex.as_bytes();
                if hex_bytes.len() >= 8 {
                    for (j, &byte) in hex_bytes.iter().enumerate() {
                        if j < 8 {
                            debug_msg[17 + j] = byte;
                        }
                    }
                }
                dbg(&debug_msg);
            }

            if total_size == 0 {
                return Vec::new();
            }

            // Allocate buffer and read all hint chunks
            let mut hints = vec![0u8; total_size];
            let mut offset = 0;

            for i in 2..=num_args {
                let arg_len = ligetron_arg_len(i);
                if arg_len > 0 && offset + arg_len <= hints.len() {
                    if ligetron_arg_copy(i, hints.as_mut_ptr().add(offset), arg_len) {
                        offset += arg_len;
                    } else {
                        #[cfg(target_arch = "wasm32")]
                        dbg(b"DEBUG:ARG_COPY_FAILED\n");
                        return Vec::new();
                    }
                }
            }

            hints.truncate(offset);
            hints
        }
    }

    let hints_blob = read_hints_from_ligetron();
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:ARGS_OK\n");
    
    // Create Ligetron guest with the hints blob
    // The guest will deserialize the StateTransitionWitnessWithAddress when read_from_host() is called
    let guest = LigetronGuest::with_hints(hints_blob);
    
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:PROCESSING_WITNESS\n");
    
    let storage = ZkStorage::new();
    let stf: StfBlueprint<
        ConfigurableSpec<CelestiaSpec, Ligetron, MockZkvm, MultiAddressEvm, Zk>,
        Runtime<_>,
    > = StfBlueprint::new();

    let rollup_params = sov_celestia_adapter::verifier::RollupParams {
        rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
        rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
    };

    let stf_verifier =
        StfVerifier::<_, _, _, Ligetron, MockZkvm>::new(stf, CelestiaVerifier::new(rollup_params));

    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:CALLING_RUN_BLOCK\n");

    let result = stf_verifier
        .run_block(guest, storage)
        .expect("Prover must be honest");
        
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:RUN_BLOCK_SUCCESS\n");
    
    // Emit journal for extraction by the host
    // For now, emit a simple deterministic journal based on the execution
    let journal_data = b"celestia_journal_output";
    
    // Use the proper Ligetron journal emission mechanism
    #[cfg(target_arch = "wasm32")]
    {
        // Emit journal using Ligetron's sov_journal.h functions
        extern "C" {
            fn sov_journal_emit(data: *const u8, len: usize);
        }
        unsafe {
            sov_journal_emit(journal_data.as_ptr(), journal_data.len());
        }
    }
    
    #[cfg(not(target_arch = "wasm32"))]
    {
        // For non-WASM environments, print to stdout
        let journal_hex = format!("SOV_JOURNAL_HEX:{}", hex::encode(journal_data));
        println!("{}", journal_hex);
    }
}
#[cfg(target_arch = "wasm32")]
#[repr(C)]
struct Ciovec { ptr: *const u8, len: usize }

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" { fn fd_write(fd: u32, iovs: *const Ciovec, iovs_len: u32, nwritten: *mut u32) -> u16; }

#[cfg(target_arch = "wasm32")]
fn dbg(msg: &[u8]) {
    let iov = Ciovec { ptr: msg.as_ptr(), len: msg.len() };
    let mut nw: u32 = 0; unsafe { let _ = fd_write(1, &iov, 1, &mut nw as *mut u32); }
}
