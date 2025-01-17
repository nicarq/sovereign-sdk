#[cfg(feature = "risc0")]
pub use actual_impl::*;

#[cfg(feature = "risc0")]
mod actual_impl {
    use std::hint::black_box;

    use risc0_zkvm;
    use risc0_zkvm_platform::syscall::{sys_cycle_count, SyscallName};

    fn serialize_metric(name: &str, cycle_count: u64, free_heap_bytes: u64) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        // We know the exact capacity:
        // name_bytes plus one null terminator plus two u64s (16 bytes total)
        let mut serialized = Vec::with_capacity(name_bytes.len() + 1 + 16);
        serialized.extend_from_slice(name_bytes);
        serialized.push(0);
        serialized.extend_from_slice(&cycle_count.to_le_bytes());
        serialized.extend_from_slice(&free_heap_bytes.to_le_bytes());
        serialized
    }

    /// Name of the syscall that is used to report metrics
    // Safety: string is null terminated
    pub const SYSCALL_NAME_METRICS: SyscallName =
        unsafe { SyscallName::from_bytes_with_nul("cycle_metrics\0".as_bytes().as_ptr()) };

    /// Gets the current cycle count
    pub fn get_cycle_count() -> u64 {
        sys_cycle_count()
    }

    /// Reports cycle count metrics to the zk guest
    pub fn report_cycle_count(name: &str, cycle_count: u64, free_heap_bytes: u64) {
        let serialized = serialize_metric(name, cycle_count, free_heap_bytes);
        risc0_zkvm::guest::env::send_recv_slice::<u8, u8>(SYSCALL_NAME_METRICS, &serialized);
    }

    /// Returns how many bytes of heap are still available
    pub fn get_available_heap() -> u64 {
        // TODO hack, this is allocating just to get a pointer to the top of the heap.
        // Assumes bump alloc
        // When embed alloc is fixed https://github.com/risc0/risc0/pull/2677 can use that.
        let new_alloc = black_box(Box::new(()));
        let available = 0x0C00_0000 - &new_alloc as *const _ as usize;
        available as u64
    }

    #[cfg(all(test, feature = "native"))]
    mod tests {
        use super::*;
        use crate::zkvm::deserialize_metrics_call;

        fn check_in_out(name: &str, cycle_count: u64, free_heap_bytes: u64) {
            let serialized = serialize_metric(name, cycle_count, free_heap_bytes);

            let (de_name, de_cycles, de_heap) = deserialize_metrics_call(&serialized[..]).unwrap();

            assert_eq!(de_name, name, "wrong metric name");
            assert_eq!(de_cycles, cycle_count, "wrong cycle count");
            assert_eq!(de_heap, free_heap_bytes, "wrong free heap");
        }

        #[test]
        fn callback_serialize_and_deserialize() {
            let cases = vec![
                ("zeros", 0, 0),
                ("something", 1024, 4095),
                ("different", 9056, 3870),
                ("one_max", u64::MAX, 514),
                ("two_max", 512, u64::MAX),
            ];
            for (name, cycles, heap_bytes) in cases {
                check_in_out(name, cycles, heap_bytes);
            }
        }
    }
}

#[cfg(not(feature = "risc0"))]
pub use facade::*;
#[cfg(not(feature = "risc0"))]
mod facade {
    /// Gets the current cycle count. Note: this function will always return 0 if the risc0 feature is not enabled!
    pub fn get_cycle_count() -> u64 {
        0
    }

    /// Reports the cycle counts to the zkvm. Note: this function will panic if the risc0 feature is not enabled!
    pub fn report_cycle_count(_name: &str, _count: u64, _free_heap_bytes: u64) {
        panic!("Reporting risc0 cycle count without risc0 feature enabled");
    }

    /// Gets the available heap. Note: this function will always return 0x0C00_0000 if the risc0 feature is not enabled!
    pub fn get_available_heap() -> u64 {
        0x0C00_0000
    }
}
