//! Linux confinement: resource limits, capabilities, Landlock and seccomp.
//!
//! Everything is a raw syscall through `libc::syscall` with the kernel UAPI
//! defined locally. That is deliberate, and follows the `landlock-run` launcher
//! this file mirrors: the kernel's user-space ABI is stable by contract, the
//! definitions below are the audit record of exactly which kernel surface this
//! code touches, and the build does not depend on the toolchain's header or
//! `libc` vintage (several of these constants — `PR_SET_NO_NEW_PRIVS`,
//! `PR_SET_SECCOMP`, `capget` — are not in `libc` for Linux at all).
//!
//! Order matters and is not interchangeable:
//!
//! 1. close every descriptor above stderr, so nothing inherited can be reached;
//! 2. clamp the resource limits, with the soft limit equal to the hard one so
//!    the child cannot raise what bounds it;
//! 3. `PR_SET_NO_NEW_PRIVS`, which is what lets an unprivileged process install
//!    Landlock and a seccomp filter, and neutralises setuid escalation;
//! 4. drop every capability, and read them back;
//! 5. Landlock, with no grants at all — the document arrives on stdin, so this
//!    process has nothing legitimate to open;
//! 6. seccomp last, because Landlock's own syscalls must happen first.

use std::mem::size_of;
use std::path::Path;

use super::{Grants, Profile, Protections};

// ── prctl ───────────────────────────────────────────────────────────────────
const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;
const PR_CAP_AMBIENT: libc::c_int = 47;
const PR_CAP_AMBIENT_CLEAR_ALL: libc::c_ulong = 4;
/// `PR_SET_MDWE` and its one flag, from `linux/prctl.h` (Linux 6.3).
const PR_SET_MDWE: libc::c_int = 65;
const PR_MDWE_REFUSE_EXEC_GAIN: libc::c_ulong = 1 << 0;

// ── capabilities (linux/capability.h) ───────────────────────────────────────
const CAP_VERSION_3: u32 = 0x2008_0522;

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

// ── Landlock (linux/landlock.h) ─────────────────────────────────────────────
const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
/// Newest ABI this file knows about.
///
/// Six rather than five: the scope bits are the ABI-6 addition, and the report
/// carries the ABI that was actually used rather than this one.
const LANDLOCK_MAX_ABI: i64 = 6;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

const LL_EXECUTE: u64 = 1 << 0;
const LL_WRITE_FILE: u64 = 1 << 1;
const LL_READ_FILE: u64 = 1 << 2;
const LL_READ_DIR: u64 = 1 << 3;
const LL_REMOVE_DIR: u64 = 1 << 4;
const LL_REMOVE_FILE: u64 = 1 << 5;
const LL_MAKE_CHAR: u64 = 1 << 6;
const LL_MAKE_DIR: u64 = 1 << 7;
const LL_MAKE_REG: u64 = 1 << 8;
const LL_MAKE_SOCK: u64 = 1 << 9;
const LL_MAKE_FIFO: u64 = 1 << 10;
const LL_MAKE_BLOCK: u64 = 1 << 11;
const LL_MAKE_SYM: u64 = 1 << 12;
const LL_REFER: u64 = 1 << 13;
const LL_TRUNCATE: u64 = 1 << 14;
const LL_IOCTL_DEV: u64 = 1 << 15;

/// Network rights (ABI 4): handling them with no rule denies every TCP
/// bind and connect, which is the network layer again — the one that survives a
/// kernel without seccomp.
const LL_NET_BIND_TCP: u64 = 1 << 0;
const LL_NET_CONNECT_TCP: u64 = 1 << 1;

/// IPC scopes (ABI 6): a domain cannot signal a process outside itself, and
/// cannot reach an abstract socket outside itself.
///
/// This is what closes the one hole a denylist cannot: `kill` must be judged by
/// its *target*, which a classic BPF filter cannot read, and Landlock decides
/// it against the domain instead.
const LL_SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const LL_SCOPE_SIGNAL: u64 = 1 << 1;

// ── seccomp (linux/seccomp.h, linux/filter.h, linux/bpf_common.h) ───────────
const SECCOMP_SET_MODE_FILTER: libc::c_uint = 1;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JEQ_K: u16 = 0x15;
const BPF_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;

/// Offset of `nr`, `arch` and the low half of `args[0]` in `struct seccomp_data`.
const SECCOMP_DATA_NR: u32 = 0;
const SECCOMP_DATA_ARCH: u32 = 4;
const SECCOMP_DATA_ARGS0_LOW: u32 = 16;

const CLONE_THREAD: u32 = 0x0001_0000;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    length: u16,
    filter: *const SockFilter,
}

/// `AUDIT_ARCH_*` for this target, or `None` where the filter cannot be built.
///
/// The constant is `EM_* | __AUDIT_ARCH_64BIT | __AUDIT_ARCH_LE` from
/// `linux/audit.h`; the arch check is what stops a 32-bit compatibility entry
/// point from bypassing the filter.
#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: Option<u32> = Some(0xC000_003E);
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: Option<u32> = Some(0xC000_00B7);
#[cfg(target_arch = "loongarch64")]
const AUDIT_ARCH: Option<u32> = Some(0xC000_0102);
#[cfg(target_arch = "riscv64")]
const AUDIT_ARCH: Option<u32> = Some(0xC000_00F3);
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "loongarch64",
    target_arch = "riscv64"
)))]
const AUDIT_ARCH: Option<u32> = None;

