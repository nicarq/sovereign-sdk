#![cfg(feature = "native")]

use serde::{Deserialize, Serialize};
use sov_ligetron_adapter::{Ligetron, LigetronMethodId};
use sov_rollup_interface::zk::{CodeCommitment, Zkvm, ZkvmHost, ZkvmGuest};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TestData {
    value: u64,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TestOutput {
    result: u64,
    processed_message: String,
}

#[test]
fn test_ligetron_adapter_basic() {
    // Test basic adapter functionality without actually running Ligetron
    // (since we don't have the binaries in CI)
    
    const FAKE_WASM: &[u8] = b"fake wasm program for testing";
    let mut host = <Ligetron as Zkvm>::Host::from_args(&FAKE_WASM);
    
    // Test adding hints
    let test_input = TestData {
        value: 42,
        message: "hello world".to_string(),
    };
    host.add_hint(&test_input);
    
    // Test code commitment generation
    let commitment = host.code_commitment();
    
    // Verify commitment is deterministic
    let commitment2 = host.code_commitment();
    assert_eq!(commitment, commitment2);
    
    // Test commitment encoding/decoding
    let encoded = commitment.encode();
    let decoded = LigetronMethodId::decode(&encoded).unwrap();
    assert_eq!(commitment, decoded);
}

#[test]
fn test_guest_simulation() {
    // Test the guest simulation functionality
    const FAKE_WASM: &[u8] = b"fake wasm program";
    let mut host = <Ligetron as Zkvm>::Host::from_args(&FAKE_WASM);
    
    // Add some test hints
    let input1 = TestData {
        value: 100,
        message: "first input".to_string(),
    };
    let input2 = TestData {
        value: 200,
        message: "second input".to_string(),
    };
    
    host.add_hint(&input1);
    host.add_hint(&input2);
    
    // Create a guest simulation
    let guest = host.simulate_with_hints();
    
    // Test reading hints
    let read_input1: TestData = guest.read_from_host();
    let read_input2: TestData = guest.read_from_host();
    
    assert_eq!(read_input1, input1);
    assert_eq!(read_input2, input2);
    
    // Test committing outputs
    let output = TestOutput {
        result: read_input1.value + read_input2.value,
        processed_message: format!("{} + {}", read_input1.message, read_input2.message),
    };
    
    guest.commit(&output);
    
    // In a real scenario, the committed data would be extracted by the host
    // and included in the proof package
}

#[test]
fn test_method_id_properties() {
    // Test that method ID is deterministic and based on program content
    const PROGRAM1: &[u8] = b"program version 1";
    const PROGRAM2: &[u8] = b"program version 2";
    const PROGRAM1_COPY: &[u8] = b"program version 1";
    
    let host1 = <Ligetron as Zkvm>::Host::from_args(&PROGRAM1);
    let host2 = <Ligetron as Zkvm>::Host::from_args(&PROGRAM2);
    let host1_copy = <Ligetron as Zkvm>::Host::from_args(&PROGRAM1_COPY);
    
    let id1 = host1.code_commitment();
    let id2 = host2.code_commitment();
    let id1_copy = host1_copy.code_commitment();
    
    // Same program should produce same ID
    assert_eq!(id1, id1_copy);
    
    // Different programs should produce different IDs
    assert_ne!(id1, id2);
}

#[test]
fn test_hint_serialization_order() {
    // Test that hints are serialized and deserialized in the correct order
    const FAKE_WASM: &[u8] = b"test program";
    let mut host = <Ligetron as Zkvm>::Host::from_args(&FAKE_WASM);
    
    // Add hints of different types in a specific order
    host.add_hint(&42u64);
    host.add_hint(&"test string".to_string());
    host.add_hint(&vec![1u8, 2, 3, 4]);
    host.add_hint(&true);
    
    let guest = host.simulate_with_hints();
    
    // Read them back in the same order
    let hint1: u64 = guest.read_from_host();
    let hint2: String = guest.read_from_host();
    let hint3: Vec<u8> = guest.read_from_host();
    let hint4: bool = guest.read_from_host();
    
    assert_eq!(hint1, 42u64);
    assert_eq!(hint2, "test string");
    assert_eq!(hint3, vec![1u8, 2, 3, 4]);
    assert_eq!(hint4, true);
}

#[test]
fn test_empty_hints() {
    // Test behavior with no hints
    const FAKE_WASM: &[u8] = b"minimal program";
    let mut host = <Ligetron as Zkvm>::Host::from_args(&FAKE_WASM);
    
    let guest = host.simulate_with_hints();
    
    // Trying to read from empty hints should panic
    // This is expected behavior as documented in the guest implementation
    std::panic::catch_unwind(|| {
        let _: u64 = guest.read_from_host();
    }).expect_err("Reading from empty hints should panic");
}

#[test]
fn test_complex_data_structures() {
    // Test with more complex nested data structures
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct ComplexData {
        nested: TestData,
        array: Vec<u32>,
        optional: Option<String>,
    }
    
    const FAKE_WASM: &[u8] = b"complex program";
    let mut host = <Ligetron as Zkvm>::Host::from_args(&FAKE_WASM);
    
    let complex_input = ComplexData {
        nested: TestData {
            value: 999,
            message: "nested message".to_string(),
        },
        array: vec![10, 20, 30, 40],
        optional: Some("optional value".to_string()),
    };
    
    host.add_hint(&complex_input);
    
    let guest = host.simulate_with_hints();
    let read_complex: ComplexData = guest.read_from_host();
    
    assert_eq!(read_complex, complex_input);
}
