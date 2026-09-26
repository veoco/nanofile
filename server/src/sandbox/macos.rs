//! How macOS states the memory bound.
//!
//! Darwin has an address-space limit, but it cannot be *stated* as a size:
//! `vm_map_set_size_limit` refuses a limit below what the process already has
//! mapped, and `dosetrlimit` turns that refusal into `EINVAL`. A process that
//! links dyld has gigabytes of shared cache and reserved mappings before its own
//! code runs, so `setrlimit(RLIMIT_AS, 1 GiB)` — the absolute cap the other two
//! platforms set — is always refused here. What the kernel does accept is that
//! size *plus* the cap, which bounds the same thing from the other end: the
//! address space may grow by the cap and no further, and a mapping past it fails
//! with `ENOMEM` (`vm_map_enter` checks the map's size against the limit on every
//! entry, from macOS 12 onwards).
//!
//! [`mapped_address_space`] is what makes that statement possible, and it is the
//! only reason this module exists: every macOS this build supports takes the
//! limit stated from it.
//!
//! [`mapped_address_space`]: https://developer.apple.com/documentation/kernel/1502863-proc_pid_rusage

/// The address space this process has already mapped, in bytes.
///
/// `MACH_TASK_BASIC_INFO` reports `virtual_size`, which for a normal task is the
/// VM map's own size — the quantity `vm_map_set_size_limit` compares an
/// `RLIMIT_AS` against. Reading it is a query about this process, so it needs no
/// entitlement and no port beyond the task port the runtime already holds.
#[cfg(target_os = "macos")]
pub(super) fn mapped_address_space() -> Option<u64> {
    unsafe extern "C" {
        /// `mach_task_self_` from `<mach/mach_init.h>`: the calling task's own
        /// port. Declared here rather than taken from `libc`, whose copy is
        /// deprecated in favour of a whole Mach binding crate.
        static mach_task_self_: libc::mach_port_t;
    }

    // `mach_task_basic_info` begins with `virtual_size`, and the buffer is sized
    // from the count `libc` derives from that struct. The first word of it is
    // that field: every platform this ships to is little-endian.
    let words = (libc::MACH_TASK_BASIC_INFO_COUNT as usize).div_ceil(2);
    let mut buffer = [0u64; 8];
    assert!(words <= buffer.len(), "the info struct fits the buffer");
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    let read = unsafe {
        libc::task_info(
            mach_task_self_,
            libc::MACH_TASK_BASIC_INFO,
            buffer.as_mut_ptr().cast::<libc::integer_t>(),
            &mut count,
        )
    };
    // Zero is not a small process, it is a query that returned nothing.
    (read == 0 && buffer[0] > 0).then_some(buffer[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address-space limit is set against this number, so it has to be one:
    /// a failed query would put the limit below what is already mapped, and the
    /// kernel would refuse it — leaving the limits item without a memory bound.
    #[test]
    fn this_process_has_mapped_address_space() {
        let bytes = mapped_address_space().expect("task_info reports this process");
        assert!(bytes > 0, "a running process has a mapped address space");
    }
}