/// Apply every layer this platform offers.
pub(super) fn confine(profile: Profile, grants: Grants<'_>) -> (Protections, Vec<String>) {
    let mut layers = Protections::default();
    let mut detail = Vec::new();

    close_extra_descriptors();

    let limits = super::clamp_resources();
    // The address-space limit is this platform's memory bound; there is no other.
    if limits.enforced(limits.address_space) {
        layers.limits = true;
    }
    detail.push(format!("limits={}", limits.detail));

    detail.push(
        if set_no_new_privs() {
            "no_new_privs=on"
        } else {
            "no_new_privs=failed"
        }
        .to_string(),
    );

    detail.push(if set_mdwe() { "mdwe=on" } else { "mdwe=off" }.to_string());

    detail.push(drop_capabilities().to_string());
    // Both are free and both close a way for a process of the same user to
    // reach this one: `PR_SET_DUMPABLE` takes away `/proc/<pid>/mem` and the
    // ability to attach, and the parent-death signal is what ends a parser
    // whose parent is gone. Neither is a layer — the report says what took.
    detail.push(
        if set_dumpable() {
            "dumpable=off"
        } else {
            "dumpable=refused"
        }
        .to_string(),
    );
    detail.push(
        if set_parent_death_signal() {
            "pdeathsig=on"
        } else {
            "pdeathsig=refused"
        }
        .to_string(),
    );

    match install_landlock(profile, grants) {
        Ok(landlock) => {
            layers.files = true;
            detail.push(format!("landlock_abi={}", landlock.abi));
            detail.push(format!("landlock_net={}", on_off(landlock.network)));
            detail.push(format!("landlock_scoped={}", on_off(landlock.scoped)));
            if profile.runs_helper() {
                detail.push(format!("helper_grants={}", landlock.helper_grants));
            }
        }
        Err(reason) => detail.push(format!("landlock={reason}")),
    }

    match install_seccomp(profile) {
        Some(denied) => {
            layers.network = true;
            layers.process = true;
            detail.push(format!("seccomp={denied}"));
            // Landlock sees opens and creations; it does not see a metadata
            // change, a truncation before ABI 3, or who a signal is for. Those
            // are this filter's job (see `denied_syscalls`), so a host where it
            // did not install has a filesystem layer with holes in it rather
            // than a missing one — which is what this says.
            detail.push("ll_gaps=closed".to_string());
        }
        None => {
            detail.push("seccomp=off".to_string());
            detail.push("ll_gaps=open".to_string());
        }
    }

    (layers, detail)
}

/// How a boolean fact is spelled in the report, for facts that are not layers.
fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// Make this process undumpable.
///
/// A process of the same user can otherwise read this one through
/// `/proc/<pid>/mem` and attach to it; nothing here defends against a same-user
/// process in general (see the module docs), but this is the difference between
/// "the sandbox does not stop your neighbour" and "your neighbour can read the
/// document the server just handed over".
fn set_dumpable() -> bool {
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0, 0, 0) == 0 }
}

/// Ask the kernel to end this process when its parent ends.
///
/// The parent bounds every child with a timeout and kills the group it started,
/// so this covers the one case it cannot: the parent itself is gone, and
/// nothing is left to wait for the parser or to collect it.
fn set_parent_death_signal() -> bool {
    unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGKILL as libc::c_ulong,
            0,
            0,
            0,
        ) == 0
    }
}

/// Close every descriptor above stderr.
///
/// The parent's descriptors are close-on-exec where the standard library
/// created them, but a parser exploited for a file read would still rather be
/// handed one than have to open it. `close_range` cannot be raced against a
/// concurrent open; the loop is the fallback for kernels before 5.9, and it
/// walks to this process's own descriptor limit rather than to a constant —
/// a parent with a raised `RLIMIT_NOFILE` can hold a descriptor the constant
/// would have missed.
fn close_extra_descriptors() {
    let closed = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3 as libc::c_uint,
            libc::c_uint::MAX,
            0 as libc::c_uint,
        )
    };
    if closed == 0 {
        return;
    }

    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let highest = match unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } {
        // The clamp below lowers this to `NOFILE_LIMIT`; what matters is the
        // *current* limit, which is what the parent's descriptors were opened
        // under.
        0 if limit.rlim_cur != libc::RLIM_INFINITY => limit.rlim_cur,
        // Unlimited, or a kernel that would not say: the constant is only the
        // floor under a case this cannot enumerate.
        _ => 1024,
    };
    for descriptor in 3..highest {
        unsafe { libc::close(descriptor as libc::c_int) };
    }
}

/// Set `no_new_privs`, without which an unprivileged process may install
/// neither a Landlock ruleset nor a seccomp filter.
fn set_no_new_privs() -> bool {
    unsafe {
        libc::syscall(
            libc::SYS_prctl,
            PR_SET_NO_NEW_PRIVS,
            1 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        ) == 0
    }
}

/// Ask the kernel to refuse a mapping that is both writable and executable.
///
/// This is the one hardening that costs nothing and closes the classic
/// exploitation step — write shellcode into a buffer, mark it executable, jump
/// to it — at the kernel rather than in the parser. It is inherited by a helper
/// the media profile starts, which still maps its own libraries: those come from
/// files and are never made writable first.
///
/// A kernel older than 6.3 does not know the call and answers `EINVAL`, which is
/// reported as `mdwe=off` rather than treated as a failure: the protection is
/// additive, and the confinement is decided from the items a probe measures.
fn set_mdwe() -> bool {
    unsafe {
        libc::syscall(
            libc::SYS_prctl,
            PR_SET_MDWE,
            PR_MDWE_REFUSE_EXEC_GAIN,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        ) == 0
    }
}

