//! Windows confinement: a Job Object around the child, a restricted token for it
//! to run under, an AppContainer around that, and the process mitigations the
//! kernel will take for it.
//!
//! Windows has no `RLIMIT_AS` and no unprivileged Landlock, so these are the
//! layers, and the parent applies every one at creation: a token, a job and a
//! container are each a property of the process, not something a running one can
//! adopt.
//!
//! * **The Job Object** (the parent, named as a creation attribute) caps committed
//!   memory, CPU time and the process count, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
//!   on a handle the parent holds takes whatever the child started with it. The
//!   child reads the limits back instead of trusting them and holds no handle of
//!   its own, so nothing inside can raise them. It carries no window-station,
//!   clipboard or handle restrictions: those apply from the first instruction and
//!   `JOB_OBJECT_UILIMIT_HANDLES` kills the loader before it can print. There is no
//!   descriptor or file-size bound here at all.
//! * **The restricted token** (the parent, at creation) cannot be applied to a
//!   running process, so the parent starts the child with it: `CreateProcessAsUser`
//!   with a restricted version of its own token, the one case Windows allows
//!   without `SeAssignPrimaryToken`. Privileges are gone, the administrative SIDs
//!   are deny-only, and writes are checked against the restricting SIDs. No
//!   integrity label is set: low integrity is the container's own property, and the
//!   child reports the level it reads back (`il=`).
//! * **The AppContainer** (the parent, at creation) is a process-creation
//!   attribute: `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` with an AppContainer
//!   SID and *no capabilities*. Capabilities are what grant network access and
//!   access outside the package, so an empty list opens no socket and reads no file
//!   whose ACL names only the user. This is Chromium's zero-capability
//!   configuration.
//!
//! On top, the child sets the process mitigations it can live with — win32k
//! lockdown, arbitrary code guard, no extension points, no fonts, no remote or
//! low-integrity images, the ASLR family, strict handle checks, and the
//! child-process policy for every profile but media — and reads each back with
//! `GetProcessMitigationPolicy`. They are *runtime* policies, not creation
//! attributes, because the media profile must start its helper. Between creation
//! and `confine` the job's active-process limit is the only bound on child
//! creation, and nothing untrusted runs in that window.
//!
//! The restricting SIDs are this process's own identity: the logon session, the
//! user, and the groups Windows grants the objects a process needs to attach to
//! (`INTERACTIVE`, `Authenticated Users`, `Users`, `Everyone`). They cannot be
//! dropped: a restricted token is checked twice on every access, and a restricting
//! list without them leaves the child created and dead inside the loader with
//! `STATUS_DLL_INIT_FAILED` (`0xC0000142`).
//!
//! Because the user's own SID is in that list, `WRITE_RESTRICTED` narrows writes
//! to what this user may write rather than to nothing, and is close to a no-op on
//! its own. What bounds writes is the container and the integrity level: an
//! AppContainer check requires an ACE for the package SID — or for
//! `ALL APPLICATION PACKAGES` — *in addition* to what the user SIDs grant, so a
//! file naming only the user stops being readable. The worker's own image is the
//! one file that has to stay readable and a per-user install carries no such ACE,
//! so the parent grants it to the container before the first launch
//! ([`grant_image_access`]).
//!
//! With the container, all four items are the platform's own. Without it the child
//! still runs in the job, under the restricted token and with the mitigations, and
//! reports `files=open`: `limits` is what keeps the server up, and the container is
//! the only mechanism that could add the files and network items. Every launch
//! attempts it once; a host that refuses is not asked twice.
//!
//! # Why a host may end up without the container
//!
//! Creating one is a registry profile plus package-SID access on the worker's own
//! image, and a host sandbox that confines every command it runs — a development
//! or agent environment — refuses exactly those. The failure arrives as a child
//! that dies in its loader with no diagnosis, or as an ACL that cannot be written.
//! A launch therefore attempts the container first and falls back to the job
//! object, reporting the missing item.
//!
//! # The less privileged container
//!
//! Windows 11 lets a container opt out of `ALL APPLICATION PACKAGES` with
//! `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` `..._OPT_OUT`, and
//! `lpac=` says whether it took. Windows 10 answers with an error, so the attribute
//! is dropped and the plain container kept. The attribute is accepted at creation,
//! so a container the loader cannot start in fails *after* `CreateProcess` returns
//! success; the parent then stops asking and starts over in the plain container.
//!
//! The opt-out does not close the note below. It is the security attribute
//! `WIN://NOALLAPPPKG`, which makes the low-box check *ignore* `ALL APPLICATION
//! PACKAGES` without removing it from the token, and the process stays an
//! AppContainer. What keeps the system tree readable is another principal,
//! `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`), whose ACEs ship on the
//! system files a packaged process loads — a property of the Windows build rather
//! than a documented invariant, which is why `system=` is measured and why a host
//! answering `system=denied` is stricter rather than broken. Nor does the opt-out
//! cover the registry: an LPAC needs the `registryRead` capability to read the keys
//! a parser would want.
//!
//! What no protection here bounds is the shape of the boundary. The container reads
//! the system tree it loads from (`Windows`, `Program Files` — the trees whose ACLs
//! carry the packages a low-box process is checked against), reported as
//! `system=readable` beside the per-user paths it was refused. A write is reported
//! as measured — `writes=store` inside the container's redirected store,
//! `writes=user` outside it — and writes have no size bound here at all. The
//! registry it reads and the IPC it reaches are the same dual-principal check and
//! the handles this process handed it: the platform's own limits, the same shape
//! as the macOS profile's grants.
//!
//! # Where the media profile's helper may live
//!
//! The helper needs an explicit grant — on its own file and on the libraries
//! beside it, one file at a time, and nothing else — because a container's file
//! access is decided by ACEs naming its own package SID. The child reports
//! `helper=allowed` or `helper-grant-refused`.
//!
//! Granting only files is deliberate. Walking *up* and granting every directory
//! would be unnecessary: [`restricted_token`] keeps `SeChangeNotifyPrivilege`
//! (`DISABLE_MAX_PRIVILEGE` disables everything except it), so a caller holding it
//! is not checked for traverse at all. It is also ruinous: `SetNamedSecurityInfo`
//! "impose[s] the current inheritance model on the ACLs of all objects in the
//! hierarchy below the target object" (Microsoft, *Automatic Propagation of
//! Inheritable ACEs*), so one ACE on `C:\Users\<user>` rewrites the inheritance of
//! everything beneath it.
//!
//! A helper's stdio is the first suspect when it cannot run: `NUL` is a device a
//! container may refuse to open, and that refusal arrives from the same call as the
//! process creation. The streams are pipes — stdin closed here, stdout and stderr
//! drained — and the launch reports `parse=media-ok(<bytes>)`. A helper whose only
//! grant is `ALL APPLICATION PACKAGES` gets the opt-out given up for the media
//! profile rather than an environment problem reported ([`use_plain_container`]).
//!
//! # The two principals, and why the opt-out is the one that can go
//!
//! A low-box check is answered by an ACE for the process's package SID *or* for
//! `ALL APPLICATION PACKAGES`; the second is what the system tree carries, and the
//! shortcut [`dacl_grants_any_package`] takes it so a file the OS already granted
//! is not rewritten on every launch. The opt-out makes the check ignore it, so that
//! shortcut certifies a read only while the launch is not asking for the opt-out.
//! [`add_access`] therefore asks from the launch's own decision, and a file this
//! process may not re-DACL — a system-owned DLL, an image under `Program Files` —
//! is where the two meet: the opt-out is given up and the container kept, rather
//! than the child being handed an image it cannot read. The launch reports
//! `lpac=off`, the settings page carries the note, and the log says the media
//! profile gave it up.
//!
//! # References
//!
//! * Microsoft, *Launch an AppContainer*: the attribute, the empty capability
//!   list, and the requirement that the image be readable by the container.
//! * Microsoft, *Process Mitigation Policies* and *Job Objects*, including
//!   `PROC_THREAD_ATTRIBUTE_JOB_LIST` and the child-process policy that is only
//!   accepted for a process in a job.
//! * Chromium's `sandbox/win/src/`: the same launch, the same one-active-process
//!   job, and the note that the creation-time child-process policy exists
//!   *because* a job can be escaped.
use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, LocalFree, SetHandleInformation, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
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
    TokenIntegrityLevel, TokenIsAppContainer, TokenIsRestricted, TokenUser, WELL_KNOWN_SID_TYPE,
    WRITE_RESTRICTED, WinAuthenticatedUserSid, WinBuiltinAnyPackageSid, WinBuiltinUsersSid,
    WinInteractiveSid, WinWorldSid,
};
use windows_sys::Win32::Security::{EqualSid, GetAce, GetAclInformation};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateMutexW, CreateProcessAsUserW,
    CreateProcessW, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
    GetProcessMitigationPolicy, INFINITE, InitializeProcThreadAttributeList, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ProcessASLRPolicy, ProcessChildProcessPolicy, ProcessDynamicCodePolicy,
    ProcessExtensionPointDisablePolicy, ProcessFontDisablePolicy, ProcessImageLoadPolicy,
    ProcessStrictHandleCheckPolicy, ProcessSystemCallDisablePolicy, ReleaseMutex,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, SetProcessMitigationPolicy, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;

