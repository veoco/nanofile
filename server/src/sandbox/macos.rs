//! How macOS states the memory bound.
//!
//! Darwin cannot state `RLIMIT_AS` as an absolute size: `vm_map_set_size_limit`
//! refuses a limit below what is already mapped, and dyld maps gigabytes of shared
//! cache before the process's own code runs. What it accepts is that size *plus* the
//! cap, which bounds the same thing from the other end: the address space may grow by
//! the cap and no further, and a mapping past it fails with `ENOMEM`.
//!
//! [`mapped_address_space`] is the only reason this module exists.
//!
//! [`mapped_address_space`]: https://developer.apple.com/documentation/kernel/1502863-proc_pid_rusage

/// The address space this process has already mapped, in bytes.
///
/// `MACH_TASK_BASIC_INFO`'s `virtual_size` is what `vm_map_set_size_limit` compares an
/// `RLIMIT_AS` against, and reading it needs no entitlement.
#[cfg(target_os = "macos")]
pub(super) fn mapped_address_space() -> Option<u64> {
    unsafe extern "C" {
        /// `mach_task_self_` from `<mach/mach_init.h>`, declared here because
        /// `libc`'s copy is deprecated.
        static mach_task_self_: libc::mach_port_t;
    }

    // `mach_task_basic_info` begins with `virtual_size`, in the buffer's first word;
    // every platform this ships to is little-endian.
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

/// Whether the OS image the loader reads is still reachable, which it is.
///
/// A residual rather than a claim: the worker is dynamically linked, so its profile
/// grants read/map on the system library trees and shared cache — the files item means
/// "no path the server's user owns", not "no file at all". Reported through the same
/// token Windows uses, so the page says it rather than leaving `files=denied` to imply
/// more. A host answering `denied` is stricter, not broken.
pub(super) fn system_tree() -> &'static str {
    let candidates = [
        "/usr/lib/dyld",
        "/usr/lib/libSystem.B.dylib",
        "/System/Library/CoreServices/SystemVersion.plist",
    ];

    let mut refused = false;
    for candidate in candidates {
        match std::fs::File::open(candidate) {
            Ok(_) => return "readable",
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => refused = true,
            Err(_) => {}
        }
    }
    if refused { "denied" } else { "absent" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The limit is set against this number: a failed query would put it below what is
    /// already mapped, and the kernel would refuse it.
    #[test]
    fn this_process_has_mapped_address_space() {
        let bytes = mapped_address_space().expect("task_info reports this process");
        assert!(bytes > 0, "a running process has a mapped address space");
    }
}