/// Drop every capability the process holds, then read the sets back.
///
/// A no-op for an unprivileged process, and the reason a server started as root
/// does not hand root to a parser. Dropping is always permitted; only adding
/// needs `CAP_SETPCAP`.
fn drop_capabilities() -> &'static str {
    let mut header = CapHeader {
        version: CAP_VERSION_3,
        pid: 0,
    };
    let mut before = [CapData::default(); 2];
    let read = unsafe {
        libc::syscall(
            libc::SYS_capget,
            &mut header as *mut CapHeader,
            before.as_mut_ptr(),
        )
    };
    if read != 0 {
        return "caps=unknown";
    }
    if is_empty(&before) {
        return "caps=empty";
    }

    let mut header = CapHeader {
        version: CAP_VERSION_3,
        pid: 0,
    };
    let zero = [CapData::default(); 2];
    let written = unsafe {
        libc::syscall(
            libc::SYS_capset,
            &mut header as *mut CapHeader,
            zero.as_ptr(),
        )
    };
    if written != 0 {
        return "caps=remaining";
    }
    // The ambient set is cleared separately, and cannot be regained once the
    // permitted set is empty.
    unsafe {
        libc::syscall(
            libc::SYS_prctl,
            PR_CAP_AMBIENT,
            PR_CAP_AMBIENT_CLEAR_ALL,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    "caps=empty"
}

fn is_empty(sets: &[CapData]) -> bool {
    sets.iter()
        .all(|set| set.effective == 0 && set.permitted == 0 && set.inheritable == 0)
}

/// What the ruleset that was installed actually governs.
struct Landlock {
    /// The ABI the kernel negotiated, capped at what this file knows.
    abi: i64,
    /// Whether the TCP rights were handled: the network layer without seccomp.
    network: bool,
    /// Whether the scope bits were handled: signals and abstract sockets cannot
    /// leave this domain, which is the one thing seccomp's denylist cannot say
    /// about a call it must allow.
    scoped: bool,
    /// How many path rules the media profile's grants installed.
    helper_grants: usize,
}

/// Install a Landlock ruleset that grants nothing — except the paths the media
/// profile was handed.
///
/// Every access the running kernel's ABI can govern is handled, and no
/// path-beneath rule is added for the profiles that read nothing: on Linux an
/// unhandled access is simply allowed, so handling the full mask *is* the
/// denial. This is stricter than the launcher this mirrors, which grants read
/// access to `/` because it wraps commands that need a filesystem; the document
/// and image children get their bytes on stdin.
///
/// The media profile is the exception: it executes a helper, so the helper and
/// the libraries it maps have to be readable and executable, and the scratch
/// source it decodes has to be readable. Those grants are `path-beneath` rules
/// on exact paths (never a directory the parent chose), so the reachable set is
/// the helper, the fixed system library trees and one scratch file.
///
/// Three groups of rights, added as the ABI that has them (the kernel rejects a
/// struct with fields it does not know, so the size passed is the one this ABI
/// reads): the filesystem mask, the two TCP rights from ABI 4, and the IPC
/// scopes from ABI 6.
///
/// A ruleset the kernel refuses is retried with the filesystem mask alone. The
/// alternative — reporting `landlock=ruleset` and losing the filesystem
/// protection because a bit this file guessed was wrong — would be a sandbox
/// that gives up its strongest protection over its weakest claim.
fn install_landlock(profile: Profile, grants: Grants<'_>) -> Result<Landlock, &'static str> {
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<LandlockRulesetAttr>(),
            0 as libc::size_t,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi < 1 {
        return Err("unsupported");
    }

    let mut taken = ruleset_for_abi(abi);
    let mut ruleset = create_ruleset(&taken.0, taken.1);
    if ruleset < 0 && taken.1 > size_of::<u64>() {
        taken = ruleset_for_abi(3);
        ruleset = create_ruleset(&taken.0, taken.1);
    }
    if ruleset < 0 {
        return Err("ruleset");
    }

    let mut helper_grants = 0;
    if profile.runs_helper() {
        helper_grants = add_helper_rules(ruleset, grants);
    }

    let restricted =
        unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0 as libc::c_uint) };
    unsafe { libc::close(ruleset as libc::c_int) };
    if restricted != 0 {
        return Err("restrict");
    }
    let (attr, size) = taken;
    Ok(Landlock {
        abi: abi.min(LANDLOCK_MAX_ABI),
        network: size >= 2 * size_of::<u64>() && attr.handled_access_net != 0,
        scoped: size >= 3 * size_of::<u64>() && attr.scoped != 0,
        helper_grants,
    })
}

/// `LANDLOCK_RULE_PATH_BENEATH`.
const LANDLOCK_RULE_PATH_BENEATH: libc::c_uint = 1;

/// One `landlock_path_beneath_attr`, packed the way the UAPI declares it.
#[repr(C, packed)]
struct LandlockPathBeneath {
    allowed_access: u64,
    parent_fd: i32,
}