use super::{Grants, Profile, Protections};

/// Whether the less privileged container should be asked for.
static LPAC_WANTED: AtomicBool = AtomicBool::new(true);
/// Whether a launch actually asked for it (the platform may be too old).
static LPAC_ATTEMPTED: AtomicBool = AtomicBool::new(false);
/// Whether the last launch got it.
static LPAC_APPLIED: AtomicBool = AtomicBool::new(false);
/// The profiles that gave the opt-out up because the program they run could not
/// run under it.
///
/// One per profile, and only the media profile ever sets one: `ALL APPLICATION
/// PACKAGES` is the principal a helper's ACL is granted through, and the other two
/// profiles run no second program.
static PLAIN_CONTAINER: [AtomicBool; 3] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

/// Whether the child runs in a less privileged AppContainer.
///
/// Only the parent knows: the policy is a creation attribute and the child's token
/// looks the same either way. It is reported as a fact about the launch, not as a
/// verdict on files — the child's `system=` measurement says whether the system
/// tree is reachable, and under the opt-out it still is, through the user's SID.
pub(super) fn lpac() -> bool {
    LPAC_APPLIED.load(Ordering::Relaxed)
}

/// Whether a launch asked for the less privileged container.
pub(super) fn lpac_attempted() -> bool {
    LPAC_ATTEMPTED.load(Ordering::Relaxed)
}

/// Stop asking for it, for the rest of this process's life.
///
/// Called when a child created under the policy did not come back: the attribute is
/// accepted at creation, so a container the loader cannot start in fails *after*
/// `CreateProcess` returns success. The container is kept; only the opt-out goes.
pub(super) fn disable_lpac() {
    LPAC_WANTED.store(false, Ordering::Relaxed);
}

/// Whether `profile` gave the opt-out up for itself.
pub(super) fn plain_container(profile: Profile) -> bool {
    PLAIN_CONTAINER[slot(profile)].load(Ordering::Relaxed)
}

/// Stop asking for the opt-out where this profile's own program is concerned.
///
/// The container is kept. What asks for this is a helper that could not run under
/// the opt-out: `ALL APPLICATION PACKAGES` is the principal its ACL carries, and
/// the opt-out is what makes the low-box check ignore it.
pub(super) fn use_plain_container(profile: Profile) {
    PLAIN_CONTAINER[slot(profile)].store(true, Ordering::Relaxed);
}

/// Forget that a profile gave the opt-out up.
///
/// A different helper is a different measurement.
pub(super) fn forget_plain_container() {
    for held in &PLAIN_CONTAINER {
        held.store(false, Ordering::Relaxed);
    }
}

/// Which of the per-profile slots `profile` uses.
fn slot(profile: Profile) -> usize {
    match profile {
        Profile::Documents => 0,
        Profile::Images => 1,
        Profile::Media => 2,
    }
}

/// Whether a launch for `profile` should ask for the less privileged container.
fn want_lpac(profile: Profile) -> bool {
    LPAC_WANTED.load(Ordering::Relaxed) && !plain_container(profile)
}

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
/// `DISABLE_MAX_PRIVILEGE` leaves only `SeChangeNotifyPrivilege` (no traverse check
/// — see [`grant_helper_access`]), `LUA_TOKEN` makes the administrative SIDs
/// deny-only, and `WRITE_RESTRICTED` checks writes against the restricting SIDs,
/// which *are* this user's own identity: close to a no-op, not a write bound. The
/// write bound is the container and the integrity level.
const RESTRICTED_FLAGS: u32 = DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED;

/// `SE_GROUP_LOGON_ID` from `winnt.h`.
const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

/// `INHERIT_ONLY_ACE` from `winnt.h`: an ACE passed down, not applied here.
const INHERIT_ONLY_ACE: u8 = 0x08;

// No window-station, desktop, clipboard or handle restrictions on the job: UI
// restrictions apply from the first instruction, and `JOB_OBJECT_UILIMIT_HANDLES`
// denies the loader the console it attaches — `STATUS_DLL_INIT_FAILED`
// (`0xC0000142`) before it can print. Win32k lockdown, the container and the
// child-process policy cover the same ground without killing the child.

/// Room for one SID, `SECURITY_MAX_SID_SIZE`.
const MAX_SID_BYTES: usize = 68;

/// Apply the confinement the child can give itself, and report what it measured.
///
/// The other layers are the parent's, but the token's own properties are read here
/// rather than taken from the parent's word: a token that is not an AppContainer's
/// claims neither files nor network.
pub(super) fn confine(profile: Profile) -> (Protections, Vec<String>) {
    let mut layers = Protections::default();
    let mut detail = Vec::new();

    // The job is the parent's, so this process can only read it back: what the
    // limits *are* is a fact about the job, and a job nobody set limits on is not a
    // layer. The process count it must show is the profile's own.
    let expected_processes = if profile.runs_helper() { 2 } else { 1 };
    match job_limits(expected_processes) {
        Ok((memory, cpu)) => {
            layers.limits = true;
            layers.process = true;
            detail.push(format!(
                "job=memory{memory},cpu{cpu}s,processes{expected_processes},parent"
            ));
        }
        Err(reason) => detail.push(format!("job={reason}")),
    }

    if is_app_container() {
        layers.files = true;
        layers.network = true;
        detail.push("container=appcontainer".to_string());
    }

    detail.push(format!("il={}", integrity_level()));
    detail.push(format!(
        "token={}",
        if is_restricted_token() {
            "restricted"
        } else {
            "unrestricted"
        }
    ));
    detail.push(format!("mitigations={}", harden_process(profile)));

    // What a write reaches: inside the container, writes are redirected to its own
    // store; outside it, they are the user's own access. The difference between a
    // document filling its sandbox and one filling the server's disk.
    if layers.files {
        detail.push(format!("writes={}", write_scope()));
    }
    // What the media helper may read beyond its own image: the DLLs beside it, one
    // file at a time, because a dynamically linked helper loads them from there.
    if profile.runs_helper() && layers.files {
        detail.push("helper_scope=libs".to_string());
    }

    (layers, detail)
}

