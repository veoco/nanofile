//! Windows confinement: a Job Object around the child, a restricted token for it
//! to run under, and an AppContainer around that.
//!
//! Windows has no `RLIMIT_AS` and no unprivileged equivalent of Landlock, so the
//! layers are these three, and they are applied in two processes:
//!
//! * **The Job Object** (this child, at startup) caps the process's committed
//!   memory, its CPU time and how many processes it may hold, and restricts the
//!   window station, the clipboard and handles to other processes. The child
//!   creates it and assigns itself, so the layer is one it can verify.
//! * **The restricted token** (the parent, at creation) cannot be applied to a
//!   running process, so it is the parent that starts the child with one:
//!   `CreateProcessAsUser` with a restricted version of the parent's own token,
//!   which is the one case Windows allows without `SeAssignPrimaryToken`.
//!   Privileges are gone, the administrative SIDs are deny-only, and write
//!   access is checked against the restricting SIDs alone.
//! * **The AppContainer** (the parent, at creation) is a process-creation
//!   attribute rather than a token: `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`
//!   with an AppContainer SID and *no capabilities* is what turns the child into
//!   a low-box process. Capabilities are what grant a container network access
//!   and access outside its own package, so an empty list is a process that
//!   cannot open a socket to anything and cannot read a file whose ACL names only
//!   its user. This is the configuration Chromium's own zero-capability sandbox
//!   uses, and the one the reference below documents.
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
//! What no layer here bounds is the shape of the boundary itself: the container
//! still reads the system tree it loads from (`Windows`, `Program Files` — the
//! paths `ALL APPLICATION PACKAGES` covers), still reaches local IPC through the
//! handles it inherits, and still reads the registry. Those are the platform's
//! own limits, and they are the same shape as the macOS profile's grants.
//!
//! # References
//!
//! * Microsoft, *Launch an AppContainer*: the attribute, the empty capability
//!   list, and the requirement that the image be readable by the container.
//! * Chromium's `sandbox/win/src/`: the same launch (`CreateProcessAsUser` with a
//!   plain restricted token plus `SECURITY_CAPABILITIES`), and the check that
//!   refuses to start when the image is not accessible to the container.

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
use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
use windows_sys::Win32::Security::{
    ACL, CopySid, CreateRestrictedToken, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
    DISABLE_MAX_PRIVILEGE, GetLengthSid, GetTokenInformation, IsValidSid, LUA_TOKEN,
    NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_QUERY,
    TOKEN_USER, TokenGroups, TokenIsAppContainer, TokenUser, WELL_KNOWN_SID_TYPE, WRITE_RESTRICTED,
    WinAuthenticatedUserSid, WinBuiltinUsersSid, WinInteractiveSid, WinWorldSid,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
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

    match job_object() {
        Ok(job) => {
            layers.limits = true;
            layers.process = true;
            detail.push(format!("job=memory{MEMORY_LIMIT},cpu{CPU_LIMIT_SECONDS}s"));
            // The handle stays open on purpose: `KILL_ON_JOB_CLOSE` fires when
            // it is the last one, and this process is the only thing that
            // should end here.
            let _ = job;
        }
        Err(reason) => detail.push(format!("job={reason}")),
    }

    if is_app_container() {
        layers.files = true;
        layers.network = true;
        detail.push("container=appcontainer".to_string());
    }

    (layers, detail)
}

/// Create the job, set its limits and put this process in it.
fn job_object() -> Result<HANDLE, &'static str> {
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return Err("create-failed");
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
        return Err("limits-failed");
    }

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
    unsafe {
        SetInformationJobObject(
            job,
            JobObjectBasicUIRestrictions,
            std::ptr::addr_of!(ui).cast(),
            sized,
        )
    };

    if unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } == 0 {
        unsafe { CloseHandle(job) };
        return Err("assign-failed");
    }
    Ok(job)
}

/// A child started by the parent, with pipes for the protocol.
pub(crate) struct Child {
    process: HANDLE,
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
    }
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
    if let Some(token) = restricted_token() {
        let mut with_token = args.to_vec();
        // The child cannot see the token it was created with, and whether
        // writes are restricted is worth saying out loud.
        with_token.push(OsString::from("--restricted"));
        match start(program, &with_token, Some(token), container.as_ref()) {
            Ok(child) => return Ok(child),
            Err(restricted) => {
                if container.is_some() {
                    note_shortfall("launch-refused");
                }
                tracing::warn!(
                    "extract-worker: the restricted token was refused ({restricted}); \
                     starting the child with the default token"
                );
                // Both failures are carried, not just the last one: the first
                // says whether the token or the launch was refused, and the
                // probe path has no subscriber for the warning above to reach.
                return start(program, args, None, None).map_err(|default| {
                    std::io::Error::new(
                        default.kind(),
                        format!("with a restricted token: {restricted}; without one: {default}"),
                    )
                });
            }
        }
    }
    start(program, args, None, None)
}

