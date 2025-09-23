//! This module implements the `ZkvmGuest` trait for the Ligetron zkVM.

use serde::de::DeserializeOwned;
use sov_rollup_interface::zk::ZkvmGuest;
use std::sync::Mutex;

/// A guest implementation for Ligetron that provides in-host simulation capabilities.
/// 
/// This is primarily used for testing and simulation outside of the actual Ligetron zkVM environment.
/// In the real Ligetron WASM environment, hints and commits would be handled through the
/// Ligetron runtime and the sov_journal.h helper functions.
#[derive(Default)]
pub struct LigetronGuest {
    /// Hints provided by the host, stored as a bincode blob
    hints: Mutex<Vec<u8>>,
    /// Committed values accumulated during execution
    commits: Mutex<Vec<u8>>,
}

impl LigetronGuest {
    /// Create a new empty guest instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a guest instance with pre-loaded hints.
    /// This is used by the host's `simulate_with_hints()` method.
    pub fn with_hints(hints_blob: Vec<u8>) -> Self {
        Self {
            hints: Mutex::new(hints_blob),
            commits: Mutex::new(Vec::new()),
        }
    }

    /// Get the accumulated commits for testing purposes.
    /// This is not part of the ZkvmGuest trait but useful for verification in tests.
    #[cfg(test)]
    pub fn get_commits(&self) -> Vec<u8> {
        self.commits.lock().unwrap().clone()
    }
}

impl ZkvmGuest for LigetronGuest {
    type Verifier = crate::LigetronVerifier;

    fn read_from_host<T: DeserializeOwned>(&self) -> T {
        let mut hints_guard = self.hints.lock().unwrap();
        
        // Create a cursor from the current hints blob
        let mut cursor = std::io::Cursor::new(std::mem::take(&mut *hints_guard));
        
        // Deserialize the next item from the blob
        let item: T = bincode::deserialize_from(&mut cursor)
            .expect("Failed to deserialize hint from host. Ensure hint types match between host and guest.");
        
        // Get the position after deserializing
        let position_after = cursor.position() as usize;
        
        // Store the remaining bytes back
        let remaining = cursor.into_inner();
        if position_after < remaining.len() {
            *hints_guard = remaining[position_after..].to_vec();
        } else {
            hints_guard.clear();
        }
        
        item
    }

    fn commit<T: serde::Serialize>(&self, item: &T) {
        let mut commits_guard = self.commits.lock().unwrap();
        
        // Serialize and append the item to the commits blob
        let serialized = bincode::serialize(item)
            .expect("Failed to serialize committed item");
        commits_guard.extend(serialized);
    }
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
