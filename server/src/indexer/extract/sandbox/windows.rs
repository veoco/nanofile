//! Windows confinement: a Job Object around the child, a restricted token for it
//! to run under, an AppContainer around that, and the process mitigations the
//! kernel will take for it.
//!
//! Windows has no `RLIMIT_AS` and no unprivileged equivalent of Landlock, so the
//! layers are these three, and the parent applies all of them at creation — a
//! token, a job and a container are each a property of the process rather than
//! something a running one can adopt:
//!
//! * **The Job Object** (the parent, named as a creation attribute) caps the
//!   process's committed memory, its CPU time and how many processes it may
//!   hold, restricts the window station, the clipboard and handles to other
//!   processes, and — because `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is set and
//!   the handle stays here — takes whatever it started with it. The child reads
//!   the limits back rather than trusting that they were set. A child that held
//!   its own job handle, which is what this used to be, could have raised the
//!   limit it was bounded by.
//! * **The restricted token** (the parent, at creation) cannot be applied to a
//!   running process, so it is the parent that starts the child with one:
//!   `CreateProcessAsUser` with a restricted version of the parent's own token,
//!   which is the one case Windows allows without `SeAssignPrimaryToken`.
//!   Privileges are gone, the administrative SIDs are deny-only, and write
//!   access is checked against the restricting SIDs alone. The token is also put
//!   at **low integrity**, which is the one mechanism that bounds a write by
//!   itself: the mandatory check is a second, independent half of every access
//!   check, and it is why Chromium runs its renderers below medium.
//! * **The AppContainer** (the parent, at creation) is a process-creation
//!   attribute rather than a token: `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`
//!   with an AppContainer SID and *no capabilities* is what turns the child into
//!   a low-box process. Capabilities are what grant a container network access
//!   and access outside its own package, so an empty list is a process that
//!   cannot open a socket to anything and cannot read a file whose ACL names only
//!   its user. This is the configuration Chromium's own zero-capability sandbox
//!   uses, and the one the reference below documents.
//!
//! On top of those, the creation attributes also carry the kernel's own refusal
//! to let the child create processes, and the child asks for the process
//! mitigations it can live with: win32k lockdown, arbitrary code guard, no
//! extension points, no fonts, no remote or low-integrity images, the ASLR
//! family and strict handle checks. Each is read back with
//! `GetProcessMitigationPolicy` — including the child-process policy the parent
//! named — and the report counts what is there rather than what was asked for.
//!
//! The restricting SIDs are this process's own identity: the logon session, the
//! user, and the groups Windows grants the objects a process needs to attach to
//! (`INTERACTIVE`, `Authenticated Users`, `Users`, `Everyone`). They cannot be
//! dropped, because a restricted token is checked twice on every access and a
//! restricting list that does not cover the window station and desktop the child
//! inherits leaves it created and then dead inside the loader with
//! `STATUS_DLL_INIT_FAILED` (`0xC0000142`).
//!
//! Because the user's own SID is in that list, `WRITE_RESTRICTED` narrows writes
//! to what this user may write rather than to nothing. What closes that gap is
//! the container: an AppContainer access check requires an ACE for the package
//! SID — or for `ALL APPLICATION PACKAGES` — *in addition* to whatever the user
//! and group SIDs grant, so a file that only names the user stops being readable.
//! The one file that has to keep being readable is the worker's own image, which
//! a per-user install does not carry that ACE on, so the parent grants it to the
//! container's SID before the first launch (see [`grant_image_access`]).
//!
//! With the container in place, all four layers are the platform's own and the
//! level is `full`. Without it — a host that refuses the token, or the container,
//! or the launch — the child still runs, and reports the layers it has rather
//! than the ones that were asked for, which is what the parent's fallback logs.
//!
//! What no layer here bounds is the shape of the boundary itself. The container
//! reads the system tree it loads from (`Windows`, `Program Files` — the paths
//! `ALL APPLICATION PACKAGES` covers), which is the over-grant this
//! configuration has, and the reason the child reports `system=readable` beside
//! the per-user paths it was refused. It also writes inside its own profile
//! store, which is what `writes=own-store` says: bounded by the store rather
//! than denied, and the one resource this platform has no bound for at all (the
//! unix file-size limit has no equivalent here). Everything else is that same
//! dual-principal check rather than an open door: the registry it reads is the
//! keys carrying the same grant — system ones, not the user's — while its writes
//! are redirected to its own per-app store, and the IPC it reaches is over the
//! handles this process handed it. Those are the platform's own limits, and they
//! are the same shape as the macOS profile's grants.
//!
//! # References
//!
//! * Microsoft, *Launch an AppContainer*: the attribute, the empty capability
//!   list, and the requirement that the image be readable by the container.
//! * Microsoft, *Process Mitigation Policies* and *Job Objects*: the two APIs
//!   the layers here are built from, including `PROC_THREAD_ATTRIBUTE_JOB_LIST`
//!   and the child-process policy that is only accepted for a process in a job.
//! * Chromium's `sandbox/win/src/`: the same launch (`CreateProcessAsUser` with a
//!   plain restricted token plus `SECURITY_CAPABILITIES`), the same
//!   one-active-process job with the UI restrictions, and the note that the
//!   creation-time child-process policy exists *because* a job can be escaped.
//!
use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, LocalFree, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CopySid,
    CreateRestrictedToken, CreateWellKnownSid, DACL_SECURITY_INFORMATION, DISABLE_MAX_PRIVILEGE,
    GetLengthSid, GetTokenInformation, IsValidSid, LUA_TOKEN, NO_INHERITANCE, PSECURITY_DESCRIPTOR,
    PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenGroups,
    TokenIntegrityLevel, TokenIsAppContainer, TokenUser, WELL_KNOWN_SID_TYPE, WRITE_RESTRICTED,
    WinAuthenticatedUserSid, WinBuiltinUsersSid, WinInteractiveSid, WinWorldSid,
};
use windows_sys::Win32::Security::{EqualSid, GetAce, GetAclInformation};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
    GetProcessMitigationPolicy, INFINITE, InitializeProcThreadAttributeList, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ProcessChildProcessPolicy,
    ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy, ProcessFontDisablePolicy,
    ProcessImageLoadPolicy, ProcessStrictHandleCheckPolicy, ProcessSystemCallDisablePolicy,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, SetProcessMitigationPolicy, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::Layers;

/// Most committed memory the child may use, in bytes.
///
/// The same gigabyte the Unix layer allows, for the same reason: a document is
/// at most 256 MiB decompressed and yields at most 8 MiB of text.
const MEMORY_LIMIT: u64 = 1 << 30;