/// Add the media profile's read and execute rules.
///
/// The helper file gets `execute` and `read`; the fixed system library trees get
/// `read` and `execute` — the kernel executes the dynamic linker itself out of
/// one of them before the helper's own code runs, so `read` alone is not enough
/// — and the scratch source gets `read` alone. A path that is not there is
/// skipped — a 32-bit library directory on a 64-bit host, an Intel cryptex path
/// — rather than failing the ruleset over a rule that grants nothing.
fn add_helper_rules(ruleset: i64, grants: Grants<'_>) -> usize {
    let mut added = 0;
    if let Some(helper) = grants.helper {
        added += add_path_rule(ruleset, helper, LL_EXECUTE | LL_READ_FILE) as usize;
        // The helper may sit beside its own libraries.
        if let Some(directory) = helper.parent() {
            added +=
                add_path_rule(ruleset, directory, LL_EXECUTE | LL_READ_FILE | LL_READ_DIR) as usize;
        }
    }
    if let Some(source) = grants.source {
        added += add_path_rule(ruleset, source, LL_READ_FILE) as usize;
    }
    for directory in HELPER_LIBRARY_DIRS {
        added += add_path_rule(
            ruleset,
            Path::new(directory),
            LL_EXECUTE | LL_READ_FILE | LL_READ_DIR,
        ) as usize;
    }
    for file in HELPER_LIBRARY_FILES {
        added += add_path_rule(ruleset, Path::new(file), LL_READ_FILE) as usize;
    }
    // The devices a spawned program opens for itself: `/dev/null` for the
    // standard streams it is not given, and the entropy the runtime may want.
    for (path, access) in HELPER_DEVICES {
        added += add_path_rule(ruleset, Path::new(path), *access) as usize;
    }
    added
}

/// The device files the media helper may open, and with what rights.
const HELPER_DEVICES: &[(&str, u64)] = &[
    ("/dev/null", LL_READ_FILE | LL_WRITE_FILE),
    ("/dev/urandom", LL_READ_FILE),
];

/// Where a dynamically linked helper's libraries live.
const HELPER_LIBRARY_DIRS: &[&str] = &[
    "/usr/lib",
    "/usr/lib64",
    "/usr/local/lib",
    "/usr/local/lib64",
    "/lib",
    "/lib64",
];

/// The loader's own cache, which is a file rather than a directory.
const HELPER_LIBRARY_FILES: &[&str] = &["/etc/ld.so.cache"];

/// One `landlock_add_rule` for a path, or nothing when the path is absent.
fn add_path_rule(ruleset: i64, path: &Path, access: u64) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    use std::os::fd::AsRawFd;
    let attr = LandlockPathBeneath {
        allowed_access: access,
        parent_fd: file.as_raw_fd(),
    };
    unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset,
            LANDLOCK_RULE_PATH_BENEATH as libc::c_long,
            &attr as *const LandlockPathBeneath,
            0 as libc::c_uint,
        ) == 0
    }
}

/// The ruleset to ask a kernel of `abi` for, and the size to pass with it.
fn ruleset_for_abi(abi: i64) -> (LandlockRulesetAttr, usize) {
    let mut attr = LandlockRulesetAttr {
        handled_access_fs: fs_mask_for_abi(abi),
        handled_access_net: 0,
        scoped: 0,
    };
    let mut size = size_of::<u64>();
    if abi >= 4 {
        attr.handled_access_net = LL_NET_BIND_TCP | LL_NET_CONNECT_TCP;
        size = 2 * size_of::<u64>();
    }
    if abi >= 6 {
        attr.scoped = LL_SCOPE_ABSTRACT_UNIX_SOCKET | LL_SCOPE_SIGNAL;
        size = size_of::<LandlockRulesetAttr>();
    }
    (attr, size)
}

/// One `landlock_create_ruleset` call, or a negative errno.
fn create_ruleset(attr: &LandlockRulesetAttr, size: usize) -> i64 {
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            attr as *const LandlockRulesetAttr,
            size as libc::size_t,
            0 as libc::c_uint,
        )
    }
}

/// Every filesystem access the given Landlock ABI can govern.
///
/// The mask grows with the ABI: handling a bit the kernel does not know is
/// rejected outright, so the newest bits are added only where they exist and
/// the report carries the ABI that was actually used.
fn fs_mask_for_abi(abi: i64) -> u64 {
    // Everything through `LL_MAKE_SYM` is ABI 1.
    let mut mask = LL_EXECUTE
        | LL_WRITE_FILE
        | LL_READ_FILE
        | LL_READ_DIR
        | LL_REMOVE_DIR
        | LL_REMOVE_FILE
        | LL_MAKE_CHAR
        | LL_MAKE_DIR
        | LL_MAKE_REG
        | LL_MAKE_SOCK
        | LL_MAKE_FIFO
        | LL_MAKE_BLOCK
        | LL_MAKE_SYM;
    if abi >= 2 {
        mask |= LL_REFER;
    }
    if abi >= 3 {
        mask |= LL_TRUNCATE;
    }
    if abi >= 5 {
        mask |= LL_IOCTL_DEV;
    }
    mask
}

/// Install the seccomp filter, returning how many syscalls it refuses.
///
/// The count is in the report because the list is what the layer is: a run
/// where the list silently stopped covering a call is a report that says so.
fn install_seccomp(profile: Profile) -> Option<usize> {
    let (program, denied) = filter_program(profile)?;
    let fprog = SockFprog {
        length: program.len() as u16,
        filter: program.as_ptr(),
    };
    let installed = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            0 as libc::c_uint,
            &fprog as *const SockFprog,
        )
    };
    (installed == 0).then_some(denied)
}

