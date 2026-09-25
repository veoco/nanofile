//! Windows confinement: a Job Object around the child itself.
//!
//! Windows has no `RLIMIT_AS` and no unprivileged equivalent of Landlock, so
//! the layer that *does* exist is the one that matters most here: a Job Object
//! can cap the process's committed memory, its CPU time and how many processes
//! it may hold, and it restricts the window station, the clipboard and handles
//! to other processes.
//!
//! The Job Object is created and assigned by the child, which is also the
//! process it governs (`AssignProcessToJobObject` accepts the caller). The
//! handle is deliberately never closed: `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
//! would terminate this process the moment the last handle went away, so the
//! handle lives as long as the process does and the kernel closes it on exit —
//! which is exactly when nothing should be left running in the job.
//!
//! What this does not do is confine the filesystem or the network. A restricted
//! token would strip the privileges a service account holds, but creating a
//! process with one means `CreateProcessAsUser` and a hand-rolled
//! `STARTUPINFO`/pipe plumbing for the protocol; on the evidence of the two
//! sandboxes this follows, an ACL- or token-based file boundary is its own
//! piece of work. Until then the level here is honestly `Partial`: memory, CPU
//! and process count are bounded, files and sockets are not.

use std::mem::size_of;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use super::Layers;

/// Most committed memory the child may use, in bytes.
///
/// The same gigabyte the Unix layer allows, for the same reason: a document is
/// at most 256 MiB decompressed and yields at most 8 MiB of text.
const MEMORY_LIMIT: usize = 1 << 30;

/// CPU seconds before the job terminates the process.
///
/// A Job Object measures this in 100-nanosecond units.
const CPU_LIMIT_SECONDS: i64 = 15;
const HUNDRED_NANOSECONDS: i64 = 10_000_000;

/// Apply the confinement a Job Object can give.
pub(super) fn confine() -> (Layers, Vec<String>) {
    let mut layers = Layers::default();
    let mut detail = Vec::new();

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        detail.push("job=failed".to_string());
        return (layers, detail);
    }

    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_PROCESS_TIME
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    limits.BasicLimitInformation.PerProcessUserTimeLimit = CPU_LIMIT_SECONDS * HUNDRED_NANOSECONDS;
    // One process: this one. A parser has no child to run, and a fork bomb has
    // nowhere to go.
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = MEMORY_LIMIT;

    let sized = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
    let set_limits = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            sized,
        )
    };
    if set_limits == 0 {
        detail.push("job=limits-failed".to_string());
        return (layers, detail);
    }
    layers.limits = true;
    detail.push(format!("job=memory{MEMORY_LIMIT},cpu{CPU_LIMIT_SECONDS}s"));

    // The window station, the clipboard, the desktop and handles to other
    // processes: none of it belongs to a document parser.
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_EXITWINDOWS,
    };
    let sized = size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32;
    let set_ui = unsafe {
        SetInformationJobObject(
            job,
            JobObjectBasicUIRestrictions,
            std::ptr::addr_of!(ui).cast(),
            sized,
        )
    };
    if set_ui == 0 {
        detail.push("job=ui-failed".to_string());
    }

    let assigned = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
    if assigned == 0 {
        detail.push("job=assign-failed".to_string());
        return (layers, detail);
    }
    layers.process = true;
    // The handle stays open on purpose: `KILL_ON_JOB_CLOSE` fires when it is
    // the last one, and the process is the only thing that should end here.
    detail.push("job=on".to_string());

    (layers, detail)
}