/// CPU seconds before the job terminates the process.
///
/// A Job Object measures this in 100-nanosecond units.
const CPU_LIMIT_SECONDS: i64 = 15;
const HUNDRED_NANOSECONDS: i64 = 10_000_000;

/// Flags that strip a token down to what a parser needs and nothing more.
///
/// `DISABLE_MAX_PRIVILEGE` removes every privilege, `LUA_TOKEN` makes the
/// administrative SIDs deny-only, and `WRITE_RESTRICTED` checks write access
/// against the restricting SIDs below rather than the token's groups.
const RESTRICTED_FLAGS: u32 = DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED;

/// `SE_GROUP_LOGON_ID` from `winnt.h`.
const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

// The window station, desktop, clipboard and handle restrictions this job used
// to carry are gone, and the reason is measured: a job's UI restrictions apply
// from the process's first instruction, and `JOB_OBJECT_UILIMIT_HANDLES` denies
// the loader the console it attaches on the way up — the child dies with
// `STATUS_DLL_INIT_FAILED` (`0xC0000142`) before it can print anything. They
// were applied *after* startup until the job moved to the parent, which is why
// this only appeared then. What covers the same ground now is stronger and does
// not have to be set on the job: win32k lockdown refuses every win32k system
// call outright, the container reaches no other process's objects, and the
// child-process policy refuses process creation.

/// Room for one SID, `SECURITY_MAX_SID_SIZE`.
const MAX_SID_BYTES: usize = 68;

/// Apply the confinement the child can give itself: the Job Object.
///
/// The other two layers are the parent's, but this process can still see one of
/// them: whether the token it was given is an AppContainer's is a property of the
/// token, read here rather than taken from the parent's word. A token that is not
/// one claims neither layer, and the measurement below clears a claim the token
/// does not back up.
pub(super) fn confine() -> (Layers, Vec<String>) {
    let mut layers = Layers::default();
    let mut detail = Vec::new();

    // The job is the parent's, so this process can only read it back: what the
    // limits *are* is a fact about the job, and a job nobody set limits on is
    // not a layer. That is the one difference from the arrangement this
    // replaced, where the process held its own limits and could raise them.
    match job_limits() {
        Ok((memory, cpu)) => {
            layers.limits = true;
            layers.process = true;
            detail.push(format!("job=memory{memory},cpu{cpu}s,parent"));
        }
        Err(reason) => detail.push(format!("job={reason}")),
    }

    if is_app_container() {
        layers.files = true;
        layers.network = true;
        detail.push("container=appcontainer".to_string());
    }

    detail.push(format!("il={}", integrity_level()));
    detail.push(format!("mitigations={}", harden_process()));

    // What a write reaches, which is the near edge of the files layer: inside
    // the container writes are redirected to its own store, outside it they are
    // the user's own access. Reported rather than assumed, because it is the
    // difference between a document filling its sandbox and a document filling
    // the disk the server runs on.
    if layers.files {
        detail.push(format!("writes={}", write_scope()));
    }

    (layers, detail)
}

/// The limits the parent's job puts on this process, read back from the job.
///
/// `QueryInformationJobObject` with a null job asks about the job this process
/// belongs to, which is the one the parent named at creation. A host that runs
/// this under a job of its own (a CI runner does) nests rather than replaces
/// it, so the immediate job is still the right one to ask.
fn job_limits() -> Result<(u64, i64), &'static str> {
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    let sized = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
    let read = unsafe {
        QueryInformationJobObject(
            null_mut(),
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of_mut!(info).cast(),
            sized,
            null_mut(),
        )
    };
    if read == 0 {
        return Err("unverified");
    }

    // The values, not only the flags: a job the host already put this process in
    // (a CI runner wraps everything it starts) has limits of its own, and one
    // whose limits are exactly these is the parent's. A job that is not there is
    // reported as absent rather than counted — the layer is a bound only if
    // there is one.
    let limits = info.BasicLimitInformation.LimitFlags;
    let ours = limits & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0
        && limits & JOB_OBJECT_LIMIT_PROCESS_TIME != 0
        && limits & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0
        && limits & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0
        && info.BasicLimitInformation.ActiveProcessLimit == 1
        && info.ProcessMemoryLimit as u64 == MEMORY_LIMIT
        && info.BasicLimitInformation.PerProcessUserTimeLimit
            == CPU_LIMIT_SECONDS * HUNDRED_NANOSECONDS;
    if !ours {
        return Err("limits-missing");
    }

    Ok((
        info.ProcessMemoryLimit as u64,
        info.BasicLimitInformation.PerProcessUserTimeLimit / HUNDRED_NANOSECONDS,
    ))
}

/// The integrity level this process's token carries, read back.
///
/// Read rather than assumed: the container path runs at low integrity by
/// construction, and the token-only path does not — which is exactly the
/// difference worth reporting, because integrity is the one mechanism that
/// bounds a *write* on its own.
fn integrity_level() -> &'static str {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return "unknown";
    }
    // The label is a SID inside the buffer this call fills, and the buffer is
    // word-aligned because it holds a pointer.
    let mut buffer: Vec<u64> = vec![0; 16];
    let mut needed = 0u32;
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast::<c_void>(),
            size_of_val(&buffer[..]) as u32,
            &mut needed,
        )
    };
    unsafe { CloseHandle(token) };
    if read == 0 {
        return "unknown";
    }

    let label = unsafe { &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
    let sid = label.Label.Sid.cast::<u8>();
    if sid.is_null() {
        return "unknown";
    }
    // A SID is revision, subauthority count, a six-byte authority, then the
    // subauthorities; the integrity level is the last of them.
    let (revision, count) = unsafe { (*sid, *sid.add(1)) };
    if revision != 1 || count == 0 {
        return "unknown";
    }
    let offset = 8 + 4 * (count as usize - 1);
    let value = unsafe { std::ptr::read_unaligned(sid.add(offset).cast::<u32>()) };
    match value {
        0 => "untrusted",
        0x1000 => "low",
        0x2000 => "medium",
        0x3000 => "high",
        0x4000 => "system",
        _ => "unknown",
    }
}

/// What a write from this process reaches.
///
/// A write the container redirects into its own store is bounded by the store;
/// a write that reaches the user's temporary directory is the user's own
/// access, and is the case the container exists to prevent. Measured by making
/// one and taking it back.
fn write_scope() -> &'static str {
    let probe = std::env::temp_dir().join("nanofile-extraction-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            "own-store"
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => "denied",
        Err(_) => "unmeasured",
    }
}