/// Start the child with this process's own token and no container.
///
/// Only the probe uses this. A child that was created and then died before it
/// reported can be failing because of its token or its container, or because of
/// everything else about how it was started, and the two are told apart the only
/// way that cannot be argued with: start it again without either and see.
pub(crate) fn spawn_unrestricted(program: &OsStr, args: &[OsString]) -> std::io::Result<Child> {
    start(program, args, None, None)
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
    // Only these three handles are inherited, and the container is named in the
    // same list. Without the list, `bInheritHandles` would hand the child
    // everything inheritable this process holds — the server's own standard
    // streams among it.
    let mut attributes =
        match AttributeList::new(&inherited, container.map(|held| &held.capabilities)) {
            Ok(attributes) => attributes,
            Err(error) => {
                close_all(&inherited);
                close_all(&[parent_stdin, parent_stdout, parent_stderr]);
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

    let flags = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
    let environment = environment.as_ptr().cast::<c_void>();
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
                null(),
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
                null(),
                &startup.StartupInfo,
                &mut info,
            ),
        }
    };

    close_all(&inherited);
    if created == 0 {
        let error = std::io::Error::last_os_error();
        close_all(&[parent_stdin, parent_stdout, parent_stderr]);
        return Err(error);
    }

    unsafe { CloseHandle(info.hThread) };
    Ok(Child {
        process: info.hProcess,
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
/// and the AppContainer it runs in.
struct AttributeList {
    buffer: Vec<u8>,
}

impl AttributeList {
    fn new(
        handles: &[HANDLE; 3],
        capabilities: Option<&SECURITY_CAPABILITIES>,
    ) -> std::io::Result<Self> {
        let count = 1 + u32::from(capabilities.is_some()) as usize;
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count as u32, 0, &mut size) };
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, count as u32, 0, &mut size) } == 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut attributes = Self { buffer };
        let handles = unsafe {
            attributes.set(
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast::<c_void>(),
                size_of_val(&handles),
            )
        };
        let container = match capabilities {
            Some(capabilities) => unsafe {
                attributes.set(
                    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                    std::ptr::from_ref(capabilities).cast::<c_void>(),
                    size_of::<SECURITY_CAPABILITIES>(),
                )
            },
            None => Ok(()),
        };
        match handles.and(container) {
            Ok(()) => Ok(attributes),
            Err(error) => {
                // The buffer is a live attribute list, so the failing path drops
                // it through the same cleanup a successful one does.
                drop(attributes);
                Err(error)
            }
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
/// The probe prints it. Without it, losing the container would show up only as a
/// child that reports `files=open`, with the reason on a `tracing` warning the
/// probe path has no subscriber for.
static SHORTFALL: OnceLock<&'static str> = OnceLock::new();

/// The reason the container was asked for and not applied, if there is one.
pub(crate) fn shortfall() -> Option<&'static str> {
    SHORTFALL.get().copied()
}

fn note_shortfall(reason: &'static str) {
    let _ = SHORTFALL.set(reason);
}

/// The name the worker's AppContainer SID is derived from.
///
/// The SID is a hash of it rather than a registered package, which is all a
/// process that never writes to its own storage needs: the same name gives the
/// same SID on every run, and nothing has to be installed for it to exist.
const APP_CONTAINER_NAME: &str = "Nanofile.Extraction.Worker";

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
/// Derived rather than created: `CreateAppContainerProfile` would write a package
/// profile into the user's AppData for a process that has no storage of its own,
/// and this needs the SID alone.
fn app_container(program: &OsStr) -> Option<AppContainer> {
    let Some(sid) = app_container_sid() else {
        note_shortfall("sid-refused");
        return None;
    };
    // The image is the one file the child cannot run without, and it is the
    // parent's job to make it readable: inside the container the check that
    // matters is the package SID's, and a per-user install has no ACE for it.
    if !grant_image_access(program, sid) {
        note_shortfall("image-grant-refused");
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

/// This process's own SID for the worker container.
///
/// Derived once and kept: the SID is a constant of the name, the grant recorded
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
        let derived = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
        if derived < 0 || sid.is_null() {
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
    let (image, granted) = GRANTED.get_or_init(|| {
        (
            program.to_os_string(),
            add_read_execute(program, sid).is_ok(),
        )
    });
    *granted && image == program
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
    (created != 0 && !restricted.is_null()).then_some(restricted)
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
