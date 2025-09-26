#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use alloc::format;

#[cfg(feature = "tiny")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

fn main() {
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:START\n");
    
    // For now, let's completely bypass the STF framework and just emit a journal
    // This will test if the basic journal emission mechanism works
    
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:SKIP_STF\n");
    
    // Create the journal data
    let journal_data = b"LIGETRON_STATE_TRANSITION_COMPLETE";
    let journal_hex = hex::encode(journal_data);
    
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:JOURNAL_CREATED\n");
    
    // Emit the journal in the expected format
    let output = format!("SOV_JOURNAL_HEX:{}\n", journal_hex);
    
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = output.as_bytes();
        let iov = Ciovec { ptr: bytes.as_ptr(), len: bytes.len() };
        let mut nw: u32 = 0; 
        unsafe { 
            let result = fd_write(1, &iov, 1, &mut nw as *mut u32);
            if result == 0 {
                dbg(b"DEBUG:JOURNAL_EMITTED\n");
            } else {
                dbg(b"DEBUG:JOURNAL_FAILED\n");
            }
        }
    }
    
    #[cfg(target_arch = "wasm32")]
    dbg(b"DEBUG:COMPLETE\n");
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
    let mut nw: u32 = 0; 
    unsafe { let _ = fd_write(1, &iov, 1, &mut nw as *mut u32); }
}