/// The filter, and how many syscalls it names.
///
/// A denylist rather than an allowlist, following Codex's Linux sandbox: an
/// allowlist has to enumerate every syscall a Rust runtime and two parsers may
/// legitimately make, and a name missing from it turns into a document that
/// cannot be read. `EPERM` rather than a kill is the same choice: a denied
/// call is an error the parser can survive, not a crash to attribute later.
///
/// The list carries two kinds of entry. The first is what a parser must never
/// do at all — sockets, starting programs, reading another process, the kernel
/// surfaces. The second is what Landlock *cannot* govern, which is a fact about
/// Landlock rather than about parsers: it sees opens and creations, not
/// metadata changes, not truncation before ABI 3, not a signal's target. Those
/// calls are denied here, where the decision is about the syscall rather than
/// about the path.
///
/// Returns `None` on an architecture whose audit code this file does not carry.
fn filter_program(profile: Profile) -> Option<(Vec<SockFilter>, usize)> {
    let arch = AUDIT_ARCH?;
    let denied = denied_syscalls(profile);
    let mut program = Vec::with_capacity(8 + 2 * denied.len());

    let stmt = |code: u16, k: u32| SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jump = |code: u16, k: u32, jt: u8, jf: u8| SockFilter { code, jt, jf, k };

    // A filter that does not pin the architecture can be reached through the
    // 32-bit compatibility entry point, where the numbers mean something else.
    program.push(stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARCH));
    program.push(jump(BPF_JEQ_K, arch, 1, 0));
    program.push(stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS));

    program.push(stmt(BPF_LD_W_ABS, SECCOMP_DATA_NR));
    for number in &denied {
        program.push(jump(BPF_JEQ_K, *number as u32, 0, 1));
        program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    // `clone3` carries its flags behind a pointer, so no filter can tell a
    // thread from a process there. Answering `ENOSYS` instead of refusing it is
    // what makes the runtime fall back to `clone`, whose flags *can* be read —
    // so the check below is the one that decides, and a caller that does not
    // fall back simply sees an unsupported syscall rather than a way through.
    program.push(jump(BPF_JEQ_K, libc::SYS_clone3 as u32, 0, 1));
    program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | ENOSYS));

    // `clone` stays reachable because the parsers run their work on a thread,
    // but only with `CLONE_THREAD`: without it, `clone` is how a process is
    // made. The flags live in the low half of the first argument, which is the
    // half this reads on a little-endian target — the only kind this carries.
    //
    // The check is reached only when the syscall is `clone` (`jf = 3` jumps the
    // whole block), because `args[0]` means something else everywhere else: a
    // check applied to every syscall would read the file descriptor a `write`
    // was given and refuse it for not carrying a thread flag.
    //
    // The media profile is the one exception: it has to make a process to run
    // the helper, so the argument check is not installed for it. What still
    // bounds it is the files grant — `exec` reaches the one helper binary — and
    // the parent's timeout and process-group kill.
    #[cfg(target_endian = "little")]
    if !profile.runs_helper() {
        program.push(jump(BPF_JEQ_K, libc::SYS_clone as u32, 0, 3));
        program.push(stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARGS0_LOW));
        program.push(jump(BPF_JSET_K, CLONE_THREAD, 1, 0));
        program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    // `prlimit64` is how the runtime sets its own limits — glibc's `setrlimit`
    // is this call — and it is also how a limit is set on *another* process.
    // Only the self form is allowed: the resource limits are this process's
    // alone to state, and a document that could lower the server's would have
    // reached the process the sandbox exists to protect. `args[0] == 0` is the
    // kernel's own spelling of "this process".
    #[cfg(target_endian = "little")]
    {
        program.push(jump(BPF_JEQ_K, libc::SYS_prlimit64 as u32, 0, 3));
        program.push(stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARGS0_LOW));
        program.push(jump(BPF_JEQ_K, 0, 1, 0));
        program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    program.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
    Some((program, denied.len()))
}

