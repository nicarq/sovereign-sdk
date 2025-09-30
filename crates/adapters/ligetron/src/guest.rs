//! This module implements the `ZkvmGuest` trait for the Ligetron zkVM.

#[cfg(target_arch = "wasm32")]
extern crate alloc;

use serde::de::DeserializeOwned;
use sov_rollup_interface::zk::ZkvmGuest;

use bincode::Options;
#[cfg(target_arch = "wasm32")]
use alloc::vec::Vec;
#[cfg(not(target_arch = "wasm32"))]
use std::vec::Vec;

#[cfg(target_arch = "wasm32")]
type InnerCell<T> = core::cell::RefCell<T>;
#[cfg(not(target_arch = "wasm32"))]
type InnerCell<T> = std::sync::Mutex<T>;

/// A guest implementation for Ligetron that provides in-host simulation capabilities.
/// 
/// This is primarily used for testing and simulation outside of the actual Ligetron zkVM environment.
/// In the real Ligetron WASM environment, hints and commits would be handled through the
/// Ligetron runtime and the sov_journal.h helper functions.
#[derive(Default)]
pub struct LigetronGuest {
    /// Hints provided by the host, stored as a bincode blob
    hints: InnerCell<Vec<u8>>,
    /// Committed values accumulated during execution
    commits: InnerCell<Vec<u8>>,
}

// wasm32 execution under ligero-prover is single-threaded; it's safe to mark this Send+Sync.
#[cfg(target_arch = "wasm32")]
unsafe impl Send for LigetronGuest {}
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for LigetronGuest {}

impl LigetronGuest {
    /// Create a new empty guest instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a guest instance with pre-loaded hints.
    /// This is used by the host's `simulate_with_hints()` method.
    pub fn with_hints(hints_blob: Vec<u8>) -> Self {
        Self {
            hints: InnerCell::new(hints_blob),
            commits: InnerCell::new(Vec::new()),
        }
    }

    /// Get the accumulated commits for testing purposes.
    /// This is not part of the ZkvmGuest trait but useful for verification in tests.
    #[cfg(test)]
    pub fn get_commits(&self) -> Vec<u8> {
        #[cfg(target_arch = "wasm32")]
        { self.commits.borrow().clone() }
        #[cfg(not(target_arch = "wasm32"))]
        { self.commits.lock().unwrap().clone() }
    }

    /// Return committed bytes when running in the native (non-wasm) environment.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn commits_as_bytes(&self) -> Vec<u8> {
        self.commits.lock().unwrap().clone()
    }
}

impl ZkvmGuest for LigetronGuest {
    type Verifier = crate::LigetronVerifier;

