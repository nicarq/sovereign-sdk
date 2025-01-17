#[cfg(feature = "risc0")]
pub use actual_impl::*;

#[cfg(feature = "risc0")]
mod actual_impl {
    use std::hint::black_box;

    use risc0_zkvm;
    use risc0_zkvm_platform::syscall::{sys_cycle_count, SyscallName};

    use crate::cycle_utils::CycleMetric;

    /// Name of the syscall that is used to report metrics
    // Safety: string is null terminated
    pub const SYSCALL_NAME_METRICS: SyscallName =
        unsafe { SyscallName::from_bytes_with_nul("cycle_metrics\0".as_bytes().as_ptr()) };

    /// Gets the current cycle count
    pub fn get_cycle_count() -> u64 {
        sys_cycle_count()
    }

    /// Reports cycle count metrics to the zk guest
    pub fn report_cycle_count(metric: CycleMetric) {
        risc0_zkvm::guest::env::send_recv_slice::<u8, u8>(
            SYSCALL_NAME_METRICS,
            &bincode::serialize(&metric).unwrap(),
        );
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
}

#[cfg(not(feature = "risc0"))]
pub use facade::*;
#[cfg(not(feature = "risc0"))]
mod facade {
    use crate::cycle_utils::CycleMetric;

    /// Gets the current cycle count. Note: this function will always return 0 if the risc0 feature is not enabled!
    pub fn get_cycle_count() -> u64 {
        0
    }

    /// Reports the cycle counts to the zkvm. Note: this function will panic if the risc0 feature is not enabled!
    pub fn report_cycle_count(_metric: CycleMetric) {
        panic!("Reporting risc0 cycle count without risc0 feature enabled");
    }

    /// Gets the available heap. Note: this function will always return 0x0C00_0000 if the risc0 feature is not enabled!
    pub fn get_available_heap() -> u64 {
        0x0C00_0000
    }
}