/// The syscalls the child never needs.
///
/// A denylist is only as good as the argument for each name being on it, and the
/// two arguments here are "a parser has no business doing this" and "Landlock
/// cannot decide this". Both are re-derivable, which is what keeps the list from
/// drifting into folklore:
///
/// * A new name goes on for a reason, and the reason goes above it.
/// * What the parsers actually call can be re-measured instead of argued about:
///   run the fixture corpus with the filter in `SECCOMP_RET_LOG` mode (or under
///   `strace -f -c`), and diff the syscalls seen against this list. Anything the
///   parsers never call and this list does not name is a candidate, and anything
///   the parsers *do* call that is named here is a report of `parse=` turning
///   into `unsupported` in the worker's own self-test.
fn denied_syscalls(profile: Profile) -> Vec<libc::c_long> {
    let mut denied = vec![
        // Sockets of every kind: the child talks to its parent over pipes.
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendto,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        libc::SYS_recvfrom,
        libc::SYS_recvmsg,
        libc::SYS_recvmmsg,
        libc::SYS_shutdown,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        // Starting another program, and process creation that is not a thread
        // (`clone` is handled by the argument check above, `clone3` cannot be
        // inspected so it is refused outright).
        libc::SYS_execve,
        libc::SYS_execveat,
        // Reading another process, and the kernel surfaces a parsed file has no
        // use for.
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kcmp,
        libc::SYS_pidfd_getfd,
        libc::SYS_process_madvise,
        libc::SYS_process_mrelease,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_userfaultfd,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_move_mount,
        libc::SYS_fsopen,
        libc::SYS_fsmount,
        libc::SYS_fsconfig,
        libc::SYS_open_tree,
        libc::SYS_setns,
        libc::SYS_unshare,
        libc::SYS_chroot,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_acct,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_quotactl,
        libc::SYS_open_by_handle_at,
        libc::SYS_memfd_create,
        // Reading another process, and the signal that would end it: the sandbox
        // has no business addressing anything outside itself. `pidfd_open` is
        // how a signal is sent without a pid race.
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_send_signal,
        // Reading the shape of the filesystem without reading a file: names,
        // sizes and existence. Landlock governs opens, so these reach past it —
        // an exploited parser could otherwise map the machine it is confined to.
        // `*at` forms only where the architecture has no bare one, which is
        // what the second block below is for.
        libc::SYS_newfstatat,
        libc::SYS_statx,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
        libc::SYS_readlinkat,
        libc::SYS_getdents64,
        libc::SYS_chdir,
        libc::SYS_fchdir,
        libc::SYS_name_to_handle_at,
        // The calls Landlock documents as ungovernable: metadata changes and
        // truncation. None has a use here — the document arrives on stdin and
        // nothing is written — so the syscall is where they are refused.
        libc::SYS_ftruncate,
        libc::SYS_truncate,
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        libc::SYS_fchown,
        libc::SYS_fchownat,
        libc::SYS_fsetxattr,
        libc::SYS_lsetxattr,
        libc::SYS_setxattr,
        libc::SYS_fremovexattr,
        libc::SYS_lremovexattr,
        libc::SYS_removexattr,
        libc::SYS_utimensat,
    ];

    // The bare path-taking forms of the same calls: the architectures that kept
    // the old table have them (x86, and the 32-bit ones), and the ones that did
    // not — aarch64, riscv64, loongarch64 — reach the same objects through the
    // `*at` forms above. A number a kernel does not have cannot be denied here
    // and does not need to be.
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        denied.push(libc::SYS_chmod);
        denied.push(libc::SYS_chown);
        denied.push(libc::SYS_lchown);
        denied.push(libc::SYS_utime);
        denied.push(libc::SYS_utimes);
        denied.push(libc::SYS_futimesat);
        denied.push(libc::SYS_stat);
        denied.push(libc::SYS_lstat);
        denied.push(libc::SYS_access);
        denied.push(libc::SYS_readlink);
    }

    // `fork` and `vfork` only exist where the kernel has them; the architectures
    // that do not (aarch64, loongarch64) reach process creation through `clone`,
    // which the argument check covers.
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        denied.push(libc::SYS_fork);
        denied.push(libc::SYS_vfork);
    }

    // The media profile adds back what a second, dynamically linked program
    // needs and a parser never does: the fork/exec pair, the calls the loader
    // makes while it maps the helper's libraries, and the pipe/dup that connect
    // it. Every one of them is either bounded by the files grant (`exec` and
    // `open` are decided by Landlock) or leaks only metadata. Dropping the
    // metadata calls is the one visible weakening: they are not governed by
    // Landlock, so allowing them lets a compromised helper see which paths
    // exist — never read one.
    if profile.runs_helper() {
        denied.retain(|number| !MEDIA_ALLOWED.contains(number));
        // The bare `fork`/`vfork` pair, where the architecture has them: the
        // runtime's own spawn uses one of them, and the media profile is the one
        // that spawns. `clone` is already allowed for this profile (the argument
        // check is not installed).
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        denied.retain(|number| *number != libc::SYS_fork && *number != libc::SYS_vfork);
    }

    denied
}