/// Ask the kernel for the process mitigations a document parser can live with.
///
/// Each is set and then *read back*, and the count in the report is the number
/// that took: a policy this Windows does not know fails the call, and one that
/// took is one an exploit has to get past. `SetProcessMitigationPolicy` is the
/// interface Chromium uses for the same set.
///
/// What is deliberately not here is code-integrity policy (CIG): it refuses
/// images that are not Microsoft-signed, and this worker is not signed.
fn harden_process() -> usize {
    // Every one of these is a DWORD whose first bit is the mitigation and whose
    // second, where it exists, asks for audit-only; the values are the fields
    // of the corresponding `PROCESS_MITIGATION_*_POLICY` in `winnt.h`.
    const ENABLE: u32 = 0x1;
    const IMAGE_LOAD_NO_REMOTE: u32 = 0x1;
    const IMAGE_LOAD_NO_LOW_LABEL: u32 = 0x2;
    /// Bottom-up randomization, forced relocation, and the larger range.
    const ASLR: u32 = 0x1 | 0x2 | 0x8;

    let policies: [(
        windows_sys::Win32::System::Threading::PROCESS_MITIGATION_POLICY,
        u32,
    ); 8] = [
        // No calls serviced by `win32k.sys` at all: the kernel surface a
        // document parser has no business reaching, and the one an exploit
        // would use to find a kernel bug to climb out through.
        (ProcessSystemCallDisablePolicy, ENABLE),
        // Arbitrary Code Guard: no new executable pages, so injected code has
        // nowhere to run. Nothing here generates code at runtime.
        (ProcessDynamicCodePolicy, ENABLE),
        // No AppInit DLLs, winsock providers, global hooks or legacy IMEs.
        (ProcessExtensionPointDisablePolicy, ENABLE),
        // No fonts: a parser that renders nothing has no use for the font
        // parser, which is a large attack surface of its own.
        (ProcessFontDisablePolicy, ENABLE),
        // Images from a network share, and images marked low integrity.
        (
            ProcessImageLoadPolicy,
            IMAGE_LOAD_NO_REMOTE | IMAGE_LOAD_NO_LOW_LABEL,
        ),
        (
            windows_sys::Win32::System::Threading::ProcessASLRPolicy,
            ASLR,
        ),
        // A bad handle reference raises instead of being ignored, which turns
        // some exploits into crashes.
        (ProcessStrictHandleCheckPolicy, ENABLE),
        // No process creation at all, on the process rather than on the job: a
        // job can be escaped by a process that knows how, which is why Chromium
        // carries this next to its own job. Set here rather than named at
        // creation, because nothing untrusted runs before this point anyway —
        // the document is not read until after `confine` returns.
        (ProcessChildProcessPolicy, ENABLE),
    ];

    let mut taken = 0;
    for (policy, flags) in policies {
        let applied = unsafe {
            SetProcessMitigationPolicy(
                policy,
                std::ptr::from_ref(&flags).cast::<c_void>(),
                size_of_val(&flags),
            )
        } != 0;
        if !applied {
            continue;
        }
        // Read back rather than trust the call: a policy the kernel accepted and
        // did not apply is not a mitigation, and the report counts what is.
        let mut observed = 0u32;
        let read = unsafe {
            GetProcessMitigationPolicy(
                GetCurrentProcess(),
                policy,
                std::ptr::addr_of_mut!(observed).cast::<c_void>(),
                size_of_val(&observed),
            )
        } != 0;
        if read && observed & flags == flags {
            taken += 1;
        }
    }

    taken
}

/// A child started by the parent, with pipes for the protocol.
pub(crate) struct Child {
    process: HANDLE,
    /// The job the child runs in, held here rather than by the child.
    ///
    /// `KILL_ON_JOB_CLOSE` fires when the last handle closes, so the parent
    /// holding it is what makes the bound outlive the child — and, because the
    /// child never gets a handle of its own, nothing running inside can raise a
    /// limit the way a process that created its own job could.
    job: HANDLE,
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

impl Child {
    /// The child's exit code, or `None` while it is still running.
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        match unsafe { WaitForSingleObject(self.process, 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0u32;
                if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(Some(code as i32))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    pub(crate) fn kill(&mut self) {
        unsafe { TerminateProcess(self.process, 1) };
    }

    /// Wait for the child to finish, however long that takes.
    pub(crate) fn wait(&mut self) {
        unsafe { WaitForSingleObject(self.process, INFINITE) };
    }

    /// Take the three pipe ends the protocol uses.
    pub(crate) fn take_pipes(&mut self) -> (Option<File>, Option<File>, Option<File>) {
        (self.stdin.take(), self.stdout.take(), self.stderr.take())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.process) };
        // Last handle to the job: if the child somehow outlived this, the job
        // takes it (and anything it started) with it.
        if !self.job.is_null() {
            unsafe { CloseHandle(self.job) };
        }
    }
}

/// The job the child is created in, with the limits it runs under.
///
/// The parent's rather than the child's, and named as a creation attribute so
/// there is no window in which the child runs without it. `None` when this host
/// will not make one: the child then reports no job and the limits layer is
/// absent, which is a level the policy decides about rather than something to
/// paper over.
fn parent_job() -> Option<HANDLE> {
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return None;
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
    limits.ProcessMemoryLimit = MEMORY_LIMIT as usize;

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
        unsafe { CloseHandle(job) };
        return None;
    }
    Some(job)
}

/// Start the child, in an AppContainer with a restricted token when the system
/// will make them.
///
/// The container is what bounds files and the network, and it is applied on top
/// of the token rather than instead of it: the token takes the privileges away,
/// the container makes every access check require the package SID as well as the
/// user's. A host that will not give one still gets the token, and a host that
/// will not give that either still gets a job-limited child rather than no child
/// at all.
pub(crate) fn spawn(program: &OsStr, args: &[OsString]) -> std::io::Result<Child> {
    let container = app_container(program);
    let attempts: &[bool] = if container.is_some() {
        &[true, false]
    } else {
        &[false]
    };
    let mut refused: Vec<String> = Vec::new();

    for &with_container in attempts {
        let Some(token) = restricted_token() else {
            // No token at all leaves the ordinary child, which still bounds
            // itself with a Job Object.
            break;
        };
        let mut attempt = args.to_vec();
        // The child cannot see the token it was created with, and whether
        // writes are restricted is worth saying out loud.
        attempt.push(OsString::from("--restricted"));
        let held = if with_container {
            container.as_ref()
        } else {
            None
        };
        match start(program, &attempt, Some(token), held) {
            Ok(child) => return Ok(child),
            Err(error) => {
                if with_container {
                    note_shortfall("launch-refused", Some(&error));
                }
                refused.push(format!(
                    "with {}: {error}",
                    if with_container {
                        "a container"
                    } else {
                        "a restricted token"
                    }
                ));
            }
        }
    }

    if let Some(error) = refused.first() {
        tracing::warn!(
            "extract-worker: the child was refused {error}; starting it with the default token"
        );
    }
    // Every failure is carried, not just the last one: the first says whether the
    // container or the token was refused, and the probe path has no subscriber
    // for the warning above to reach.
    start(program, args, None, None).map_err(|default| {
        std::io::Error::new(
            default.kind(),
            format!("{}; with the default token: {default}", refused.join("; ")),
        )
    })
}