/// The limits the parent's job puts on this process, read back from the job.
///
/// `QueryInformationJobObject` with a null job asks about the job this process
/// belongs to. A host that runs this under a job of its own (a CI runner does) nests
/// rather than replaces it, so the immediate job is still the right one to ask.
fn job_limits(expected_processes: u32) -> Result<(u64, i64), &'static str> {
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

    // The values, not only the flags: a host job (a CI runner wraps everything it
    // starts) has limits of its own, and one whose limits are exactly these is the
    // parent's. No job is reported absent, not counted.
    let limits = info.BasicLimitInformation.LimitFlags;
    let ours = limits & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0
        && limits & JOB_OBJECT_LIMIT_PROCESS_TIME != 0
        && limits & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0
        && limits & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0
        // The crash policy is a bound like the others: a worker that dies on an
        // unhandled exception takes the job with it, which is what makes the exit
        // code the parent reads meaningful. Read back like the rest, because a job
        // somebody else nested would not have it.
        && limits & JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION != 0
        && info.BasicLimitInformation.ActiveProcessLimit == expected_processes
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
/// The container path runs at low integrity by construction and the token-only path
/// does not; integrity is the one mechanism that bounds a *write* on its own.
fn integrity_level() -> &'static str {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return "unknown";
    }
    // The label is a SID inside the buffer, which is word-aligned because it holds
    // a pointer.
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
/// A write the container redirects into its own store is bounded by the store; a
/// write that reaches the user's temporary directory is the user's own access, the
/// case the container exists to prevent. Both are measured by making a file and
/// taking it back.
///
/// The answer may *not* be inferred from the create having succeeded: whether it
/// landed in the container's store or the user's directory is a fact about the
/// path. The store lives under `%LOCALAPPDATA%\Packages\<package SID>\`, so a file
/// created anywhere else was created with the user's own authority.
fn write_scope() -> &'static str {
    let probe = std::env::temp_dir().join("nanofile-extraction-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            // The typed path is not where the file landed; only the resolved path
            // says whether the write was redirected. A host that will not answer
            // falls back to the typed path, which reads as the user's own.
            let resolved = std::fs::canonicalize(&probe).unwrap_or_else(|_| probe.clone());
            let _ = std::fs::remove_file(&probe);
            if is_container_store(&resolved) {
                "store"
            } else {
                "user"
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => "denied",
        Err(_) => "unmeasured",
    }
}

/// Whether a path is inside the AppContainer's own redirected store.
///
/// `%LOCALAPPDATA%\Packages` is where a packaged process's per-app state lives. The
/// path arrives as the kernel resolved it — `\\?\`-prefixed and verbatim — so the
/// prefix is removed first: a verbatim `C:` is not the same component as a plain
/// one. A path that does not resolve there was not redirected.
fn is_container_store(path: &Path) -> bool {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return false;
    };
    let verbatim = path.to_string_lossy();
    let stripped = verbatim
        .strip_prefix(r"\\?\")
        .map(Path::new)
        .unwrap_or(path);
    stripped.starts_with(Path::new(&local).join("Packages"))
}