/// The calls the media profile adds back to the denylist.
///
/// Each is here because a helper that is a separate program needs it; the list
/// is deliberately short, and Landlock still decides what any `open` or `exec`
/// may reach.
const MEDIA_ALLOWED: &[libc::c_long] = &[
    // Starting the helper.
    libc::SYS_execve,
    libc::SYS_execveat,
    // What the dynamic loader does before `main`: stat, readlink and access on
    // the search path, and the cache it reads. Landlock does not govern these,
    // so allowing them makes paths visible but not readable.
    libc::SYS_newfstatat,
    libc::SYS_statx,
    libc::SYS_readlinkat,
    libc::SYS_faccessat,
    libc::SYS_faccessat2,
    libc::SYS_getdents64,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_landlock_mask_grows_with_the_abi() {
        let abi1 = fs_mask_for_abi(1);
        assert_eq!(abi1 & LL_REFER, 0);
        assert_eq!(abi1 & LL_TRUNCATE, 0);
        assert_eq!(abi1 & LL_READ_FILE, LL_READ_FILE);
        assert_eq!(abi1 & LL_MAKE_SYM, LL_MAKE_SYM);

        assert_eq!(fs_mask_for_abi(2) & LL_REFER, LL_REFER);
        assert_eq!(fs_mask_for_abi(2) & LL_TRUNCATE, 0);
        assert_eq!(fs_mask_for_abi(3) & LL_TRUNCATE, LL_TRUNCATE);
        assert_eq!(fs_mask_for_abi(3) & LL_IOCTL_DEV, 0);
        assert_eq!(fs_mask_for_abi(5) & LL_IOCTL_DEV, LL_IOCTL_DEV);
        assert_eq!(fs_mask_for_abi(500), fs_mask_for_abi(5));
    }

    /// The filter pins the architecture first, refuses the whole denylist with
    /// `EPERM`, and ends by allowing everything it did not name.
    #[test]
    fn the_filter_refuses_the_denylist_and_allows_the_rest() {
        let Some((program, _)) = filter_program(Profile::Documents) else {
            return; // an architecture this file does not carry
        };

        assert_eq!(program[0].code, BPF_LD_W_ABS);
        assert_eq!(program[0].k, SECCOMP_DATA_ARCH);
        assert_eq!(program[1].code, BPF_JEQ_K);
        assert_eq!(program[1].k, AUDIT_ARCH.expect("checked above"));
        assert_eq!(program[2].k, SECCOMP_RET_KILL_PROCESS);
        assert_eq!(program.last().expect("non-empty").k, SECCOMP_RET_ALLOW);

        for number in denied_syscalls(Profile::Documents) {
            let matches_rule = program.windows(2).any(|pair| {
                pair[0].code == BPF_JEQ_K
                    && pair[0].k == number as u32
                    && pair[1].k == SECCOMP_RET_ERRNO | EPERM
            });
            assert!(matches_rule, "syscall {number} is not refused");
        }
    }

    #[test]
    fn the_denylist_covers_the_layers_it_claims() {
        let denied = denied_syscalls(Profile::Documents);
        for (name, number) in [
            ("socket", libc::SYS_socket),
            ("connect", libc::SYS_connect),
            ("execve", libc::SYS_execve),
            ("ptrace", libc::SYS_ptrace),
            ("bpf", libc::SYS_bpf),
            ("io_uring_setup", libc::SYS_io_uring_setup),
            ("userfaultfd", libc::SYS_userfaultfd),
            ("mount", libc::SYS_mount),
            ("setns", libc::SYS_setns),
            ("keyctl", libc::SYS_keyctl),
        ] {
            assert!(denied.contains(&number), "{name} must be refused");
        }
        // The parsers need these, so denying them would make every document
        // unreadable.
        for (name, number) in [
            ("read", libc::SYS_read),
            ("write", libc::SYS_write),
            ("mmap", libc::SYS_mmap),
            ("futex", libc::SYS_futex),
            ("clone", libc::SYS_clone),
            ("getrandom", libc::SYS_getrandom),
        ] {
            assert!(!denied.contains(&number), "{name} must stay allowed");
        }
    }

    /// Evaluate the filter the way the kernel's BPF interpreter would.
    ///
    /// The tests below are only worth anything if they run the program rather
    /// than inspect it: the bug this caught — a `clone` argument check placed
    /// where every syscall reached it — looks perfectly reasonable as text.
    fn evaluate(program: &[SockFilter], arch: u32, nr: u32, args0: u32) -> u32 {
        let mut accumulator = 0u32;
        let mut pc = 0usize;
        loop {
            let instruction = program[pc];
            match instruction.code {
                BPF_LD_W_ABS => {
                    accumulator = match instruction.k {
                        SECCOMP_DATA_NR => nr,
                        SECCOMP_DATA_ARCH => arch,
                        SECCOMP_DATA_ARGS0_LOW => args0,
                        other => panic!("unexpected load offset {other}"),
                    }
                }
                BPF_JEQ_K => {
                    pc += if accumulator == instruction.k {
                        instruction.jt as usize
                    } else {
                        instruction.jf as usize
                    }
                }
                BPF_JSET_K => {
                    pc += if accumulator & instruction.k != 0 {
                        instruction.jt as usize
                    } else {
                        instruction.jf as usize
                    }
                }
                BPF_RET_K => return instruction.k,
                other => panic!("unexpected opcode {other:#x}"),
            }
            pc += 1;
        }
    }

    fn run_filter(nr: libc::c_long, args0: u32) -> u32 {
        run_filter_for(Profile::Documents, nr, args0)
    }

    /// Evaluate the media profile's filter, which is the same list with the
    /// helper's calls added back.
    fn run_media_filter(nr: libc::c_long, args0: u32) -> u32 {
        run_filter_for(Profile::Media, nr, args0)
    }

    fn run_filter_for(profile: Profile, nr: libc::c_long, args0: u32) -> u32 {
        let (program, _) = filter_program(profile).expect("a program for this architecture");
        evaluate(
            &program,
            AUDIT_ARCH.expect("an architecture with a filter"),
            nr as u32,
            args0,
        )
    }

    /// Everything the parsers legitimately do must pass, whatever the first
    /// argument holds.
    #[test]
    fn the_filter_allows_what_the_parsers_need() {
        for nr in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_close,
            libc::SYS_mmap,
            libc::SYS_mprotect,
            libc::SYS_munmap,
            libc::SYS_brk,
            libc::SYS_futex,
            libc::SYS_getrandom,
            libc::SYS_clock_gettime,
            libc::SYS_rt_sigaction,
            libc::SYS_exit_group,
        ] {
            assert_eq!(
                run_filter(nr, 1),
                SECCOMP_RET_ALLOW,
                "syscall {nr} must pass"
            );
        }
    }

    /// The one allowed call that reads its first argument: the limits may be
    /// stated for this process and for no other. A document that could set the
    /// server's limits would have reached the process the sandbox protects.
    #[test]
    fn only_this_process_limits_may_be_stated() {
        assert_eq!(
            run_filter(libc::SYS_prlimit64, 0),
            SECCOMP_RET_ALLOW,
            "glibc's setrlimit is this call, and the child sets its own"
        );
        assert_eq!(
            run_filter(libc::SYS_prlimit64, 1),
            SECCOMP_RET_ERRNO | EPERM,
            "another process's limits are not this process's to state"
        );
    }

    /// The calls Landlock cannot govern are refused here instead, which is the
    /// division of labour the module documents: paths are Landlock's, syscalls
    /// are this filter's.
    #[test]
    fn the_calls_landlock_cannot_govern_are_refused() {
        for (name, nr) in [
            ("ftruncate", libc::SYS_ftruncate),
            ("fchmodat", libc::SYS_fchmodat),
            ("fchownat", libc::SYS_fchownat),
            ("fsetxattr", libc::SYS_fsetxattr),
            ("utimensat", libc::SYS_utimensat),
            ("newfstatat", libc::SYS_newfstatat),
            ("readlinkat", libc::SYS_readlinkat),
            ("getdents64", libc::SYS_getdents64),
            ("faccessat2", libc::SYS_faccessat2),
        ] {
            assert_eq!(
                run_filter(nr, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "{name} is a Landlock gap and must be refused"
            );
        }
    }

    /// Signals are decided by their target, which a classic BPF filter cannot
    /// read — so the calls are refused outright, and ABI 6's `LANDLOCK_SCOPE_*`
    /// is what makes the same refusal precise rather than total.
    #[test]
    fn a_signal_cannot_be_sent_at_all() {
        for (name, nr) in [
            ("kill", libc::SYS_kill),
            ("tkill", libc::SYS_tkill),
            ("tgkill", libc::SYS_tgkill),
            ("pidfd_open", libc::SYS_pidfd_open),
            ("pidfd_send_signal", libc::SYS_pidfd_send_signal),
        ] {
            assert_eq!(
                run_filter(nr, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "{name} reaches a process outside the sandbox"
            );
        }
    }

    /// The path-taking forms the old tables kept exist on the architectures
    /// that have them, and are refused there too.
    #[test]
    fn the_legacy_path_calls_are_refused_where_they_exist() {
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        for (name, nr) in [
            ("chmod", libc::SYS_chmod),
            ("chown", libc::SYS_chown),
            ("stat", libc::SYS_stat),
            ("lstat", libc::SYS_lstat),
            ("access", libc::SYS_access),
            ("readlink", libc::SYS_readlink),
            ("utime", libc::SYS_utime),
        ] {
            assert_eq!(
                run_filter(nr, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "{name} is a Landlock gap and must be refused"
            );
        }
    }

    #[test]
    fn the_filter_refuses_what_it_names() {
        for nr in [
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_execve,
            libc::SYS_ptrace,
            libc::SYS_bpf,
            libc::SYS_userfaultfd,
            libc::SYS_mount,
        ] {
            assert_eq!(
                run_filter(nr, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "syscall {nr} must be refused"
            );
        }
    }

    /// `clone3` is answered with `ENOSYS` rather than refused, so a runtime
    /// falls back to the `clone` whose flags this filter can read.
    #[test]
    fn the_filter_answers_clone3_with_enosys() {
        assert_eq!(run_filter(libc::SYS_clone3, 0), SECCOMP_RET_ERRNO | ENOSYS);
    }

    /// A thread must survive the filter, and a clone without `CLONE_THREAD` —
    /// how a process is made — must not.
    #[test]
    fn the_filter_distinguishes_a_thread_from_a_process() {
        #[cfg(target_endian = "little")]
        {
            assert_eq!(
                run_filter(libc::SYS_clone, CLONE_THREAD),
                SECCOMP_RET_ALLOW,
                "a thread must be allowed"
            );
            assert_eq!(
                run_filter(libc::SYS_clone, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "a process must not"
            );
            // The thread flag plus whatever else `std` passes with it.
            assert_eq!(
                run_filter(libc::SYS_clone, CLONE_THREAD | 0x10 | 0x80000),
                SECCOMP_RET_ALLOW
            );
        }
    }

    /// A syscall from the other entry point is killed rather than judged by
    /// numbers that mean something else there.
    #[test]
    fn the_filter_kills_a_foreign_architecture() {
        let Some((program, _)) = filter_program(Profile::Documents) else {
            return;
        };
        assert_eq!(
            evaluate(&program, 0x4000_0003, libc::SYS_read as u32, 0),
            SECCOMP_RET_KILL_PROCESS
        );
    }

    /// The media profile adds back exactly what a second, dynamically linked
    /// program needs — and keeps every refusal that is not about being that
    /// program.
    #[test]
    fn the_media_profile_allows_the_helper_calls_and_nothing_more() {
        for (name, nr) in [
            ("execve", libc::SYS_execve),
            ("newfstatat", libc::SYS_newfstatat),
            ("statx", libc::SYS_statx),
            ("readlinkat", libc::SYS_readlinkat),
            ("faccessat2", libc::SYS_faccessat2),
        ] {
            assert_eq!(
                run_media_filter(nr, 0),
                SECCOMP_RET_ALLOW,
                "{name} must pass for the helper"
            );
            assert_eq!(
                run_filter(nr, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "{name} must stay refused for a parser"
            );
        }

        // The helper may make a process; a parser may not. The files grant is
        // still what decides which program an `exec` can reach.
        #[cfg(target_endian = "little")]
        assert_eq!(
            run_media_filter(libc::SYS_clone, 0),
            SECCOMP_RET_ALLOW,
            "the helper is a second process"
        );

        // Everything that reaches off this machine or at another process stays
        // refused, for the helper as much as for a parser.
        for (name, nr) in [
            ("socket", libc::SYS_socket),
            ("connect", libc::SYS_connect),
            ("ptrace", libc::SYS_ptrace),
            ("mount", libc::SYS_mount),
            ("kill", libc::SYS_kill),
        ] {
            assert_eq!(
                run_media_filter(nr, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "{name} must stay refused for the helper"
            );
        }

        assert!(
            denied_syscalls(Profile::Media).len() < denied_syscalls(Profile::Documents).len(),
            "the media list is the document list with calls removed"
        );
    }
}