/// Start the child with this process's own token and no container.
///
/// Only the diagnosis uses this. A child that was created and then died before
/// it reported can be failing because of its token, because of its container or
/// because of everything else about how it was started, and the rungs are told
/// apart the only way that cannot be argued with: start it again without them
/// and see.
pub(crate) fn spawn_unrestricted(program: &OsStr, args: &[OsString]) -> std::io::Result<Child> {
    start(program, args, None, None)
}

/// Start the child with the restricted token and no container.
///
/// The rung between the other two: a host whose container will not start still
/// gets the token, and a run that fails here names the token rather than the
/// container.
pub(crate) fn spawn_token_only(program: &OsStr, args: &[OsString]) -> std::io::Result<Child> {
    match restricted_token() {
        Some(token) => {
            let mut attempt = args.to_vec();
            attempt.push(OsString::from("--restricted"));
            start(program, &attempt, Some(token), None)
        }
        None => Err(std::io::Error::other(
            "the system refused a restricted token",
        )),
    }
}

/// Create the process, its pipes and the handle list that keeps inheritance to
/// those pipes.
///
/// The token is closed on every path, including the failing ones: this runs
/// once per document, and a leak here would be a leak in the server's own
/// handle table.
fn start(
    program: &OsStr,
    args: &[OsString],
    token: Option<HANDLE>,
    container: Option<&AppContainer>,
) -> std::io::Result<Child> {
    let started = start_with(program, args, token, container);
    if let Some(token) = token {
        unsafe { CloseHandle(token) };
    }
    started
}

fn start_with(
    program: &OsStr,
    args: &[OsString],
    token: Option<HANDLE>,
    container: Option<&AppContainer>,
) -> std::io::Result<Child> {
    let [
        (child_stdin, parent_stdin),
        (child_stdout, parent_stdout),
        (child_stderr, parent_stderr),
    ] = open_pipes()?;
    let inherited = [child_stdin, child_stdout, child_stderr];

    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child_stdin;
    startup.StartupInfo.hStdOutput = child_stdout;
    startup.StartupInfo.hStdError = child_stderr;
    // The job has to be named at creation rather than assigned afterwards:
    // between the two the child is running with nothing bounding it, and the
    // assignment itself is a call this process would have to make on a process
    // it may already have lost control of.
    let job = parent_job();
    // Only these three handles are inherited, the container is named in the same
    // list, and the job and the child-process policy travel with it. Without the
    // list, `bInheritHandles` would hand the child everything inheritable this
    // process holds — the server's own standard streams among it.
    let mut attributes = match AttributeList::new(
        &inherited,
        container.map(|held| &held.capabilities),
        job.as_ref(),
    ) {
        Ok(attributes) => attributes,
        Err(error) => {
            close_all(&inherited);
            close_all(&[parent_stdin, parent_stdout, parent_stderr]);
            if let Some(job) = job {
                unsafe { CloseHandle(job) };
            }
            return Err(error);
        }
    };
    startup.lpAttributeList = attributes.as_mut_ptr();

    let application: Vec<u16> = program.encode_wide().chain(std::iter::once(0)).collect();
    let mut command_line: Vec<u16> = command_line(program, args)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let environment = environment_block();
    // `null` here would inherit the *server's* working directory, which is a
    // path out of the deployment the child has no business knowing and the
    // container may not even be able to read. Unix sets the child's directory
    // to `/` for the same reason.
    let directory = current_directory();

    let flags = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
    let environment = environment.as_ptr().cast::<c_void>();
    let directory = directory
        .as_ref()
        .map_or(null(), |directory| directory.as_ptr());
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        match token {
            Some(token) => CreateProcessAsUserW(
                token,
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                flags,
                environment,
                directory,
                &startup.StartupInfo,
                &mut info,
            ),
            None => CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                flags,
                environment,
                directory,
                &startup.StartupInfo,
                &mut info,
            ),
        }
    };

    close_all(&inherited);
    if created == 0 {
        let error = std::io::Error::last_os_error();
        close_all(&[parent_stdin, parent_stdout, parent_stderr]);
        if let Some(job) = job {
            unsafe { CloseHandle(job) };
        }
        return Err(error);
    }

    unsafe { CloseHandle(info.hThread) };
    Ok(Child {
        process: info.hProcess,
        // The child is in the job from its first instruction; this end of it
        // lives as long as the child does.
        job: job.unwrap_or(null_mut()),
        stdin: Some(unsafe { File::from_raw_handle(parent_stdin as RawHandle) }),
        stdout: Some(unsafe { File::from_raw_handle(parent_stdout as RawHandle) }),
        stderr: Some(unsafe { File::from_raw_handle(parent_stderr as RawHandle) }),
    })
}

fn close_all(handles: &[HANDLE]) {
    for handle in handles {
        unsafe { CloseHandle(*handle) };
    }
}

/// The directory the child starts in, as a NUL-terminated wide string.
///
/// The system directory rather than the server's: it is part of the tree
/// `ALL APPLICATION PACKAGES` covers, so a container can always read it, and it
/// is a fact about Windows rather than about this deployment. `None` leaves
/// `CreateProcess` to inherit the caller's directory, which is the behaviour
/// this exists to avoid — the same choice unix makes in setting the child's
/// directory to `/`.
fn current_directory() -> Option<Vec<u16>> {
    let root = std::env::var_os("SystemRoot")?;
    let mut directory: Vec<u16> = root.encode_wide().collect();
    // A root that already ends in a separator would otherwise double it.
    if directory.last() == Some(&(b'\\' as u16)) {
        directory.pop();
    }
    if directory.is_empty() {
        return None;
    }
    directory.extend("\\System32".encode_utf16());
    directory.push(0);
    Some(directory)
}