/// Ask the kernel for the process mitigations a document parser can live with.
///
/// Each is set and then *read back*, and the count is the number that took: a policy
/// this Windows does not know fails the call. What is deliberately not here is
/// code-integrity policy (CIG), which refuses images that are not Microsoft-signed,
/// and this worker is not signed.
fn harden_process(profile: Profile) -> usize {
    // Each is a DWORD whose first bit enables the mitigation and whose second, where
    // it exists, asks for audit-only.
    const ENABLE: u32 = 0x1;
    const IMAGE_LOAD_NO_REMOTE: u32 = 0x1;
    const IMAGE_LOAD_NO_LOW_LABEL: u32 = 0x2;
    /// The four bits of `PROCESS_MITIGATION_ASLR_POLICY`: bottom-up
    /// randomization, forced relocation, high entropy, and disallow-stripped.
    ///
    /// Each is asked for on its own: `SetProcessMitigationPolicy` refuses the whole
    /// word when one bit cannot be honoured, so a build without forced relocation
    /// would take the other two down with it.
    const ASLR_BOTTOM_UP: u32 = 0x1;
    const ASLR_FORCE_RELOCATE: u32 = 0x2;
    const ASLR_HIGH_ENTROPY: u32 = 0x4;

    let policies: [(
        windows_sys::Win32::System::Threading::PROCESS_MITIGATION_POLICY,
        u32,
    ); 10] = [
        // No calls serviced by `win32k.sys`: the kernel surface a parser has no
        // business reaching, and the one an exploit would use to climb out.
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
        (ProcessASLRPolicy, ASLR_BOTTOM_UP),
        (ProcessASLRPolicy, ASLR_FORCE_RELOCATE),
        (ProcessASLRPolicy, ASLR_HIGH_ENTROPY),
        // A bad handle reference raises instead of being ignored, which turns
        // some exploits into crashes.
        (ProcessStrictHandleCheckPolicy, ENABLE),
        // No process creation, on the process rather than on the job: a job can be
        // escaped by a process that knows how, which is why Chromium carries this
        // next to its own job. Nothing untrusted runs before this point anyway.
        (ProcessChildProcessPolicy, ENABLE),
    ];

    let mut taken = 0;
    for (policy, flags) in policies {
        // The media profile starts the helper, so the no-child policy is not
        // installed for it. What bounds it is the job's process limit and the
        // container's file access.
        if policy == ProcessChildProcessPolicy && profile.runs_helper() {
            continue;
        }
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
        // Read back rather than trust the call.
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
    /// `KILL_ON_JOB_CLOSE` fires when the last handle closes, so the parent holding
    /// it makes the bound outlive the child, and nothing inside can raise a limit
    /// the way a process holding its own job could.
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

    /// End the child and everything it started.
    ///
    /// `TerminateProcess` on the direct child leaves the media helper running, so
    /// the job is what is ended: it holds the whole tree. A job that could not be
    /// created falls back to the direct child.
    pub(crate) fn kill_tree(&mut self) {
        if self.job.is_null() {
            self.kill();
        } else {
            unsafe { TerminateJobObject(self.job, 1) };
        }
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
        // Last handle to the job: the job takes the child with it.
        if !self.job.is_null() {
            unsafe { CloseHandle(self.job) };
        }
    }
}

/// The job the child is created in, with the limits it runs under.
///
/// Named as a creation attribute so there is no window in which the child runs
/// without it. `None` when this host will not make one: the limits layer is then
/// absent, which the policy decides about rather than papering over.
fn parent_job(active_process_limit: u32) -> Option<HANDLE> {
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
    // The worker, and for the media profile the helper: one slot, or exactly two.
    // A fork bomb has nowhere to go either way.
    limits.BasicLimitInformation.ActiveProcessLimit = active_process_limit;
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
/// The container is what bounds files and the network, applied on top of the token
/// rather than instead of it: the token takes the privileges away, the container
/// makes every access check require the package SID as well as the user's. This is
/// the strongest creation, and the one a request uses unless the probe found it
/// cannot start a child here. A token the system refuses is recorded and the child
/// still starts with the container and the job; its report says what it actually
/// has.
pub(crate) fn spawn(
    program: &OsStr,
    args: &[OsString],
    profile: Profile,
    grants: Grants<'_>,
) -> std::io::Result<Child> {
    // The opt-out is decided before the grants because the grants decide it: a file
    // whose only package grant is `ALL APPLICATION PACKAGES` is unreadable under the
    // opt-out, so the grant may hand it back rather than the child being given an
    // image it cannot read.
    let mut opt_out = want_lpac(profile);
    let container = app_container(program, &mut opt_out);
    // A worker that starts a helper gets two job slots and a container that can read
    // the helper and the source; one that starts nothing keeps a single slot.
    let active_process_limit = process_slots(profile);
    if profile.runs_helper() {
        grant_helper_access(grants, &mut opt_out);
    }

    let token = restricted_token();
    if token.is_none() {
        // Recorded, not swallowed: a child that runs without the token has less
        // confinement than this host asked for, and the reason has to reach the
        // log next to that fact.
        note_shortfall("token-refused", None);
    }
    // `want_lpac` carries the host decision and the helper's own: the less
    // privileged container was measured refusing the helper's `CreateProcess`
    // (`Access is denied`), and the principal it reads the helper through is the one
    // the opt-out ignores. `lpac=` says which container the launch got.
    start(
        program,
        args,
        token,
        container.as_ref(),
        active_process_limit,
        opt_out,
    )
}

/// Start the child with the restricted token, a job and no container.
///
/// The job is the resource bound and the token takes the privileges away, while the
/// container is the only mechanism that could bound files and the network. A host
/// that cannot create one — a sandbox of its own refusing a nested AppContainer,
/// common in development and agent environments — still gets what keeps the server
/// up, and the child reports `files=open`.
pub(crate) fn spawn_token_only(
    program: &OsStr,
    args: &[OsString],
    profile: Profile,
    grants: Grants<'_>,
) -> std::io::Result<Child> {
    // No container, so the opt-out is not this creation's decision: a grant good
    // only through `ALL APPLICATION PACKAGES` is good enough here.
    let mut opt_out = false;
    if profile.runs_helper() {
        grant_helper_access(grants, &mut opt_out);
    }
    let token = restricted_token();
    if token.is_none() {
        note_shortfall("token-refused", None);
    }
    start(program, args, token, None, process_slots(profile), false)
}

/// Start the child with this process's own token, a job and no container.
///
/// Still bounded: the job is named at creation as for every other creation, so the
/// memory, CPU and process caps hold whatever the token turned out to be. Some
/// hosts refuse a restricted token outright — the child is *created*, then dies in
/// its loader with `STATUS_DLL_INIT_FAILED` ([`crate::sandbox::worker`] decides
/// that from the child's silence) — and the alternative is not a better token but
/// the feature off on the whole machine. The bound is what keeps the server up.
pub(crate) fn spawn_unrestricted(
    program: &OsStr,
    args: &[OsString],
    profile: Profile,
    grants: Grants<'_>,
) -> std::io::Result<Child> {
    // No container in this creation, so the opt-out is not a decision it makes: a
    // grant that is only good through `ALL APPLICATION PACKAGES` is good enough
    // here.
    let mut opt_out = false;
    if profile.runs_helper() {
        grant_helper_access(grants, &mut opt_out);
    }
    start(program, args, None, None, process_slots(profile), false)
}

/// The worker, plus the helper the media profile starts.
fn process_slots(profile: Profile) -> u32 {
    if profile.runs_helper() { 2 } else { 1 }
}

/// Create the process, its pipes and the handle list that keeps inheritance to
/// those pipes.
///
/// The token is closed on every path, including the failing ones: this runs once per
/// request, and a leak here would be a leak in the server's handle table.
fn start(
    program: &OsStr,
    args: &[OsString],
    token: Option<HANDLE>,
    container: Option<&AppContainer>,
    active_process_limit: u32,
    want_lpac: bool,
) -> std::io::Result<Child> {
    let started = start_with(
        program,
        args,
        token,
        container,
        active_process_limit,
        want_lpac,
    );
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
    active_process_limit: u32,
    want_lpac: bool,
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
    // The job is named at creation rather than assigned afterwards: between the two
    // the child runs with nothing bounding it.
    let job = parent_job(active_process_limit);
    // Only these three handles are inherited; the container and the job travel in
    // the same list. Without it, `bInheritHandles` would hand the child everything
    // inheritable this process holds — the server's own standard streams among it.
    //
    // The less privileged container is asked for whenever there is a container.
    // Windows 10 refuses the attribute, and the list is rebuilt without it — a
    // fallback between two confined containers, never to an unconfined child.
    let mut lpac = want_lpac && container.is_some();
    let capabilities = container.map(|held| &held.capabilities);
    let built = AttributeList::new(
        &inherited,
        capabilities,
        job.as_ref(),
        lpac.then_some(PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT),
    )
    .or_else(|error| {
        if !lpac {
            return Err(error);
        }
        lpac = false;
        AttributeList::new(&inherited, capabilities, job.as_ref(), None)
    });
    let mut attributes = match built {
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
    // Recorded once the list is live: `true` only when the attribute actually made
    // it in. `attempted` is what the caller reads to decide whether a child that
    // never reported is worth starting again without the opt-out.
    LPAC_APPLIED.store(lpac, Ordering::Relaxed);
    LPAC_ATTEMPTED.store(lpac, Ordering::Relaxed);

    let application: Vec<u16> = program.encode_wide().chain(std::iter::once(0)).collect();
    let mut command_line: Vec<u16> = command_line(program, args)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let environment = environment_block();
    // `null` would inherit the *server's* working directory, a path the child has no
    // business knowing and the container may not be able to read. Unix sets the
    // child's directory to `/` for the same reason.
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
        // The child is in the job from its first instruction.
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
/// The system directory rather than the server's: `ALL APPLICATION PACKAGES` covers
/// it, so a container can always read it. `None` would let `CreateProcess` inherit
/// the caller's directory, the same choice unix makes in setting `/`.
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
/// Every pair is `(child end, parent end)`.
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
/// Both ends come back inheritable and only the parent's end is cleared. The
/// child's end has to stay inheritable: `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` accepts
/// inheritable handles only.
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
    // Otherwise the child holds its own reader open and never sees end of input.
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
/// the AppContainer it runs in, the job it runs under, and — for a host that has
/// one — the opt-out from `ALL APPLICATION PACKAGES`.
///
/// The child-process policy is not here: it is a runtime policy (see
/// [`harden_process`]), because the media profile has to start its helper.
///
/// The buffer is held as words because `PROC_THREAD_ATTRIBUTE_LIST` stores pointers,
/// and `Vec<u8>` would only be aligned when the allocator happened to hand back an
/// aligned block. `Vec<u64>` is the alignment the structure needs on every
/// architecture this ships to.
struct AttributeList {
    buffer: Vec<u64>,
}

impl AttributeList {
    fn new(
        handles: &[HANDLE; 3],
        capabilities: Option<&SECURITY_CAPABILITIES>,
        job: Option<&HANDLE>,
        container_policy: Option<u32>,
    ) -> std::io::Result<Self> {
        let count = 1
            + usize::from(capabilities.is_some())
            + usize::from(job.is_some())
            + usize::from(container_policy.is_some());
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count as u32, 0, &mut size) };
        let mut buffer = vec![0u64; size.div_ceil(size_of::<u64>())];
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
                .and_then(|()| match container_policy {
                    Some(policy) => attributes.container_policy(policy),
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

    /// Ask for the less privileged container, which opts out of
    /// `ALL APPLICATION PACKAGES`.
    ///
    /// Windows 10 refuses the attribute, which is how support is detected without
    /// reading a version.
    fn container_policy(&mut self, policy: u32) -> std::io::Result<()> {
        unsafe {
            self.set(
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
                std::ptr::from_ref(&policy).cast::<c_void>(),
                size_of::<u32>(),
            )
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
/// The probe prints it as one whitespace-free token; without it, losing the
/// container would show up only as `files=open`, with the reason on a `tracing`
/// warning the probe path has no subscriber for.
///
/// Every reason of one launch, in order, rather than the first ever: a missing
/// profile store, an ACL replaced by an in-place upgrade and a refused token are
/// not interchangeable. [`clear_shortfall`] scopes it to a single launch.
static SHORTFALL: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// The reasons the last launch recorded, joined into one whitespace-free token.
pub(crate) fn shortfall() -> Option<String> {
    let reasons = SHORTFALL
        .get()
        .map(|slot| {
            slot.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })
        .unwrap_or_default();
    (!reasons.is_empty()).then(|| reasons.join(";"))
}

/// Forget the previous launch's reasons, so the next one reports its own.
///
/// Called once per *launch sequence*, not once per rung: the strongest creation's
/// reason is the one an operator needs, and clearing between rungs would leave the
/// report naming the weakest rung's failure, or none.
pub(super) fn clear_shortfall() {
    let slot = SHORTFALL.get_or_init(|| Mutex::new(Vec::new()));
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// Record one reason this launch gave for not applying the container.
///
/// Deduplicated: the same reason can arrive from more than one attempt.
fn note_shortfall(reason: &str, error: Option<&std::io::Error>) {
    let mut token = reason.to_string();
    if let Some(error) = error {
        // The report's fields are whitespace-separated, and an OS error message
        // is not.
        token.push('(');
        token.push_str(&error.to_string().replace(' ', "_"));
        token.push(')');
    }
    let slot = SHORTFALL.get_or_init(|| Mutex::new(Vec::new()));
    let mut reasons = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !reasons.contains(&token) {
        reasons.push(token);
    }
}

/// The name the worker's AppContainer is known by.
///
/// The SID is a hash of it, so the same name gives the same SID everywhere.
const APP_CONTAINER_NAME: &str = "Nanofile.Extraction.Worker";

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`: the profile is already there.
const PROFILE_EXISTS: i32 = 0x8007_00B7u32 as i32;

/// The AppContainer the child is created in.
struct AppContainer {
    /// What the process attribute points at: the package SID and an empty
    /// capability list. The SID is derived once and outlives every launch, so this
    /// may only be read while it is alive on the caller's stack.
    capabilities: SECURITY_CAPABILITIES,
}

/// The AppContainer to start `program` in, or `None` when this host will not
/// give one.
///
/// The SID and the image grant both have to be in place before the launch: a name
/// no profile has established and an image the container cannot read are each a
/// launch that fails rather than a child that runs confined.
///
/// `opt_out` is the caller's decision, which this call may have to give up: an image
/// whose only package grant is `ALL APPLICATION PACKAGES` is unreadable under the
/// opt-out, and the opt-out is the half that can go.
fn app_container(program: &OsStr, opt_out: &mut bool) -> Option<AppContainer> {
    let Some(sid) = app_container_sid() else {
        note_shortfall("sid-refused", None);
        return None;
    };
    // The image is the one file the child cannot run without, and it is the
    // parent's job to make it readable: inside the container the check that
    // matters is the package SID's, and a per-user install has no ACE for it.
    if !grant_image_access(program, sid, opt_out) {
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
/// The profile makes the name a container the system knows: `%LOCALAPPDATA%\Packages\`
/// `<name>` plus a per-app registry store, and it gives the container the only
/// directory of its own it may write in.
///
/// Made once and kept: the SID is a constant of the name, and the allocation is one
/// SID for the life of the process. A *failure* is not kept: both calls fail for a
/// reason of the moment, and a cached failure would leave every later launch without
/// a container, which the page reads as a host that cannot confine anything. Asking
/// again costs one call, and the retry is what keeps a lost race between the
/// processes that create the same profile from reading as "cannot confine". The
/// pause is paid only on a path that is already failing.
fn app_container_sid() -> Option<PSID> {
    static SID: OnceLock<Mutex<Option<usize>>> = OnceLock::new();
    let slot = SID.get_or_init(|| Mutex::new(None));
    let mut cached = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(stored) = *cached {
        return Some(stored as PSID);
    }

    let name: Vec<u16> = OsStr::new(APP_CONTAINER_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let sid = ensure_profile(&name)?;
    *cached = Some(sid as usize);
    Some(sid)
}

/// Create or find the worker's AppContainer profile, returning its SID, under a
/// cross-process lock that keeps the operation from racing itself.
///
/// `CreateAppContainerProfile` is a create-or-find several processes call for the
/// same name — the tray and the service, each probe, the first worker. Without the
/// lock the losers can see the winner's store while it is half written: the create
/// answers with a transient error, or the derive after `ERROR_ALREADY_EXISTS` finds
/// nothing. Either way no SID comes back, the child is launched in a container with
/// no profile, `CreateProcess` returns `ERROR_FILE_NOT_FOUND`, the loader dies with
/// `STATUS_DLL_INIT_FAILED`, and the probe reports `sid-refused`. The lock makes one
/// process finish the create before any other opens the store.
///
/// The create comes first so that a `PROFILE_EXISTS` under the lock always means a
/// finished profile, never one still being written. Deriving first would be cheaper
/// but is documented only to take a name, so whether it fails on a store with no
/// profile is not something this code may assume.
///
/// A *failure* is not kept: a cached one would leave every later launch without a
/// container, and the pause above is paid only on a path that is already failing.
fn ensure_profile(name: &[u16]) -> Option<PSID> {
    /// Attempts before the host is believed, and the pause between them.
    const ATTEMPTS: usize = 3;
    const PAUSE: Duration = Duration::from_millis(50);

    // Serialise the create across processes; released on every exit path.
    let _lock = ProfileLock::acquire()?;

    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(PAUSE);
        }
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
            // The name is the same, so the derived SID is the same value, and
            // under the lock the profile it names is finished.
            let derived =
                unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            if derived >= 0 && !sid.is_null() {
                return Some(sid);
            }
        } else if created >= 0 && !sid.is_null() {
            return Some(sid);
        }
    }
    None
}

/// A cross-process mutex held only for the duration of [`ensure_profile`].
///
/// Its one job is to stop two processes from running `CreateAppContainerProfile`
/// for the same name at once. A `Global\` name covers a service in another session;
/// a host that refuses the global namespace falls back to `Local\`, which still
/// serialises every collision it can have. Released and closed on drop, and an
/// abandoned mutex (a previous owner crashed mid-create) is treated as acquired
/// because the operation is idempotent.
struct ProfileLock(HANDLE);

impl ProfileLock {
    /// How long to wait for the lock before giving it up.
    ///
    /// The create is a registry write measured in milliseconds, so a wait reaching
    /// this bound means a wedged owner (`WAIT_ABANDONED` covers only a crash).
    /// Proceeding lock-free is safer: the create-or-find is idempotent and
    /// `ensure_profile` retries, so one stuck process cannot block every spawn.
    const LOCK_TIMEOUT_MS: u32 = 2000;

    fn acquire() -> Option<Self> {
        // The store is per user; `Global\` also reaches a service in another
        // session.
        static NAMES: &[&str] = &["Global\\nanofile-ac-profile", "Local\\nanofile-ac-profile"];
        for name in NAMES {
            let wide: Vec<u16> = OsStr::new(name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let handle = unsafe { CreateMutexW(null(), 0, wide.as_ptr()) };
            if handle.is_null() {
                // Refused (e.g. `Global\` without the privilege): try the next name.
                // Both refused means no lock — the lock-free path, not no
                // confinement.
                continue;
            }
            let waited = unsafe { WaitForSingleObject(handle, Self::LOCK_TIMEOUT_MS) };
            match waited {
                // Abandoned by a crashed owner; safe because the create is
                // idempotent.
                WAIT_OBJECT_0 | WAIT_ABANDONED => return Some(ProfileLock(handle)),
                // A wedged owner or any error: drop it and try the next name.
                _ => unsafe {
                    CloseHandle(handle);
                },
            }
        }
        None
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        // Release before close, or the next acquirer sees an abandoned mutex.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

/// Give the container the read access the media profile's grants need.
///
/// The worker's own image is granted by [`grant_image_access`]. The media profile
/// starts a *second* program and reads one file the parent wrote, and neither
/// carries an ACE for the package SID: the helper lives wherever it was installed,
/// and the scratch source is in the server's temporary directory. Both are granted
/// here — read, plus execute for the helper, never write.
///
/// Every grant is written on the object itself, never on a directory above it: a
/// DACL write on a directory re-imposes inheritance on everything under it (the
/// module docs have both reasons). A grant that cannot be made is recorded as a
/// shortfall and left to fail loudly.
///
/// `opt_out` may have to be given up here: a helper whose only grant is
/// `ALL APPLICATION PACKAGES` is unreadable under it (see [`add_access`]).
fn grant_helper_access(grants: Grants<'_>, opt_out: &mut bool) {
    let Some(sid) = app_container_sid() else {
        return;
    };
    if let Some(helper) = grants.helper {
        // Written on a file, never a directory: an ACE on a directory re-imposes
        // inheritance on everything beneath it. A path that is not a regular file
        // cannot be confined, and fails closed through `parse=`.
        if !helper.is_file() {
            let error = std::io::Error::other("the helper is not a regular file");
            note_shortfall("helper-grant-refused", Some(&error));
        } else {
            if let Err(error) = add_access(
                helper.as_os_str(),
                sid,
                FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                opt_out,
            ) {
                note_shortfall("helper-grant-refused", Some(&error));
            }
            // A dynamically linked helper loads libraries from beside its image, and
            // each needs the same grant. An unlistable directory skips only this.
            if let Some(directory) = helper.parent()
                && let Ok(entries) = std::fs::read_dir(directory)
            {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_library = path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case(OsStr::new("dll")));
                    if is_library {
                        let _ = add_access(
                            path.as_os_str(),
                            sid,
                            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                            opt_out,
                        );
                    }
                }
            }
        }
    }
    if let Some(source) = grants.source {
        if !source.is_file() {
            let error = std::io::Error::other("the source is not a regular file");
            note_shortfall("source-grant-refused", Some(&error));
        } else if let Err(error) = add_access(source.as_os_str(), sid, FILE_GENERIC_READ, opt_out) {
            note_shortfall("source-grant-refused", Some(&error));
        }
    }
}

/// Whether `program` has been given to the container, granting it if not.
///
/// Asked on every launch rather than remembered: a grant is a change to the file's
/// own ACL, and a cached answer cannot know the ACL still carries it. An in-place
/// upgrade replaces the image with a fresh ACL, and a path-keyed cache would keep
/// saying yes until the kernel refused the container. Asking again costs a metadata
/// read, not a write: [`add_access`] returns without touching the DACL when the ACE
/// is already there.
///
/// A grant that cannot be made — an image under `Program Files`, where the user may
/// not rewrite the DACL — leaves the child with the token alone, and the child's own
/// measurement says so.
fn grant_image_access(program: &OsStr, sid: PSID, opt_out: &mut bool) -> bool {
    match add_read_execute(program, sid, opt_out) {
        Ok(()) => true,
        Err(error) => {
            note_shortfall("image-grant-refused", Some(&error));
            false
        }
    }
}

/// Add read and execute for `sid` to `program`'s DACL, keeping every ACE it has.
///
/// The existing DACL is merged into, because a fresh one would take the file away
/// from its owner. `FILE_GENERIC_EXECUTE` is in the mask because executing an image
/// and traversing into a directory are the same bit, and the loader needs both.
fn add_read_execute(program: &OsStr, sid: PSID, opt_out: &mut bool) -> std::io::Result<()> {
    add_access(
        program,
        sid,
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        opt_out,
    )
}

/// Add `mask` for `sid` to `path`'s DACL, keeping every ACE it already has.
///
/// The two SIDs a low-box check can be answered by are not interchangeable:
/// `ALL APPLICATION PACKAGES` is ignored under the opt-out, so an ACE for it
/// certifies a read only while the launch is *not* asking for the opt-out. `opt_out`
/// is that decision.
///
/// A file whose ACL cannot be rewritten — a system-owned DLL, an image under
/// `Program Files` — is where the caller chooses: the opt-out is given up (the
/// container stays) rather than the file reported ungranted, and the launch says
/// `lpac=off`.
fn add_access(path: &OsStr, sid: PSID, mask: u32, opt_out: &mut bool) -> std::io::Result<()> {
    let path: Vec<u16> = path.encode_wide().chain(std::iter::once(0)).collect();

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
    // A NULL DACL grants everyone everything; merging into it would grant the
    // container and nobody else. Nothing to add to, so nothing is added.
    if dacl.is_null() {
        unsafe { LocalFree(descriptor) };
        return Err(std::io::Error::other(
            "the image has no discretionary ACL to add to",
        ));
    }
    // Asked as the launch's own decision: under the opt-out only the package's ACE
    // answers, without it either principal does. A grant an earlier launch made is
    // therefore not written again — the merge is a write to a file another process
    // may be running from.
    if already_granted(dacl, sid, mask, !*opt_out) {
        unsafe { LocalFree(descriptor) };
        return Ok(());
    }
    let written = write_access(&path, sid, mask, dacl);
    if written.is_ok() || !*opt_out {
        unsafe { LocalFree(descriptor) };
        return written;
    }
    // Not this process's file to rewrite, and the launch needed the package's own
    // ACE. When the principal the opt-out ignores still answers, the opt-out is
    // given up rather than the read; with neither grant it is the caller's
    // shortfall.
    let through_any_package = already_granted(dacl, sid, mask, true);
    if through_any_package {
        *opt_out = false;
    }
    unsafe { LocalFree(descriptor) };
    if through_any_package { Ok(()) } else { written }
}

/// Whether `dacl` already gives the container the access this launch needs.
///
/// The package's own ACE answers in every case; `ALL APPLICATION PACKAGES` only when
/// the low-box check will consult it, which is why `allow_any_package` comes from
/// the launch's own decision.
fn already_granted(dacl: *const ACL, sid: PSID, mask: u32, allow_any_package: bool) -> bool {
    dacl_already_grants(dacl, sid, mask)
        || (allow_any_package && dacl_grants_any_package(dacl, mask))
}

/// Merge one ACE for `sid` into `dacl` and write the result onto `path`.
///
/// The write is what a system file may refuse; the caller decides what that means.
fn write_access(path: &[u16], sid: PSID, mask: u32, dacl: *const ACL) -> std::io::Result<()> {
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
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
    unsafe { LocalFree(merged.cast()) };
    if written != 0 {
        return Err(std::io::Error::from_raw_os_error(written as i32));
    }
    Ok(())
}

/// Whether `dacl` already gives `sid` every bit of `wanted`.
///
/// The second run of this process finds the ACE the first left, and a plain merge
/// would depend on how the system treats a duplicate entry. Walking the ACL answers
/// the caller's question.
///
/// `wanted` is the caller's own mask: the answer is only yes for the access asked
/// for, and a fixed read-and-execute mask would answer no for a read-only grant and
/// send the caller down the merge path again.
fn dacl_already_grants(dacl: *const ACL, sid: PSID, wanted: u32) -> bool {
    /// `ACCESS_ALLOWED_ACE_TYPE` from `winnt.h`.
    const ALLOWED: u8 = 0x00;

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
        // Inherited-only: what the object passes down, not a grant on itself.
        if allowed.Header.AceFlags & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        // `SidStart` is the first four bytes of the SID that follows the header.
        let entry = std::ptr::addr_of!(allowed.SidStart).cast::<c_void>() as PSID;
        if unsafe { EqualSid(entry, sid) } != 0 && allowed.Mask & wanted == wanted {
            return true;
        }
    }
    false
}

/// Whether `dacl` already gives every AppContainer the access `wanted` asks for.
///
/// `ALL APPLICATION PACKAGES` (`WinBuiltinAnyPackageSid`) is the principal the
/// system tree carries, so a file the OS already granted needs no ACE from this
/// process.
///
/// It answers for the *plain* container only: the less privileged one makes the
/// low-box check ignore this principal, so the caller must not ask it on behalf of
/// a launch asking for the opt-out.
fn dacl_grants_any_package(dacl: *const ACL, wanted: u32) -> bool {
    let mut sid = [0u8; MAX_SID_BYTES];
    let mut length = size_of_val(&sid) as u32;
    let created = unsafe {
        CreateWellKnownSid(
            WinBuiltinAnyPackageSid,
            null_mut(),
            sid.as_mut_ptr().cast::<c_void>(),
            &mut length,
        )
    };
    created != 0 && dacl_already_grants(dacl, sid.as_mut_ptr().cast::<c_void>(), wanted)
}

/// Whether this process's token is an AppContainer's.
///
/// The child reads its own token rather than the command line: a request the kernel
/// did not grant must not become a claim, and this is what the file and network
/// layers are claimed on.
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

/// Whether this process's token is a restricted one.
///
/// Read back rather than taken from the command line: what the process carries is
/// the fact the report is about.
fn is_restricted_token() -> bool {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut restricted = 0i32;
    let mut length = 0u32;
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenIsRestricted,
            std::ptr::addr_of_mut!(restricted).cast::<c_void>(),
            size_of::<i32>() as u32,
            &mut length,
        )
    };
    unsafe { CloseHandle(token) };
    read != 0 && restricted != 0
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

    // No integrity label is set, and that is measured: a low label on the
    // token-only path leaves the child unable to open the desktop its loader
    // attaches to, and it dies with `STATUS_DLL_INIT_FAILED` before it can print.
    // Chromium can run renderers below medium because it gives them a window station
    // of their own; this child has the interactive one, and the container supplies
    // the write bound instead. The report says which level the token got.
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

    /// This user's own SID, which the objects a process needs to attach to are
    /// granted to.
    ///
    /// A restricted token is checked twice on every access, and the *restricting*
    /// check is the one that decides write access. The window station and desktop
    /// are granted to the user, so a restricting list without this SID leaves the
    /// child created and then dead inside the loader with `STATUS_DLL_INIT_FAILED`
    /// (`0xC0000142`) — Microsoft's KB 184802, "does not have correct security
    /// access to the window station and desktop". Chromium carries it for the same
    /// reason.
    fn push_user(&mut self, token: HANDLE) -> bool {
        let mut needed = 0u32;
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return false;
        }

        // `TOKEN_USER` holds one `SID_AND_ATTRIBUTES` pointing at a SID inside the
        // buffer, which is word-aligned because it holds a pointer.
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
    /// The set is a process's own identity, which the restricting check must satisfy
    /// for the window station and desktop the child inherits.
    fn push_well_known(&mut self, kind: WELL_KNOWN_SID_TYPE) -> bool {
        // Bytes, not words: `MAX_SID_BYTES` is a byte count, and rounding to words
        // would leave less room than the constant promises.
        let mut sid = [0u8; MAX_SID_BYTES];
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
/// An empty environment does not work: `winsock` needs `SystemRoot`, and the loader
/// and temporary-directory helpers read the others — libuv keeps a list like this
/// for the same reason. `LOCALAPPDATA` is read by the AppContainer path while the
/// process is created and is required to be present even with a nonsense value;
/// here it points at the user's own directory, which the container cannot write.
///
/// The identity variables a parser has no use for (`USERNAME`, `USERPROFILE`,
/// `LOGONSERVER`, …) are deliberately absent, and so is anything this deployment set.
const SYSTEM_VARIABLES: &[&str] = &[
    "LOCALAPPDATA",
    "Path",
    "SystemDrive",
    "SystemRoot",
    "TEMP",
    "TMP",
    "WINDIR",
];

/// The environment the child starts with: the system variables above, the two Rust
/// switches, and nothing else.
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
    // Windows expects the order an environment is kept in: by name,
    // case-insensitively.
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

    /// The closing quote must stay a quote: trailing backslashes double.
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

    /// The names are the whole allowlist.
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

    /// The process count the job must allow is the profile's own: a worker that
    /// starts a helper has two slots and one that does not has one.
    #[test]
    fn only_the_media_profile_needs_a_second_process_slot() {
        assert_eq!(process_slots(Profile::Documents), 1);
        assert_eq!(process_slots(Profile::Images), 1);
        assert_eq!(process_slots(Profile::Media), 2);
    }

    /// The store is decided by the path, not by the write having succeeded.
    #[test]
    fn the_container_store_is_recognised_by_its_path() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return; // a host that does not say where the user's state lives
        };
        let packages = Path::new(&local).join("Packages");
        let inside = packages.join("nanofile_abc").join("Temp").join("x");
        assert!(is_container_store(&inside), "{}", inside.display());

        // The kernel resolves the write to a verbatim path, and the prefix is
        // not a path component: `\\?\C:` is the same place as `C:`.
        let verbatim = std::path::PathBuf::from(format!(r"\\?\{}", inside.display()));
        assert!(is_container_store(&verbatim), "{}", verbatim.display());

        // A write anywhere else is the user's own, which is the case the
        // container exists to prevent.
        assert!(!is_container_store(
            &Path::new(&local).join("Temp").join("x")
        ));
        assert!(!is_container_store(Path::new(r"C:\Windows\Temp\x")));
        // `Packages` as a prefix of a longer component name is not the store.
        assert!(!is_container_store(
            &Path::new(&local).join("Packages-old").join("x")
        ));
    }

    /// A process that is not in a container reaches the user's own temporary
    /// directory and says so. A measurement stuck on one answer would certify a
    /// sandbox that is not there.
    #[test]
    fn this_process_reports_itself_unconfined() {
        assert!(!is_app_container(), "a test process is not in a container");
        assert!(
            !is_restricted_token(),
            "a test process does not carry a restricted token"
        );
        assert_ne!(
            integrity_level(),
            "unknown",
            "the token's integrity level is readable on any supported Windows"
        );
        assert_eq!(
            write_scope(),
            "user",
            "nothing redirects this process's writes"
        );

        // No job named for this process, so no limits may be claimed: either there
        // is none, or it is the host's and its values are not these.
        for slots in [1, 2] {
            assert!(
                job_limits(slots).is_err(),
                "a process the parent did not confine has no limits to report"
            );
        }
    }

    /// A package SID that belongs to no file on this host, as owned bytes.
    ///
    /// Derived rather than created, so a test leaves no profile behind.
    fn package_sid(name: &str) -> Vec<u8> {
        let name: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sid: PSID = null_mut();
        let derived = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
        assert!(
            derived >= 0 && !sid.is_null(),
            "deriving a SID from a name works on any supported Windows"
        );
        let length = unsafe { GetLengthSid(sid) } as usize;
        assert!(length > 0 && length <= MAX_SID_BYTES);
        // Copied out: the allocation belongs to the API's caller, and these tests
        // only need the value.
        unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) }.to_vec()
    }

    /// `ALL APPLICATION PACKAGES`, the wildcard principal a granted tree carries.
    ///
    /// `WinBuiltinAnyPackageSid` is `S-1-15-2-1`; the name-derived SID above is one
    /// package, which the shortcut does not answer for.
    fn any_package_sid() -> Vec<u8> {
        let mut sid = [0u8; MAX_SID_BYTES];
        let mut length = size_of_val(&sid) as u32;
        let created = unsafe {
            CreateWellKnownSid(
                WinBuiltinAnyPackageSid,
                null_mut(),
                sid.as_mut_ptr().cast::<c_void>(),
                &mut length,
            )
        };
        assert_ne!(
            created, 0,
            "the well-known SID is built on any supported Windows"
        );
        sid[..length as usize].to_vec()
    }

    /// A file in the temporary directory whose name nothing else uses.
    fn scratch_file() -> (std::path::PathBuf, Vec<u16>) {
        let path = std::env::temp_dir().join(format!("nanofile-acl-test-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"").expect("a file to grant on");
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        (path, wide)
    }

    /// The DACL of `wide`, in a descriptor the caller frees with `LocalFree`.
    fn dacl_of(wide: &[u16]) -> (*mut ACL, PSECURITY_DESCRIPTOR) {
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let read = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(read, 0, "the DACL of a file this process just made is read");
        assert!(!dacl.is_null(), "a created file has a DACL");
        (dacl, descriptor)
    }

    /// The shortcut answers for the plain container and not the less privileged one,
    /// which is why the launch's opt-out decision travels into the grant: a skipped
    /// ACE is a child that cannot read the file it was started from, reported as a
    /// loader death rather than as anything an operator can read.
    #[test]
    fn the_any_package_shortcut_is_asked_of_the_launch_that_needs_it() {
        let (path, wide) = scratch_file();
        let any_package = any_package_sid();
        let package = package_sid("Nanofile.Test.Package");
        let mask = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

        // An ACE for the wildcard package, which is what an installer's tree
        // carries: written with the shortcut allowed, because that is how such a
        // tree gets one.
        let mut opt_out = false;
        add_access(
            path.as_os_str(),
            any_package.as_ptr().cast_mut().cast::<c_void>(),
            mask,
            &mut opt_out,
        )
        .expect("the wildcard ACE is written");

        let (dacl, descriptor) = dacl_of(&wide);
        let held = package.as_ptr().cast_mut().cast::<c_void>();
        assert!(
            already_granted(dacl, held, mask, true),
            "the plain container is answered by ALL APPLICATION PACKAGES"
        );
        assert!(
            !already_granted(dacl, held, mask, false),
            "under the opt-out that principal is ignored, so the grant is missing"
        );
        unsafe { LocalFree(descriptor) };
        let _ = std::fs::remove_file(&path);
    }

    /// An inherited-only ACE is passed down, not granted.
    #[test]
    fn an_inherit_only_ace_does_not_grant_the_object() {
        use windows_sys::Win32::Security::{AddAccessAllowedAceEx, InitializeAcl};
        use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;

        const ACL_REVISION: u32 = 2;
        let mask = FILE_GENERIC_READ;
        // `_owned` outlives every use of the pointer: the shadowing does not free
        // it.
        let sid_owned = package_sid("Nanofile.Test.InheritOnly");
        let sid = sid_owned.as_ptr().cast_mut().cast::<c_void>();

        // Word-aligned because an ACL holds pointers, and room for two ACEs.
        let mut storage = [0u64; 32];
        let length = size_of_val(&storage) as u32;
        let acl = storage.as_mut_ptr().cast::<ACL>();
        assert_ne!(
            unsafe { InitializeAcl(acl, length, ACL_REVISION) },
            0,
            "an empty ACL is made"
        );
        assert_ne!(
            unsafe { AddAccessAllowedAceEx(acl, ACL_REVISION, INHERIT_ONLY_ACE as u32, mask, sid) },
            0,
            "the inherited-only ACE is added"
        );
        assert!(
            !dacl_already_grants(acl, sid, mask),
            "an ACE the object only passes down does not apply to it"
        );
        assert_ne!(
            unsafe { AddAccessAllowedAceEx(acl, ACL_REVISION, 0, mask, sid) },
            0,
            "the plain ACE is added"
        );
        assert!(
            dacl_already_grants(acl, sid, mask),
            "an ACE with no flags is the grant"
        );
        // The answer is about the access asked for: this ACE carries read, not
        // write, so a wanted mask with both is not granted by it.
        assert!(
            !dacl_already_grants(acl, sid, mask | FILE_GENERIC_WRITE),
            "the ACE carries read and not write"
        );
    }
}