    fn read_from_host<T: DeserializeOwned>(&self) -> T {
        #[cfg(target_arch = "wasm32")]
        {
            // Debug: print hint length in hex (little endian u32)
            let mut hints_guard = self.hints.borrow_mut();
            debug_print_len_hex(hints_guard.len() as u32);

            let opts = bincode::DefaultOptions::new()
                .with_big_endian()
                .allow_trailing_bytes();
            match opts.deserialize::<T>(&hints_guard[..]) {
                Ok(item) => {
                    debug_print_line(b"DEBUG:HINTS_OK\n");
                    hints_guard.clear();
                    item
                }
                Err(err) => {
                    let msg = alloc::format!("DEBUG:HINTS_ERR:{err:?}\n");
                    debug_print_line(msg.as_bytes());
                    debug_print_line(b"DEBUG:HINTS_BAD\n");
                    // Trap to surface failure (same behavior as expect())
                    panic!("Failed to deserialize hint from host");
                }
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut hints_guard = self.hints.lock().unwrap();
            let mut cursor = std::io::Cursor::new(std::mem::take(&mut *hints_guard));
            let opts = bincode::DefaultOptions::new().with_big_endian();
            let item: T = opts
                .deserialize_from(&mut cursor)
                .expect("Failed to deserialize hint from host. Ensure hint types match between host and guest.");
            let position_after = cursor.position() as usize;
            let remaining = cursor.into_inner();
            if position_after < remaining.len() {
                *hints_guard = remaining[position_after..].to_vec();
            } else {
                hints_guard.clear();
            }
            item
        }
    }

    fn commit<T: serde::Serialize>(&self, item: &T) {
        #[cfg(target_arch = "wasm32")]
        let mut commits_guard = self.commits.borrow_mut();
        #[cfg(not(target_arch = "wasm32"))]
        let mut commits_guard = self.commits.lock().unwrap();
        
        // Serialize and append the item to the commits blob
        let serialized = bincode::serialize(item)
            .expect("Failed to serialize committed item");
        commits_guard.extend(serialized);

        // On WASM, emit journal using multiple methods for maximum compatibility
        #[cfg(target_arch = "wasm32")]
        {
            let last_item: Vec<u8> = bincode::serialize(item).expect("serialize must succeed");
            
            // Try Ligetron-specific journal emission first
            emit_journal_ligetron(&last_item);
            
            // Also emit via WASI as fallback (may not work but worth trying)
            emit_journal_hex(&last_item);
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn to_hex(dst: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    dst.reserve(bytes.len() * 2);
    for &b in bytes {
        dst.push(HEX[(b >> 4) as usize]);
        dst.push(HEX[(b & 0x0f) as usize]);
    }
}

#[cfg(target_arch = "wasm32")]
#[repr(C)]
struct Ciovec { ptr: *const u8, len: usize }

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn fd_write(fd: u32, iovs: *const Ciovec, iovs_len: u32, nwritten: *mut u32) -> u16;
}

#[cfg(target_arch = "wasm32")]
fn emit_journal_ligetron(data: &[u8]) {
    // Try multiple Ligetron journal emission methods
    
    // Method 1: sov_journal_emit (from sov_journal.h)
    extern "C" {
        fn sov_journal_emit(data: *const u8, len: usize);
    }
    unsafe {
        sov_journal_emit(data.as_ptr(), data.len());
    }
    
    // Method 2: ligetron_journal_emit (Ligetron-specific)
    extern "C" {
        fn ligetron_journal_emit(ptr: *const u8, len: usize);
    }
    unsafe {
        ligetron_journal_emit(data.as_ptr(), data.len());
    }
    
    // Method 3: Emit hex format to stdout as additional fallback
    let mut buf: Vec<u8> = Vec::with_capacity("SOV_JOURNAL_HEX:".len() + data.len() * 2 + 1);
    buf.extend_from_slice(b"SOV_JOURNAL_HEX:");
    to_hex(&mut buf, data);
    buf.push(b'\n');
    
    let iov = Ciovec { ptr: buf.as_ptr(), len: buf.len() };
    let mut nw: u32 = 0;
    unsafe { let _ = fd_write(1, &iov as *const Ciovec, 1, &mut nw as *mut u32); }
}

#[cfg(target_arch = "wasm32")]
fn emit_journal_hex(data: &[u8]) {
    let mut buf: Vec<u8> = Vec::with_capacity("SOV_JOURNAL_HEX:".len() + data.len() * 2 + 1);
    buf.extend_from_slice(b"SOV_JOURNAL_HEX:");
    to_hex(&mut buf, data);
    buf.push(b'\n');

    let iov = Ciovec { ptr: buf.as_ptr(), len: buf.len() };
    let mut nw: u32 = 0;
    unsafe { let _ = fd_write(1, &iov as *const Ciovec, 1, &mut nw as *mut u32); }
}

#[cfg(target_arch = "wasm32")]
fn debug_print_line(msg: &[u8]) {
    let iov = Ciovec { ptr: msg.as_ptr(), len: msg.len() };
    let mut nw: u32 = 0;
    unsafe { let _ = fd_write(1, &iov as *const Ciovec, 1, &mut nw as *mut u32); }
}

#[cfg(target_arch = "wasm32")]
fn debug_print_len_hex(len: u32) {
    let mut buf: Vec<u8> = Vec::with_capacity(32);
    buf.extend_from_slice(b"DEBUG:HINTS_LEN:");
    to_hex(&mut buf, &len.to_le_bytes());
    buf.push(b'\n');
    debug_print_line(&buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guest_creation() {
        let guest = LigetronGuest::new();
        assert!(guest.hints.lock().unwrap().is_empty());
        assert!(guest.commits.lock().unwrap().is_empty());
    }

    #[test]
    fn test_guest_with_hints() {
        let hints_data = vec![1, 2, 3, 4, 5];
        let guest = LigetronGuest::with_hints(hints_data.clone());
        assert_eq!(*guest.hints.lock().unwrap(), hints_data);
        assert!(guest.commits.lock().unwrap().is_empty());
    }

    #[test]
    fn test_read_from_host() {
        // Prepare hints blob with multiple items
        let mut hints_blob = Vec::new();
        bincode::serialize_into(&mut hints_blob, &42u64).unwrap();
        bincode::serialize_into(&mut hints_blob, &"hello".to_string()).unwrap();
        bincode::serialize_into(&mut hints_blob, &vec![1u8, 2, 3]).unwrap();

        let guest = LigetronGuest::with_hints(hints_blob);

        // Read items in order
        let item1: u64 = guest.read_from_host();
        assert_eq!(item1, 42);

        let item2: String = guest.read_from_host();
        assert_eq!(item2, "hello");

        let item3: Vec<u8> = guest.read_from_host();
        assert_eq!(item3, vec![1, 2, 3]);

        // Hints should be empty now
        assert!(guest.hints.lock().unwrap().is_empty());
    }

    #[test]
    fn test_commit() {
        let guest = LigetronGuest::new();

        // Commit some items
        guest.commit(&100u32);
        guest.commit(&"world".to_string());

        // Verify commits were accumulated
        let commits = guest.get_commits();
        assert!(!commits.is_empty());

        // Deserialize and verify
        let mut cursor = std::io::Cursor::new(commits);
        let committed1: u32 = bincode::deserialize_from(&mut cursor).unwrap();
        let committed2: String = bincode::deserialize_from(&mut cursor).unwrap();

        assert_eq!(committed1, 100);
        assert_eq!(committed2, "world");
    }

    #[test]
    fn test_round_trip_simulation() {
        // Simulate a complete host-guest interaction
        let mut hints_blob = Vec::new();
        bincode::serialize_into(&mut hints_blob, &"input_data".to_string()).unwrap();
        bincode::serialize_into(&mut hints_blob, &123u64).unwrap();

        let guest = LigetronGuest::with_hints(hints_blob);

        // Guest reads hints and processes them
        let input: String = guest.read_from_host();
        let number: u64 = guest.read_from_host();

        // Guest commits results
        let result = format!("{}_{}", input, number * 2);
        guest.commit(&result);

        // Verify the result
        let commits = guest.get_commits();
        let mut cursor = std::io::Cursor::new(commits);
        let final_result: String = bincode::deserialize_from(&mut cursor).unwrap();
        assert_eq!(final_result, "input_data_246");
    }

    #[test]
    #[should_panic(expected = "Failed to deserialize hint from host")]
    fn test_read_from_empty_hints() {
        let guest = LigetronGuest::new();
        let _: u64 = guest.read_from_host(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Failed to deserialize hint from host")]
    fn test_read_wrong_type() {
        let mut hints_blob = Vec::new();
        // Serialize a string, but try to read it as a complex struct
        bincode::serialize_into(&mut hints_blob, &"hello".to_string()).unwrap();

        let guest = LigetronGuest::with_hints(hints_blob);
        
        // Try to read as a complex struct - this should fail
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct ComplexStruct {
            field1: u64,
            field2: String,
            field3: Vec<u8>,
        }
        
        let _: ComplexStruct = guest.read_from_host(); // Should panic - wrong type
    }
}