/// The three pipe pairs the protocol needs, cleaned up if one cannot be made.
///
/// Every pair is `(child end, parent end)`: standard input runs into the child,
/// standard output and standard error run out of it.
fn open_pipes() -> std::io::Result<[(HANDLE, HANDLE); 3]> {
    let mut pipes: [(HANDLE, HANDLE); 3] = [(null_mut(), null_mut()); 3];
    for (slot, child_reads) in pipes.iter_mut().zip([true, false, false]) {
        match open_pipe(child_reads) {
            Ok(pipe) => *slot = pipe,
            Err(error) => {
                for (child, parent) in pipes {
                    close_all(&[child, parent]);
                }
                return Err(error);
            }
        }
    }
    Ok(pipes)
}

/// A pipe pair: the end the child holds, and the end the parent holds.
///
/// Both ends come back inheritable — `CreatePipe` takes no such parameter — and
/// only the parent's end is cleared. The child's end has to stay inheritable:
/// `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` accepts inheritable handles only, and
/// names one it cannot pass on by failing with `ERROR_INVALID_PARAMETER`.
///
/// `child_reads` is the one thing that decides which end is which.
fn open_pipe(child_reads: bool) -> std::io::Result<(HANDLE, HANDLE)> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut read: HANDLE = null_mut();
    let mut write: HANDLE = null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let (child, parent) = if child_reads {
        (read, write)
    } else {
        (write, read)
    };
    // The parent's end must not be inherited, or the child would hold its own
    // reader open and never see end of input.
    if unsafe {
        SetHandleInformation(
            parent,
            windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
            0,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        close_all(&[read, write]);
        return Err(error);
    }
    Ok((child, parent))
}

/// The process attributes the child is created with: the handles it may inherit,
/// the AppContainer it runs in, the job it runs under, and the kernel's own
/// refusal to let it create processes.
struct AttributeList {
    buffer: Vec<u8>,
}

impl AttributeList {
    fn new(
        handles: &[HANDLE; 3],
        capabilities: Option<&SECURITY_CAPABILITIES>,
        job: Option<&HANDLE>,
    ) -> std::io::Result<Self> {
        let count = 1 + usize::from(capabilities.is_some()) + usize::from(job.is_some()) as usize;
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count as u32, 0, &mut size) };
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, count as u32, 0, &mut size) } == 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut attributes = Self { buffer };
        // The buffer is a live attribute list from here on, so every failing
        // path leaves through the same cleanup a successful one does.
        let filled = unsafe {
            attributes
                .handles(handles)
                .and_then(|()| match capabilities {
                    Some(capabilities) => attributes.container(capabilities),
                    None => Ok(()),
                })
                .and_then(|()| match job {
                    Some(job) => attributes.job(job),
                    None => Ok(()),
                })
        };
        match filled {
            Ok(()) => Ok(attributes),
            Err(error) => {
                drop(attributes);
                Err(error)
            }
        }
    }

    /// The handles the child may inherit, and nothing else.
    ///
    /// # Safety
    ///
    /// `handles` must outlive the list: the attribute names it rather than
    /// copying it.
    unsafe fn handles(&mut self, handles: &[HANDLE; 3]) -> std::io::Result<()> {
        unsafe {
            self.set(
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast::<c_void>(),
                size_of::<[HANDLE; 3]>(),
            )
        }
    }

    /// The AppContainer the child runs in.
    ///
    /// # Safety
    ///
    /// `capabilities` must outlive the list, and the SID it names must outlive
    /// the process creation.
    unsafe fn container(&mut self, capabilities: &SECURITY_CAPABILITIES) -> std::io::Result<()> {
        unsafe {
            self.set(
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                std::ptr::from_ref(capabilities).cast::<c_void>(),
                size_of::<SECURITY_CAPABILITIES>(),
            )
        }
    }

    /// The job the child is created in.
    ///
    /// # Safety
    ///
    /// `job` must outlive the process creation: the attribute names an array of
    /// handles, of which it is the only entry.
    unsafe fn job(&mut self, job: &HANDLE) -> std::io::Result<()> {
        unsafe {
            self.set(
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                std::ptr::from_ref(job).cast::<c_void>(),
                size_of::<HANDLE>(),
            )
        }
    }

    /// Fill one attribute slot, which has to happen before the list is used and
    /// after the buffer exists.
    ///
    /// # Safety
    ///
    /// `value` must point at `size` readable bytes of the type the attribute
    /// names, and stay there until the list is dropped.
    unsafe fn set(
        &mut self,
        attribute: usize,
        value: *const c_void,
        size: usize,
    ) -> std::io::Result<()> {
        let updated = unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                attribute,
                value,
                size,
                null_mut(),
                null(),
            )
        };
        if updated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn as_mut_ptr(
        &mut self,
    ) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(
                self.buffer.as_mut_ptr().cast(),
            )
        };
    }
}

/// Why the child is not in an AppContainer, when the parent asked for one.
///
/// The probe prints it, as one whitespace-free token. Without it, losing the
/// container would show up only as a child that reports `files=open`, with the
/// reason on a `tracing` warning the probe path has no subscriber for.
static SHORTFALL: OnceLock<String> = OnceLock::new();

/// The reason the container was asked for and not applied, if there is one.
pub(crate) fn shortfall() -> Option<&'static str> {
    SHORTFALL.get().map(String::as_str)
}

/// Record the first reason, which is the strongest attempt's: later rungs fail
/// for their own reasons, and the one that matters is why the container is not
/// there.
fn note_shortfall(reason: &str, error: Option<&std::io::Error>) {
    let mut token = reason.to_string();
    if let Some(error) = error {
        // The report's fields are whitespace-separated, and an OS error message
        // is not.
        token.push('(');
        token.push_str(&error.to_string().replace(' ', "_"));
        token.push(')');
    }
    let _ = SHORTFALL.set(token);
}

