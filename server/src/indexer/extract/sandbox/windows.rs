//! Windows confinement: a Job Object around the child, and a restricted token
//! for it to run under.
//!
//! Windows has no `RLIMIT_AS` and no unprivileged equivalent of Landlock, so
//! the layers are these two, and they are applied in different processes:
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
//!
//! The restricting SIDs are `Everyone` and the logon SID, and both are needed
//! for different reasons: `Everyone` is what still lets the process read the
//! system files it loads DLLs from — dropping it fails the process with
//! `STATUS_DLL_INIT_FAILED` before any of our code runs — and the logon SID is
//! what the session's own objects are granted to. Nothing else is granted, so a
//! parser that tries to leave a mark has nowhere to write.
//!
//! This is what both of the sandboxes this follows report as *partial*: reads
//! are only partly confined (`Everyone` cannot be dropped, and NTFS hard links
//! alias one file object across paths), so the level here is never `Full`.
//! A restricted token also cannot be asked for on a process that already
//! exists, which is why `spawn` — not `confine` — is where it happens, and why
//! a token that cannot be created falls back to an unrestricted (but still
//! Job-limited) child and says so.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    CopySid, CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, GetLengthSid,
    GetTokenInformation, IsValidSid, LUA_TOKEN, PSID, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_QUERY, TokenGroups,
    WRITE_RESTRICTED, WinWorldSid,
};
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
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

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

/// Start the child, with a restricted token when the system will make one.
///
/// The token is created first because the child is told whether it got one;
/// a host that refuses `CreateProcessAsUser` still gets a job-limited worker
/// rather than no worker at all.
pub(crate) fn spawn(program: &OsStr, args: &[OsString]) -> std::io::Result<Child> {
    if let Some(token) = restricted_token() {
        let mut with_token = args.to_vec();
        // The child cannot see the token it was created with, and whether
        // writes are restricted is worth saying out loud.
        with_token.push(OsString::from("--restricted"));
        match start(program, &with_token, Some(token)) {
            Ok(child) => return Ok(child),
            Err(restricted) => {
                tracing::warn!(
                    "extract-worker: the restricted token was refused ({restricted}); \
                     starting the child with the default token"
                );
                // Both failures are carried, not just the last one: the first
                // says whether the token or the launch was refused, and the
                // probe path has no subscriber for the warning above to reach.
                return start(program, args, None).map_err(|default| {
                    std::io::Error::new(
                        default.kind(),
                        format!("with a restricted token: {restricted}; without one: {default}"),
                    )
                });
            }
        }
    }
    start(program, args, None)
}

/// Create the process, its pipes and the handle list that keeps inheritance to
/// those pipes.
///
/// The token is closed on every path, including the failing ones: this runs
/// once per document, and a leak here would be a leak in the server's own
/// handle table.
fn start(program: &OsStr, args: &[OsString], token: Option<HANDLE>) -> std::io::Result<Child> {
    let started = start_with(program, args, token);
    if let Some(token) = token {
        unsafe { CloseHandle(token) };
    }
    started
}

fn start_with(program: &OsStr, args: &[OsString], token: Option<HANDLE>) -> std::io::Result<Child> {
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
    // Only these three handles are inherited. Without the list,
    // `bInheritHandles` would hand the child everything inheritable this
    // process holds — the server's own standard streams among it.
    let mut attributes = match AttributeList::new(inherited) {
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

/// A `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, which is what limits inheritance to
/// the handles a child is actually given.
struct AttributeList {
    buffer: Vec<u8>,
}

impl AttributeList {
    fn new(handles: [HANDLE; 3]) -> std::io::Result<Self> {
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(&handles),
                null_mut(),
                null(),
            )
        };
        if updated == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(list) };
            return Err(error);
        }
        Ok(Self { buffer })
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
    if !sids.push_logon_sid(own) || !sids.push_everyone() {
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
    storage: [u64; 32],
    entries: [SID_AND_ATTRIBUTES; 2],
    used: usize,
    count: usize,
}

impl RestrictingSids {
    fn new() -> Self {
        Self {
            storage: [0; 32],
            entries: [SID_AND_ATTRIBUTES {
                Sid: null_mut(),
                Attributes: 0,
            }; 2],
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

    /// The `Everyone` group, which every system file grants read access to.
    fn push_everyone(&mut self) -> bool {
        let mut sid = [0u64; MAX_SID_BYTES / 8];
        let mut length = size_of_val(&sid) as u32;
        let created = unsafe {
            CreateWellKnownSid(
                WinWorldSid,
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

/// The environment the child starts with: the two variables the runtime reads,
/// and nothing else.
///
/// The server's environment holds the master secret, the storage keys and every
/// resolved path, so it is not passed on.
fn environment_block() -> Vec<u16> {
    let mut block: Vec<u16> = Vec::new();
    for entry in ["RUST_BACKTRACE=0", "RUST_LIB_BACKTRACE=0"] {
        block.extend(entry.encode_utf16());
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

    #[test]
    fn the_environment_block_holds_only_the_backtrace_switches() {
        let block = environment_block();
        assert_eq!(block.last(), Some(&0));
        assert_eq!(block[block.len() - 2], 0, "the block ends with two zeros");
        let text = String::from_utf16_lossy(&block);
        assert!(text.contains("RUST_BACKTRACE=0"));
        assert!(text.contains("RUST_LIB_BACKTRACE=0"));
        assert_eq!(text.matches('=').count(), 2);
    }
}
