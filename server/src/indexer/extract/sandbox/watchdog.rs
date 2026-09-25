//! The memory bound macOS has to be told about, and the one it makes for itself.
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
//! only reason this module exists on a host whose kernel takes the limit.
//!
//! # The fallback
//!
//! Where the kernel will not take the limit at all — macOS 11 and older, whose VM
//! map has no size limit — the child watches its own physical footprint instead:
//! a thread reads the number the system's own memory accounting uses, the one
//! Activity Monitor shows, and ends the process when it passes the cap. That is
//! the weaker bound (a thread has to be scheduled, and a `fork` does not carry
//! it), which is why it is the fallback and not the plan.
//!
//! It is also the one bound a `fork` does not carry: the thread that watches is
//! not duplicated by `fork`, and the copy that a `process-fork`-allowed profile
//! permits therefore watches nothing. The address-space limit the newer kernels
//! take *is* inherited, so this is the ≤ macOS 11 case only. Re-arming the
//! watchdog from a `pthread_atfork` handler was considered and rejected: the
//! handler runs in a copy of a process that may have been multithreaded, and the
//! allocation a new thread needs can deadlock there — an unbounded copy is the
//! better failure of the two. `seatbelt`'s module docs carry the same note next
//! to the `fork=` fact the child reports.
//!
//! [`proc_pid_rusage`]: https://developer.apple.com/documentation/kernel/1502863-proc_pid_rusage

use std::time::Duration;

/// How often the watchdog reads the footprint.
///
/// A quarter of a second is long enough that the thread costs nothing next to a
/// parse, and short enough that the overshoot past the cap is a fraction of it.
/// A bomb allocating at gigabytes a second passes the cap by tens of megabytes
/// before it is stopped, which is what the parsers' own budgets are for.
const POLL: Duration = Duration::from_millis(250);

/// The address space this process has already mapped, in bytes.
///
/// `MACH_TASK_BASIC_INFO` reports `virtual_size`, which for a normal task is the
/// VM map's own size — the quantity `vm_map_set_size_limit` compares an
/// `RLIMIT_AS` against. Reading it is a query about this process, so it needs no
/// entitlement and no port beyond the task port the runtime already holds.
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

/// The process's physical footprint, as the system's own accounting reports it.
///
/// `RUSAGE_INFO_V0` is the oldest and therefore the smallest flavour: the kernel
/// fills the struct the flavour names, so asking for a later one would mean
/// describing fields this does not read.
fn footprint() -> Option<u64> {
    let mut info = RusageInfoV0::default();
    // The header types the buffer as `rusage_info_t *` — that is, `void **` —
    // but the kernel copies the struct into the address it is handed rather than
    // into a pointer read from it. Every caller, Apple's own included, passes the
    // struct's address rebased to that type and reads the struct afterwards.
    let buffer = std::ptr::addr_of_mut!(info).cast::<libc::rusage_info_t>();
    let read = unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V0, buffer) };
    if read != 0 {
        return None;
    }
    Some(info.ri_phys_footprint)
}

/// Watch this process's footprint and end it when it passes `cap`.
///
/// Returns the reason it is *not* watching when it could not start, so the
/// caller can report the memory bound as missing rather than as present.
pub(super) fn arm(cap: u64) -> Result<(), &'static str> {
    let first = footprint().ok_or("unreadable")?;
    // A footprint of zero is not a small process, it is a read that returned
    // nothing: an armed watchdog that can never fire is worse than none.
    if first == 0 {
        return Err("zero");
    }
    if over(first, cap) {
        stop(first, cap);
    }

    std::thread::Builder::new()
        .name("footprint".to_string())
        .stack_size(64 * 1024)
        .spawn(move || {
            loop {
                std::thread::sleep(POLL);
                if let Some(bytes) = footprint()
                    && over(bytes, cap)
                {
                    stop(bytes, cap);
                }
            }
        })
        .map(|_| ())
        .map_err(|_| "thread-failed")
}

/// Whether a footprint is past the cap.
///
/// Equal is not past: the cap is the largest footprint allowed, the same way the
/// address-space limit it stands in for is.
fn over(footprint: u64, cap: u64) -> bool {
    footprint > cap
}

/// Say why, then end the process without running anything else.
///
/// The exit code is the report — a parser being killed for its footprint has no
/// reply to write — and the parent turns it into the same verdict a document
/// that expands past the extraction budget gets.
fn stop(footprint: u64, cap: u64) -> ! {
    // Not `eprintln!`: a failed write to the parent's pipe would panic, and a
    // panic here would be reported as the wrong kind of death.
    let message = format!(
        "extract-worker: memory bound: footprint {footprint} bytes is over {cap}; stopping\n"
    );
    let _ = std::io::Write::write_all(&mut std::io::stderr(), message.as_bytes());
    unsafe { libc::_exit(super::EXIT_MEMORY_LIMIT) }
}

/// `rusage_info_v0` from `<sys/resource.h>`.
///
/// The field order is the header's, and the offsets asserted below are what keep
/// a mistyped one from silently reading a different field: the kernel writes
/// exactly `sizeof(rusage_info_v0)` bytes into this.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RusageInfoV0 {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    ri_proc_start_abstime: u64,
    ri_proc_exit_abstime: u64,
}

const _: () = assert!(std::mem::size_of::<RusageInfoV0>() == 96);
const _: () = assert!(std::mem::offset_of!(RusageInfoV0, ri_phys_footprint) == 72);

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap is the largest footprint allowed: a process sitting exactly at it
    /// is not over it.
    #[test]
    fn a_footprint_is_over_only_past_the_cap() {
        assert!(!over(1000, 1000));
        assert!(over(1001, 1000));
        assert!(!over(0, 1000));
    }

    /// Reading this process's own footprint works, and it is a live number: the
    /// watchdog is only worth arming if the read does.
    #[test]
    fn this_process_has_a_footprint() {
        let bytes = footprint().expect("proc_pid_rusage reports this process");
        assert!(bytes > 0, "a running process has a footprint: {bytes}");
        assert!(
            bytes < super::super::ADDRESS_SPACE_LIMIT,
            "this test process stays well under the cap: {bytes}"
        );
    }

    /// The address-space limit is set against this number, so it has to be one: a
    /// failed query would put the limit below what is already mapped, and the
    /// kernel would refuse it — leaving the level without a memory bound.
    #[test]
    fn this_process_has_mapped_address_space() {
        let bytes = mapped_address_space().expect("task_info reports this process");
        assert!(bytes > 0, "a running process has a mapped address space");
    }

    /// The struct is the header's, field for field.
    #[test]
    fn the_rusage_struct_is_the_headers() {
        assert_eq!(std::mem::size_of::<RusageInfoV0>(), 96);
        assert_eq!(std::mem::offset_of!(RusageInfoV0, ri_phys_footprint), 72);
        assert_eq!(std::mem::offset_of!(RusageInfoV0, ri_proc_exit_abstime), 88);
    }
}