/// The name the worker's AppContainer is known by.
///
/// The SID is a hash of it, so the same name gives the same SID on every run and
/// on every machine, and one container's profile is not another's.
const APP_CONTAINER_NAME: &str = "Nanofile.Extraction.Worker";

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`, which the profile call returns
/// when the profile is already there — the common case after the first run.
const PROFILE_EXISTS: i32 = 0x8007_00B7u32 as i32;

/// The AppContainer the child is created in.
struct AppContainer {
    /// What the process attribute points at: the package SID, and an empty
    /// capability list. The SID's memory belongs to the process, not to this
    /// value — it is derived once and outlives every launch — so this may only be
    /// read while it is alive on the caller's stack.
    capabilities: SECURITY_CAPABILITIES,
}

/// The AppContainer to start `program` in, or `None` when this host will not
/// give one.
///
/// The container is its SID plus the image grant, and both have to be in place
/// before the launch: a name no profile has established and an image the
/// container cannot read are each a launch that fails rather than a child that
/// runs confined.
fn app_container(program: &OsStr) -> Option<AppContainer> {
    let Some(sid) = app_container_sid() else {
        note_shortfall("sid-refused", None);
        return None;
    };
    // The image is the one file the child cannot run without, and it is the
    // parent's job to make it readable: inside the container the check that
    // matters is the package SID's, and a per-user install has no ACE for it.
    if !grant_image_access(program, sid) {
        // The grant records why it could not be made.
        return None;
    }
    Some(AppContainer {
        capabilities: SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            // No capabilities at all, which is the whole point: a capability is
            // what opens a socket or reaches outside the package, and a worker
            // that reads bytes from a pipe needs neither.
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        },
    })
}

/// This process's own SID for the worker container, creating its profile if it
/// does not have one yet.
///
/// The profile is what makes the name a container the system knows: it is
/// `%LOCALAPPDATA%\Packages\<name>` plus a per-app registry store, which is the
/// shape Microsoft's own launch sample and the two sandboxes this follows all
/// establish before they start a process in one. It also gives the container a
/// private directory of its own to write in, which is the only place in the
/// user's profile it can.
///
/// Made once and kept: the SID is a constant of the name, the grant recorded
/// against it is a property of the image, and freeing it would only mean
/// deriving the same value again. The allocation is one SID, for the life of the
/// process.
fn app_container_sid() -> Option<PSID> {
    static SID: OnceLock<usize> = OnceLock::new();
    let stored = *SID.get_or_init(|| {
        let name: Vec<u16> = OsStr::new(APP_CONTAINER_NAME)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sid: PSID = null_mut();
        let created = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                null(),
                0,
                &mut sid,
            )
        };
        if created == PROFILE_EXISTS {
            // Already made — by an earlier run, or by another copy of this
            // binary — and the call does not hand back the SID it did not make.
            // The name is the same, so the derived SID is the same value.
            let derived =
                unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            if derived < 0 {
                return 0;
            }
        } else if created < 0 {
            return 0;
        }
        if sid.is_null() {
            return 0;
        }
        sid as usize
    });
    (stored != 0).then(|| stored as PSID)
}

/// Whether `program` has been given to the container, granting it if not.
///
/// Once per process and per image: a grant is a change to the file's own ACL, and
/// doing it again for every document would add a second identical ACE each time.
/// A call for a *different* image is refused rather than silently reported as
/// granted, which leaves it to run under the restricted token alone.
///
/// A grant that cannot be made — an image under `Program Files`, where the user
/// may not rewrite the DACL, or one whose ACL cannot be read — leaves the child
/// with the token alone. That is less confinement than this host could give, and
/// the child's own measurement is what says so: nothing here claims a layer the
/// token did not hand over.
fn grant_image_access(program: &OsStr, sid: PSID) -> bool {
    static GRANTED: OnceLock<(OsString, bool)> = OnceLock::new();
    if let Some((image, granted)) = GRANTED.get() {
        return *granted && image == program;
    }
    match add_read_execute(program, sid) {
        Ok(()) => {
            let _ = GRANTED.set((program.to_os_string(), true));
            true
        }
        Err(error) => {
            note_shortfall("image-grant-refused", Some(&error));
            let _ = GRANTED.set((program.to_os_string(), false));
            false
        }
    }
}

/// Add read and execute for `sid` to `program`'s DACL, keeping every ACE it has.
///
/// The existing DACL is read and merged into: writing a fresh one would take the
/// file away from the user who owns it. `FILE_GENERIC_EXECUTE` is in the mask
/// because executing an image and traversing into a directory are the same bit,
/// and the loader needs both to map the file it was started from.
fn add_read_execute(program: &OsStr, sid: PSID) -> std::io::Result<()> {
    let path: Vec<u16> = program.encode_wide().chain(std::iter::once(0)).collect();

    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let read = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if read != 0 {
        return Err(std::io::Error::from_raw_os_error(read as i32));
    }
    // A NULL DACL grants everyone everything; merging our entry into it would
    // produce a DACL that grants the container and nobody else, taking the file
    // away from the user it belongs to. There is nothing to add to, so nothing
    // is added.
    if dacl.is_null() {
        unsafe { LocalFree(descriptor) };
        return Err(std::io::Error::other(
            "the image has no discretionary ACL to add to",
        ));
    }
    // A grant an earlier run made is read rather than written again: the merge
    // below is a write to a file another process may be running from, and the
    // question the caller asks is whether the container can read the image, not
    // whether this call changed anything.
    if dacl_already_grants(dacl, sid) {
        unsafe { LocalFree(descriptor) };
        return Ok(());
    }

    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            // A trustee of form SID carries the SID itself where a name would
            // otherwise be.
            ptstrName: sid.cast::<u16>(),
        },
    };
    let mut merged: *mut ACL = null_mut();
    let built = unsafe { SetEntriesInAclW(1, &entry, dacl, &mut merged) };
    if built != 0 {
        unsafe { LocalFree(descriptor) };
        return Err(std::io::Error::from_raw_os_error(built as i32));
    }

    let written = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            merged,
            null_mut(),
        )
    };
    unsafe {
        LocalFree(merged.cast());
        // Frees the descriptor and the DACL that came with it.
        LocalFree(descriptor);
    }
    if written != 0 {
        return Err(std::io::Error::from_raw_os_error(written as i32));
    }
    Ok(())
}

/// Whether `dacl` already gives `sid` the read and execute the container needs.
///
/// The second run of this process finds the ACE the first one left, and a plain
/// merge would depend on how the system treats a duplicate entry. Walking the
/// ACL answers the question the caller actually has.
fn dacl_already_grants(dacl: *const ACL, sid: PSID) -> bool {
    /// `ACCESS_ALLOWED_ACE_TYPE` from `winnt.h`, which is where this file keeps
    /// the few Windows constants `windows-sys` does not carry behind a feature.
    const ALLOWED: u8 = 0x00;
    const WANTED: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    let read = unsafe {
        GetAclInformation(
            dacl,
            std::ptr::addr_of_mut!(info).cast::<c_void>(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if read == 0 {
        return false;
    }

    for index in 0..info.AceCount {
        let mut ace: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            continue;
        }
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if allowed.Header.AceType != ALLOWED {
            continue;
        }
        // `SidStart` is the first four bytes of the SID that follows the header,
        // so its address is the SID's address.
        let entry = std::ptr::addr_of!(allowed.SidStart).cast::<c_void>() as PSID;
        if unsafe { EqualSid(entry, sid) } != 0 && allowed.Mask & WANTED == WANTED {
            return true;
        }
    }
    false
}

/// Whether this process's token is an AppContainer's.
///
/// The parent asks for the container through the process attributes rather than
/// through the command line, so the child reads its own token instead of being
/// told: a request the kernel did not grant must not become a claim, and this is
/// what the file and network layers are claimed on.
fn is_app_container() -> bool {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut inside = 0i32;
    let mut length = 0u32;
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenIsAppContainer,
            std::ptr::addr_of_mut!(inside).cast::<c_void>(),
            size_of::<i32>() as u32,
            &mut length,
        )
    };
    unsafe { CloseHandle(token) };
    read != 0 && inside != 0
}

/// A restricted version of this process's own token.
///
/// `None` when the system will not make one; the caller then starts an
/// ordinary child rather than not starting one at all.
fn restricted_token() -> Option<HANDLE> {
    let mut own: HANDLE = null_mut();
    let opened = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_QUERY,
            &mut own,
        )
    };
    if opened == 0 || own.is_null() {
        return None;
    }

    let mut sids = RestrictingSids::new();
    if !sids.push_logon_sid(own)
        || !sids.push_user(own)
        || !sids.push_well_known(WinInteractiveSid)
        || !sids.push_well_known(WinAuthenticatedUserSid)
        || !sids.push_well_known(WinBuiltinUsersSid)
        || !sids.push_well_known(WinWorldSid)
    {
        unsafe { CloseHandle(own) };
        return None;
    }

    let mut restricted: HANDLE = null_mut();
    let created = unsafe {
        CreateRestrictedToken(
            own,
            RESTRICTED_FLAGS,
            0,
            null(),
            0,
            null(),
            sids.len(),
            sids.as_ptr(),
            &mut restricted,
        )
    };
    unsafe { CloseHandle(own) };
    if created == 0 || restricted.is_null() {
        return None;
    }

    // No integrity label is set here, and that is a measured decision rather
    // than an omission: a low label applied to the token-only path — the one a
    // host falls back to when it will not start a container — leaves the child
    // unable to open the desktop its loader attaches to, and it dies with
    // `STATUS_DLL_INIT_FAILED` before it can print anything. Chromium runs its
    // renderers below medium because it gives them a window station of their
    // own; this child has the interactive one, and the container is where the
    // write bound comes from instead. The report says which level the token
    // ended up with ([`integrity_level`]), which is what the container's own
    // label shows up as.
    Some(restricted)
}

/// The restricting SIDs of a restricted token, and the storage they live in.
///
/// `SID_AND_ATTRIBUTES` points at a SID rather than holding it, so the token
/// call must be given memory that outlives the call.
struct RestrictingSids {
    storage: [u64; 64],
    entries: [SID_AND_ATTRIBUTES; 6],
    used: usize,
    count: usize,
}

impl RestrictingSids {
    fn new() -> Self {
        Self {
            storage: [0; 64],
            entries: [SID_AND_ATTRIBUTES {
                Sid: null_mut(),
                Attributes: 0,
            }; 6],
            used: 0,
            count: 0,
        }
    }

    /// Copy `source` into our own storage and add it to the list.
    fn push(&mut self, source: PSID) -> bool {
        if self.count == self.entries.len()
            || source.is_null()
            || unsafe { IsValidSid(source) } == 0
        {
            return false;
        }
        let length = unsafe { GetLengthSid(source) } as usize;
        if length == 0 || self.used + length > size_of_val(&self.storage) {
            return false;
        }
        let destination = unsafe {
            self.storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(self.used)
                .cast::<c_void>()
        };
        if unsafe { CopySid(length as u32, destination, source) } == 0 {
            return false;
        }
        self.entries[self.count] = SID_AND_ATTRIBUTES {
            Sid: destination,
            Attributes: 0,
        };
        self.used += length;
        self.count += 1;
        true
    }

    fn as_ptr(&self) -> *const SID_AND_ATTRIBUTES {
        self.entries.as_ptr()
    }

    fn len(&self) -> u32 {
        self.count as u32
    }

    /// The logon session this process runs in.
    fn push_logon_sid(&mut self, token: HANDLE) -> bool {
        let mut needed = 0u32;
        unsafe { GetTokenInformation(token, TokenGroups, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return false;
        }

        // `TOKEN_GROUPS` ends in a variable-length array, so it is read through
        // the struct's own field offset rather than a guessed one, and the
        // buffer is word-aligned because it holds pointers.
        let mut buffer: Vec<u64> = vec![0; (needed as usize).div_ceil(8)];
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenGroups,
                buffer.as_mut_ptr().cast::<c_void>(),
                needed,
                &mut needed,
            )
        };
        if read == 0 {
            return false;
        }

        let count = unsafe { *buffer.as_ptr().cast::<u32>() } as usize;
        let entries = unsafe {
            buffer
                .as_ptr()
                .cast::<u8>()
                .add(offset_of!(TOKEN_GROUPS, Groups))
                .cast::<SID_AND_ATTRIBUTES>()
        };
        let capacity = needed as usize / size_of::<SID_AND_ATTRIBUTES>();
        for index in 0..count.min(capacity) {
            let entry = unsafe { *entries.add(index) };
            if entry.Attributes & SE_GROUP_LOGON_ID == SE_GROUP_LOGON_ID {
                return self.push(entry.Sid);
            }
        }
        false
    }

    /// This user's own SID, which is the one the objects a process needs to
    /// attach to are granted to.
    ///
    /// A restricted token is checked twice on every access: once with its own
    /// SIDs and once with the restricting ones, and there the *restricting*
    /// check is the one that decides write access. The window station and the
    /// desktop a process inherits are granted to the user — that is how the
    /// parent is using them — so a restricting list without this SID leaves the
    /// child created and then dead inside the loader with
    /// `STATUS_DLL_INIT_FAILED` (`0xC0000142`), which is what Microsoft's
    /// KB 184802 attributes to a process that "does not have correct security
    /// access to the window station and desktop". Chromium's restricted tokens
    /// carry the current user for the same reason.
    fn push_user(&mut self, token: HANDLE) -> bool {
        let mut needed = 0u32;
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return false;
        }

        // `TOKEN_USER` holds one `SID_AND_ATTRIBUTES`, which points at a SID
        // inside the buffer, and the buffer is word-aligned because it holds a
        // pointer.
        let mut buffer: Vec<u64> = vec![0; (needed as usize).div_ceil(8)];
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast::<c_void>(),
                needed,
                &mut needed,
            )
        };
        if read == 0 {
            return false;
        }

        let user = unsafe {
            buffer
                .as_ptr()
                .cast::<u8>()
                .add(offset_of!(TOKEN_USER, User))
                .cast::<SID_AND_ATTRIBUTES>()
        };
        self.push(unsafe { (*user).Sid })
    }

    /// A well-known SID, by the identifier Windows gives it.
    ///
    /// The list is the one a process's own identity is made of — the session,
    /// the user, and the groups Windows grants the objects a process needs to
    /// attach to. Chromium's restricted tokens carry the same set, and for the
    /// same reason: the restricting check has to be able to satisfy the window
    /// station and desktop the child inherits.
    fn push_well_known(&mut self, kind: WELL_KNOWN_SID_TYPE) -> bool {
        let mut sid = [0u64; MAX_SID_BYTES / 8];
        let mut length = size_of_val(&sid) as u32;
        let created = unsafe {
            CreateWellKnownSid(
                kind,
                null_mut(),
                sid.as_mut_ptr().cast::<c_void>(),
                &mut length,
            )
        };
        created != 0 && self.push(sid.as_mut_ptr().cast::<c_void>())
    }
}

/// The command line `CreateProcess` is given, quoted the way it parses it.
fn command_line(program: &OsStr, args: &[OsString]) -> String {
    let mut line = quote(program);
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

/// Quote one argument by the rules `CommandLineToArgvW` reads back.
fn quote(argument: &OsStr) -> String {
    let text = argument.to_string_lossy();
    if !text.is_empty() && !text.contains([' ', '\t', '"']) {
        return text.into_owned();
    }

    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for character in text.chars() {
        match character {
            '\\' => {
                backslashes += 1;
                quoted.push('\\');
            }
            '"' => {
                // A quote has to be escaped, and so does every backslash that
                // precedes it (they would otherwise escape the escape).
                for _ in 0..=backslashes {
                    quoted.push('\\');
                }
                quoted.push('"');
                backslashes = 0;
            }
            other => {
                backslashes = 0;
                quoted.push(other);
            }
        }
    }
    // Trailing backslashes would escape the closing quote; doubling them keeps
    // them literal.
    for _ in 0..backslashes {
        quoted.push('\\');
    }
    quoted.push('"');
    quoted
}

/// The variables a Windows process needs before its own code runs.
///
/// A process started with an empty environment does not work here: `winsock`
/// fails to initialize without `SystemRoot`, the loader and the temporary
/// directory helpers read the others, and both Go and libuv put a list like this
/// back into a block that was cleared for the same reason (libuv: "Windows has a
/// few essential environment variables"). None of them is a secret.
///
/// `LOCALAPPDATA` is on the list because the AppContainer path reads it while the
/// process is being created — Microsoft's own process-container engine requires
/// it to be present, even with a nonsense value — and because a container without
/// a profile has no redirection to fall back on. Here it points at the user's own
/// directory, which the container cannot write to and has no business reading.
///
/// The identity variables a parser has no use for (`USERNAME`, `USERPROFILE`,
/// `LOGONSERVER`, …) are deliberately not on the list, and neither is anything
/// this deployment set.
const SYSTEM_VARIABLES: &[&str] = &[
    "LOCALAPPDATA",
    "Path",
    "SystemDrive",
    "SystemRoot",
    "TEMP",
    "TMP",
    "WINDIR",
];

/// The environment the child starts with: the system variables above, the two
/// switches the Rust runtime reads, and nothing else.
///
/// The server's environment holds the master secret, the storage keys and every
/// resolved path, so it is not passed on.
fn environment_block() -> Vec<u16> {
    let mut entries: Vec<(OsString, OsString)> = SYSTEM_VARIABLES
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(*name), value)))
        .collect();
    entries.push((OsString::from("RUST_BACKTRACE"), OsString::from("0")));
    entries.push((OsString::from("RUST_LIB_BACKTRACE"), OsString::from("0")));
    // Windows reads the block in the order it was written and expects the order
    // an environment is kept in: by name, case-insensitively.
    entries.sort_by(|(left, _), (right, _)| {
        left.to_string_lossy()
            .to_ascii_uppercase()
            .cmp(&right.to_string_lossy().to_ascii_uppercase())
    });

    let mut block: Vec<u16> = Vec::new();
    for (name, value) in entries {
        block.extend(name.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    // A block ends with an empty string, i.e. a second terminator.
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_without_spaces_is_left_alone() {
        assert_eq!(quote(OsStr::new("--selftest")), "--selftest");
    }

    #[test]
    fn a_path_with_spaces_is_quoted() {
        assert_eq!(
            quote(OsStr::new(r"C:\Program Files\nanofile.exe")),
            r#""C:\Program Files\nanofile.exe""#
        );
    }

    /// The closing quote must stay a quote: a trailing backslash doubles.
    #[test]
    fn trailing_backslashes_are_doubled() {
        assert_eq!(
            quote(OsStr::new(r"C:\dir with space\")),
            r#""C:\dir with space\\""#
        );
    }

    #[test]
    fn an_embedded_quote_is_escaped() {
        assert_eq!(quote(OsStr::new(r#"a "b" c"#)), r#""a \"b\" c""#);
    }

    #[test]
    fn the_command_line_joins_program_and_arguments() {
        let line = command_line(
            OsStr::new(r"C:\Program Files\nanofile.exe"),
            &[OsString::from("extract-worker")],
        );
        assert_eq!(line, r#""C:\Program Files\nanofile.exe" extract-worker"#);
    }

    /// The block carries what Windows needs to start a process and nothing this
    /// deployment set: the names are the whole allowlist.
    #[test]
    fn the_environment_block_holds_the_system_variables_and_nothing_else() {
        let block = environment_block();
        assert_eq!(block.last(), Some(&0));
        assert_eq!(block[block.len() - 2], 0, "the block ends with two zeros");
        let text = String::from_utf16_lossy(&block);
        assert!(text.contains("RUST_BACKTRACE=0"));
        assert!(text.contains("RUST_LIB_BACKTRACE=0"));

        let mut names: Vec<&str> = text
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.split('=').next().unwrap_or_default())
            .collect();
        for name in &names {
            assert!(
                SYSTEM_VARIABLES.contains(name)
                    || matches!(*name, "RUST_BACKTRACE" | "RUST_LIB_BACKTRACE"),
                "unexpected variable {name}"
            );
        }
        // Case-insensitive by name, which is the order Windows reads it in.
        let written = names.clone();
        names.sort_by_key(|name| name.to_ascii_uppercase());
        assert_eq!(written, names, "{written:?}");
    }
}
