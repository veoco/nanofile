//! Linux confinement: resource limits, capabilities, Landlock and seccomp.
//!
//! Everything is a raw syscall through `libc::syscall`, with the kernel UAPI defined
//! locally: the ABI is stable by contract, the definitions are the audit record of
//! which surface this code touches, and the build depends on neither the toolchain's
//! headers nor the `libc` vintage.
//!
//! Order matters and is not interchangeable:
//!
//! 1. close every descriptor above stderr;
//! 2. clamp the resource limits, soft equal to hard, so the child cannot raise them;
//! 3. `PR_SET_NO_NEW_PRIVS`, which lets an unprivileged process install Landlock and
//!    seccomp and neutralises setuid escalation;
//! 4. drop every capability, and read them back;
//! 5. Landlock, with no grants — the document arrives on stdin;
//! 6. seccomp last, because Landlock's own syscalls must happen first.

use std::mem::size_of;
use std::path::{Path, PathBuf};

use super::{Grants, Profile, Protections, shebang_interpreter};

const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;
const PR_CAP_AMBIENT: libc::c_int = 47;
const PR_CAP_AMBIENT_CLEAR_ALL: libc::c_ulong = 4;
/// `PR_SET_MDWE` and its one flag, from `linux/prctl.h` (Linux 6.3).
const PR_SET_MDWE: libc::c_int = 65;
const PR_MDWE_REFUSE_EXEC_GAIN: libc::c_ulong = 1 << 0;

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

const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
/// Newest ABI this file knows about.
///
/// Six rather than five: the scope bits are the ABI-6 addition, and the report
/// carries the ABI actually used rather than this one.
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

/// IPC scopes (ABI 6): a domain cannot signal or reach an abstract socket outside
/// itself.
///
/// This closes the hole a denylist cannot: `kill` is judged by its *target*, which a
/// classic BPF filter cannot read, and Landlock decides it against the domain.
const LL_SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const LL_SCOPE_SIGNAL: u64 = 1 << 1;

const SECCOMP_SET_MODE_FILTER: libc::c_uint = 1;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JEQ_K: u16 = 0x15;
const BPF_JGE_K: u16 = 0x35;
const BPF_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;

/// Offset of `nr`, `arch` and the low half of `args[0]` in `struct seccomp_data`.
const SECCOMP_DATA_NR: u32 = 0;
const SECCOMP_DATA_ARCH: u32 = 4;
const SECCOMP_DATA_ARGS0_LOW: u32 = 16;

/// `__X32_SYSCALL_BIT` from `arch/x86/include/uapi/asm/unistd.h`.
///
/// An x32 call carries this bit in `nr` but reports `AUDIT_ARCH_X86_64`, so the
/// architecture check cannot separate it and every denylist number below is 64-bit.
/// `seccomp(2)` names this as the bypass of an `nr`-keyed denylist.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

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
/// The arch check stops a 32-bit compatibility entry point from bypassing the
/// filter; it does not separate x86-64 from x32, which report the same value.
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

/// `newfstatat`'s number, which `libc` has no constant for on `loongarch64`.
///
/// That target's module lists 301 syscalls and misses this one, while `aarch64` and
/// `riscv64` give 79; all three read the same `asm-generic` table and the entries
/// either side agree, so 79 is this target's number too.
#[cfg(target_arch = "loongarch64")]
const SYS_NEWFSTATAT: libc::c_long = 79;
#[cfg(not(target_arch = "loongarch64"))]
const SYS_NEWFSTATAT: libc::c_long = libc::SYS_newfstatat;

/// `fchmodat2`'s number, which `libc` 0.2.189 carries only for some targets.
///
/// Linux 6.6 added it as 452 in `asm-generic`, and `x86_64` agrees, but the other
/// targets' `libc` modules do not define it. It closes the one metadata write Landlock
/// cannot govern that `fchmod` and `fchmodat` do not cover.
const SYS_FCHMODAT2: libc::c_long = 452;

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
    // Both are free and both close a way for a same-user process to reach this one:
    // `PR_SET_DUMPABLE` takes away `/proc/<pid>/mem`, and the parent-death signal ends a
    // parser whose parent is gone. Neither is a layer; the report says what took.
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
                detail.push(format!("helper_scope={}", landlock.helper_scope));
                if !landlock.helper {
                    // The one program this profile exists to run is not
                    // granted, whatever else the ruleset took.
                    detail.push("helper-rule-refused".to_string());
                }
                // The helper starts by copying this process, so the filter that denies
                // `clone` without `CLONE_THREAD` cannot be installed here, and
                // `RLIMIT_NPROC` counts per real user id too. The parent's timeout and the
                // process group it kills are what bound the copies, and the report says so.
                detail.push("media_process=unbounded".to_string());
            }
        }
        Err(reason) => detail.push(format!("landlock={reason}")),
    }

    match install_seccomp(profile) {
        Some(denied) => {
            layers.network = true;
            layers.process = true;
            detail.push(format!("seccomp={denied}"));
            // The media filter re-allows the metadata calls the loader makes, so its reachable
            // paths are wider than the parser profiles'; the page says so beside `ll_gaps=closed`.
            if profile.runs_helper() {
                detail.push("metadata=open".to_string());
            }
            // Landlock sees opens and creations, not a metadata change, a truncation before ABI 3
            // or a signal's target: those are the filter's job, so a host where it did not install
            // has a filesystem layer with holes rather than a missing one.
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
/// A same-user process could otherwise read it through `/proc/<pid>/mem` and attach.
/// Nothing here defends against same-user processes in general; this is the difference
/// between "the sandbox does not stop your neighbour" and "your neighbour reads the
/// document".
fn set_dumpable() -> bool {
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0, 0, 0) == 0 }
}

/// Ask the kernel to end this process when its parent ends: the parent's timeout and
/// group kill cover everything but the parent itself being gone.
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
/// The standard library's descriptors are close-on-exec, but an exploited parser would
/// rather be handed one. `close_range` cannot race a concurrent open; the loop is the
/// pre-5.9 fallback, and it walks to this process's own limit so a raised
/// `RLIMIT_NOFILE` cannot hide one.
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
/// The classic exploitation step, closed at the kernel rather than in the parser. It is
/// inherited by a helper the media profile starts, which still maps its libraries from
/// files that are never writable. A kernel older than 6.3 answers `EINVAL`, reported as
/// `mdwe=off`: the protection is additive and the items decide the confinement.
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

/// Drop every capability the process holds, then read the sets back: a no-op for an
/// unprivileged process, and the reason a server started as root does not hand root to
/// a parser. Dropping is always permitted; only adding needs `CAP_SETPCAP`.
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
    /// The shape the media profile's read grants took, as the report spells it.
    ///
    /// A count says how many rules took, not their breadth; `libs` is the helper, its
    /// interpreter, the libraries beside it and the system library trees.
    helper_scope: &'static str,
    /// Whether the rule for the helper file itself was added.
    ///
    /// The count cannot say: a helper that cannot be opened leaves the library and
    /// interpreter rules in place, so `helper_grants` is non-zero while the program this
    /// profile exists to run is not granted at all.
    helper: bool,
}

/// Install a Landlock ruleset that grants nothing — except what the media profile was
/// handed.
///
/// Handling every access the ABI can govern *is* the denial: on Linux an unhandled
/// access is allowed, so the document and image children, which get their bytes on
/// stdin, need no path rule.
///
/// The media profile is the exception: it executes a helper, so the helper, its
/// interpreter and the libraries it maps must be readable, and the scratch source must
/// be readable. Those are `path-beneath` rules on exact paths, never a directory the
/// parent chose.
///
/// Rights are added as the ABI has them (a field the kernel does not know is rejected):
/// the filesystem mask, the two TCP rights from ABI 4, and the IPC scopes from ABI 6. A
/// refused ruleset is retried with the filesystem mask alone, rather than losing the
/// strongest protection over a guessed bit.
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
    let mut helper = false;
    if profile.runs_helper() {
        let rules = add_helper_rules(ruleset, grants);
        helper_grants = rules.added;
        helper = rules.helper;
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
        // The one shape this platform's media grants take: libraries, listed one
        // file at a time. There is no package-manager tree to add here, which is
        // what separates it from the macOS profile's wording.
        helper_scope: "libs",
        helper,
    })
}

const LANDLOCK_RULE_PATH_BENEATH: libc::c_uint = 1;

/// One `landlock_path_beneath_attr`, packed the way the UAPI declares it.
#[repr(C, packed)]
struct LandlockPathBeneath {
    allowed_access: u64,
    parent_fd: i32,
}

/// The media profile's rules, and whether the helper itself got one.
struct HelperRules {
    added: usize,
    helper: bool,
}

/// Add the media profile's read and execute rules.
///
/// `execute` goes to exactly two things: the helper the parent named, and the
/// interpreter the kernel runs before its code. The directories around them get `read`
/// alone, because a Landlock grant is hierarchical: `execute` on the helper's directory
/// is `execute` on every program in it, the whole userland for a distribution ffmpeg.
///
/// The libraries *beside* the helper are granted one file at a time, as on Windows: a
/// packaged build's libraries are what the helper needs, and `read` on the directory
/// would be `read` on every file in it. The scratch source gets `read` alone, and a path
/// that is not there is skipped rather than failing the ruleset.
///
/// One residual the count does not state: Landlock checks `FS_EXECUTE` where a program
/// is opened, not where a mapping becomes executable, so a helper that invoked `ld.so`
/// on another program could run any ELF the library trees make readable. That stays
/// inside this profile — Landlock, seccomp and the limits still apply — so it is a
/// widening of what the helper may run, not a way out.
fn add_helper_rules(ruleset: i64, grants: Grants<'_>) -> HelperRules {
    let mut added = 0;
    let mut helper = false;
    if let Some(path) = grants.helper {
        helper = add_path_rule(ruleset, path, helper_file_access());
        added += helper as usize;
        for interpreter in helper_interpreters(path) {
            added += add_path_rule(ruleset, &interpreter, helper_interpreter_access()) as usize;
        }
        // The helper may sit beside its own libraries, and each one is a read.
        for library in helper_libraries(path) {
            added += add_path_rule(ruleset, &library, LL_READ_FILE) as usize;
        }
    }
    if let Some(source) = grants.source {
        added += add_path_rule(ruleset, source, LL_READ_FILE) as usize;
    }
    for directory in HELPER_LIBRARY_DIRS {
        added += add_path_rule(ruleset, Path::new(directory), helper_library_access()) as usize;
    }
    for file in HELPER_LIBRARY_FILES {
        added += add_path_rule(ruleset, Path::new(file), LL_READ_FILE) as usize;
    }
    // The devices a spawned program opens for itself: `/dev/null` for the
    // standard streams it is not given, and the entropy the runtime may want.
    for (path, access) in HELPER_DEVICES {
        added += add_path_rule(ruleset, Path::new(path), *access) as usize;
    }
    HelperRules { added, helper }
}

/// The shared libraries beside the helper, which a packaged build links against by an
/// `$ORIGIN`-relative path.
///
/// Named `*.so*` for the versioned form and the development symlink. An unlistable
/// directory yields nothing: the caller grants what it finds, and a helper that needed
/// more says so through `parse=` rather than through a silent widening.
fn helper_libraries(helper: &Path) -> Vec<PathBuf> {
    let Some(directory) = helper.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut libraries: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".so"))
        })
        .collect();
    libraries.sort();
    libraries
}

/// What the helper's own file is granted: run it, and read the image the kernel
/// is about to execute.
const fn helper_file_access() -> u64 {
    LL_EXECUTE | LL_READ_FILE
}

/// What the interpreter the kernel runs before the helper gets: a file, never a
/// directory, because it is the one program that has to be executable.
const fn helper_interpreter_access() -> u64 {
    LL_EXECUTE | LL_READ_FILE
}

/// What a system library tree gets: deliberately no `execute`, because a library is not
/// something this profile runs.
const fn helper_library_access() -> u64 {
    LL_READ_FILE | LL_READ_DIR
}

/// The programs the kernel runs before the helper's own code, which have to be
/// executable for it to be.
///
/// The static tree covers the glibc and musl interpreters this file pins; a script
/// helper names its own on its `#!` line. A path that is not there grants nothing.
fn helper_interpreters(helper: &Path) -> Vec<PathBuf> {
    let mut interpreters: Vec<PathBuf> = Vec::new();
    if let Some(named) = shebang_interpreter(helper) {
        interpreters.push(named);
    }
    for path in HELPER_INTERPRETERS {
        let path = PathBuf::from(path);
        if !interpreters.contains(&path) {
            interpreters.push(path);
        }
    }
    interpreters
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

/// The ELF interpreters, which the kernel executes out of the library trees: the trees
/// are for reading, and one interpreter file is what a dynamic helper cannot start
/// without.
const HELPER_INTERPRETERS: &[&str] = &[
    // glibc
    "/lib64/ld-linux-x86-64.so.2",
    "/lib/ld-linux.so.2",
    "/lib/ld-linux-aarch64.so.1",
    "/lib/ld-linux-armhf.so.3",
    "/lib/ld-linux-loongarch-lp64d.so.1",
    "/lib/ld-linux-riscv64-lp64d.so.1",
    // musl, which names its interpreter after the architecture
    "/lib/ld-musl-x86_64.so.1",
    "/lib/ld-musl-aarch64.so.1",
    "/lib/ld-musl-loongarch64.so.1",
    "/lib/ld-musl-riscv64.so.1",
];

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
/// The mask grows with the ABI: a bit the kernel does not know is rejected, so the
/// newest are added only where they exist.
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

/// Install the seccomp filter, returning how many syscalls it refuses: the count is in
/// the report because the list is what the layer is.
fn install_seccomp(profile: Profile) -> Option<usize> {
    let (program, denied) = filter_program(profile)?;
    install_program(&program).then_some(denied)
}

/// Install a filter that is already built.
///
/// Split from [`install_seccomp`] so the effect test can build the program in the
/// parent: this path allocates nothing, which makes it callable between `fork` and
/// `_exit`.
fn install_program(program: &[SockFilter]) -> bool {
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
    installed == 0
}

/// The filter, and how many syscalls it names.
///
/// A denylist rather than an allowlist: an allowlist must enumerate every call a Rust
/// runtime and two parsers may make, and a missing name turns into an unreadable
/// document. `EPERM` rather than a kill, so a denied call is an error the parser can
/// survive.
///
/// Two kinds of entry: what a parser must never do (sockets, starting programs, reading
/// another process), and what Landlock *cannot* govern (metadata changes, truncation
/// before ABI 3, a signal's target). `None` on an architecture whose audit code this
/// file does not carry.
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
    // x32 reports `AUDIT_ARCH_X86_64` and marks itself in `nr`, and every comparison below
    // is against a 64-bit number, so such a call would fall through to `ALLOW` — the bypass
    // `seccomp(2)` documents. Refusing the range closes it, and no legitimate syscall is
    // numbered near the bit.
    program.push(jump(BPF_JGE_K, X32_SYSCALL_BIT, 0, 1));
    program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    for number in &denied {
        program.push(jump(BPF_JEQ_K, *number as u32, 0, 1));
        program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    // `clone3` carries its flags behind a pointer, so no filter can tell a thread from a
    // process there. `ENOSYS` makes the runtime fall back to `clone`, whose flags can be
    // read; one that does not fall back sees an unsupported syscall, not a way through.
    program.push(jump(BPF_JEQ_K, libc::SYS_clone3 as u32, 0, 1));
    program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | ENOSYS));

    // `clone` stays reachable because the parsers run their work on a thread, but only with
    // `CLONE_THREAD`: without it, `clone` is how a process is made. The flags are in the low
    // half of the first argument, and the check is reached only when the syscall is `clone`,
    // because `args[0]` means something else everywhere else.
    //
    // The media profile is the exception: it makes a process to run the helper, so the
    // argument check is not installed; the files grant, the timeout and the group kill bound
    // it.
    #[cfg(target_endian = "little")]
    if !profile.runs_helper() {
        program.push(jump(BPF_JEQ_K, libc::SYS_clone as u32, 0, 3));
        program.push(stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARGS0_LOW));
        program.push(jump(BPF_JSET_K, CLONE_THREAD, 1, 0));
        program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    // `prlimit64` is glibc's `setrlimit` and also how a limit is set on another process.
    // Only the self form is allowed: `args[0] == 0` is the kernel's spelling of "this
    // process".
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
/// A denylist is only as good as the argument for each name, and there are two: "a
/// parser has no business doing this" and "Landlock cannot decide this". Both are
/// re-derivable, which keeps the list from drifting into folklore: a new name carries
/// its reason above it, and what the parsers actually call can be re-measured in
/// `SECCOMP_RET_LOG` mode or under `strace -f -c` and diffed against this list.
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
        // Reading another process, and the signal that would end it: the sandbox addresses
        // nothing outside itself. `pidfd_open` sends a signal without a pid race, and the
        // `rt_sig*queueinfo` pair is the third way one is sent.
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_send_signal,
        // Leaving the group the parent started this child in. Media may copy itself, so a copy
        // could otherwise `setsid`/`setpgid` out of reach of the parent's `killpg` and outlive
        // the request. A parser has no use for either call.
        libc::SYS_setsid,
        libc::SYS_setpgid,
        // The shape of the filesystem without reading a file: names, sizes and existence.
        // Landlock governs opens, so these reach past it and would map the machine.
        SYS_NEWFSTATAT,
        libc::SYS_statx,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
        libc::SYS_readlinkat,
        libc::SYS_getdents64,
        libc::SYS_chdir,
        libc::SYS_fchdir,
        libc::SYS_name_to_handle_at,
        // Watching the filesystem change, which is the metadata side of the same
        // reach: the parsers have nothing to watch and the server's own trees
        // are not theirs to observe.
        libc::SYS_inotify_init1,
        libc::SYS_inotify_add_watch,
        libc::SYS_fanotify_init,
        // The calls Landlock documents as ungovernable: metadata changes and truncation. The
        // document arrives on stdin and nothing is written, so the syscall is where they are
        // refused, including `fchmodat2` under its newer number.
        libc::SYS_ftruncate,
        libc::SYS_truncate,
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        SYS_FCHMODAT2,
        libc::SYS_fchown,
        libc::SYS_fchownat,
        libc::SYS_fsetxattr,
        libc::SYS_lsetxattr,
        libc::SYS_setxattr,
        libc::SYS_fremovexattr,
        libc::SYS_lremovexattr,
        libc::SYS_removexattr,
        libc::SYS_utimensat,
        // A mount's attributes are the same class of ungovernable metadata, and
        // the call is not one a parser or a helper has any business making.
        libc::SYS_mount_setattr,
    ];

    // The bare path-taking forms, on the architectures that kept the old table: the `*at`
    // forms above reach the same objects elsewhere, and a number a kernel does not have
    // cannot be denied and does not need to be.
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

    // The media profile adds back what a second, dynamically linked program needs: the
    // fork/exec pair, the loader's mapping calls, and the pipe/dup that connect it. Each is
    // bounded by the files grant (`exec` and `open` are Landlock's) or leaks only metadata;
    // dropping the metadata calls is the one visible weakening, and it lets a compromised
    // helper see which paths exist, never read one.
    if profile.runs_helper() {
        denied.retain(|number| !MEDIA_ALLOWED.contains(number));
        // The bare `fork`/`vfork` pair where the architecture has them: the runtime's spawn uses
        // one, and `clone` is already allowed here.
        #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
        denied.retain(|number| *number != libc::SYS_fork && *number != libc::SYS_vfork);
    }

    denied
}

/// The calls the media profile adds back to the denylist: each is here because a helper
/// that is a separate program needs it, and Landlock still decides what `open` or `exec`
/// may reach.
const MEDIA_ALLOWED: &[libc::c_long] = &[
    libc::SYS_execve,
    libc::SYS_execveat,
    // What the dynamic loader does before `main`: stat, readlink and access on
    // the search path, and the cache it reads. Landlock does not govern these,
    // so allowing them makes paths visible but not readable.
    SYS_NEWFSTATAT,
    libc::SYS_statx,
    libc::SYS_readlinkat,
    libc::SYS_faccessat,
    libc::SYS_faccessat2,
    libc::SYS_getdents64,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// `execute` reaches the helper and the interpreter the kernel runs before
    /// it, and no directory. A grant on a directory is a grant on every program
    /// beneath it, which for `/usr/bin/ffmpeg` is the whole userland.
    #[test]
    fn only_the_helper_and_its_interpreter_may_be_executed() {
        assert_eq!(helper_file_access() & LL_EXECUTE, LL_EXECUTE);
        assert_eq!(helper_file_access() & LL_READ_FILE, LL_READ_FILE);
        assert_eq!(helper_interpreter_access() & LL_EXECUTE, LL_EXECUTE);
        let access = helper_library_access();
        assert_eq!(
            access & LL_EXECUTE,
            0,
            "a library tree must not be executable"
        );
        assert_eq!(access & LL_READ_FILE, LL_READ_FILE, "it must stay readable");
        assert_eq!(access & LL_READ_DIR, LL_READ_DIR, "and listable");
    }

    /// The libraries beside the helper are granted one file at a time, and only
    /// the ones that are libraries. A `read` grant on the directory itself would
    /// be a read grant on every file in it — which is what this replaced.
    #[test]
    fn the_libraries_beside_the_helper_are_named_one_by_one() {
        let directory = std::env::temp_dir().join(format!(
            "nanofile-libs-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("create");
        let helper = directory.join("ffmpeg");
        std::fs::write(&helper, b"#!/bin/sh\n").expect("write helper");
        std::fs::write(directory.join("libavcodec.so.61"), b"x").expect("write library");
        std::fs::write(directory.join("libavcodec.so"), b"x").expect("write link");
        // Not a library, and not what the directory grant reaches.
        std::fs::write(directory.join("secret.txt"), b"secret").expect("write secret");
        std::fs::create_dir_all(directory.join("subdir")).expect("create subdir");

        let found = helper_libraries(&helper);
        let names: Vec<String> = found
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
            .collect();
        assert!(names.contains(&"libavcodec.so.61".to_string()), "{names:?}");
        assert!(names.contains(&"libavcodec.so".to_string()), "{names:?}");
        assert!(!names.contains(&"secret.txt".to_string()), "{names:?}");
        assert!(!names.contains(&"subdir".to_string()), "{names:?}");
        assert!(!names.contains(&"ffmpeg".to_string()), "{names:?}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The interpreter list is the helper's own first, then the static tree,
    /// without repeating a path the helper already named.
    #[test]
    fn the_interpreter_list_carries_the_helper_s_own_first() {
        let path = std::env::temp_dir().join(format!(
            "nanofile-interp-{}-{:?}.sh",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"#! /lib64/ld-linux-x86-64.so.2\n").expect("write");

        let interpreters = helper_interpreters(&path);
        assert_eq!(
            interpreters.first(),
            Some(&PathBuf::from("/lib64/ld-linux-x86-64.so.2"))
        );
        assert_eq!(
            interpreters
                .iter()
                .filter(|path| *path == &PathBuf::from("/lib64/ld-linux-x86-64.so.2"))
                .count(),
            1,
            "a path the helper named is not listed twice"
        );
        assert!(
            interpreters
                .iter()
                .any(|path| path == &PathBuf::from("/lib/ld-linux-aarch64.so.1")),
            "the static tree is still there for a helper that names nothing"
        );

        let _ = std::fs::remove_file(&path);
        assert_eq!(
            helper_interpreters(Path::new("/nonexistent-helper")),
            HELPER_INTERPRETERS
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        );
    }

    /// Every architecture the media worker ships on names its loader here: x86_64, aarch64
    /// and loongarch64 on glibc and musl. A missing entry is invisible on any host that does
    /// not use that loader.
    #[test]
    fn every_shipped_linux_target_names_its_interpreter() {
        for (gnu, glibc, musl) in [
            (
                "x86_64-unknown-linux-gnu",
                "/lib64/ld-linux-x86-64.so.2",
                "/lib/ld-musl-x86_64.so.1",
            ),
            (
                "aarch64-unknown-linux-gnu",
                "/lib/ld-linux-aarch64.so.1",
                "/lib/ld-musl-aarch64.so.1",
            ),
            (
                "loongarch64-unknown-linux-gnu",
                "/lib/ld-linux-loongarch-lp64d.so.1",
                "/lib/ld-musl-loongarch64.so.1",
            ),
        ] {
            assert!(
                HELPER_INTERPRETERS.contains(&glibc),
                "{gnu}: the helper's loader {glibc} is not granted"
            );
            assert!(
                HELPER_INTERPRETERS.contains(&musl),
                "{}: the helper's loader {musl} is not granted",
                gnu.replace("-gnu", "-musl")
            );
        }
        // Referenced by this file, not shipped by the matrix.
        for loader in [
            "/lib/ld-linux-riscv64-lp64d.so.1",
            "/lib/ld-musl-riscv64.so.1",
        ] {
            assert!(HELPER_INTERPRETERS.contains(&loader), "{loader}");
        }
    }

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

    /// Evaluate the filter the way the kernel's BPF interpreter would: the tests are worth
    /// nothing if they inspect the program rather than run it.
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
                BPF_JGE_K => {
                    pc += if accumulator >= instruction.k {
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

    /// The x32 ABI reports `AUDIT_ARCH_X86_64` and marks itself in `nr`, so the architecture
    /// check cannot see it. Without the upper bound such a call reaches the final `ALLOW`:
    /// this is the bypass `seccomp(2)` documents.
    #[test]
    fn the_filter_refuses_the_x32_syscall_range() {
        for number in [
            // The calls the denylist refuses, reached through the other ABI.
            libc::SYS_socket,
            libc::SYS_execve,
            libc::SYS_ptrace,
            libc::SYS_bpf,
            libc::SYS_setsid,
            libc::SYS_clone,
            // The calls it allows: these are the bypass, not a defence.
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_mmap,
            libc::SYS_futex,
        ] {
            let x32 = X32_SYSCALL_BIT | number as u32;
            assert_eq!(
                run_filter(x32 as libc::c_long, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "x32 syscall {number} must be refused, not fall through"
            );
            assert_eq!(
                run_filter_for(Profile::Media, x32 as libc::c_long, 0),
                SECCOMP_RET_ERRNO | EPERM,
                "x32 syscall {number} must be refused for the media profile too"
            );
        }
        // The bit on its own is inside the range as well.
        assert_eq!(
            run_filter(X32_SYSCALL_BIT as libc::c_long, 0),
            SECCOMP_RET_ERRNO | EPERM
        );
        // The 64-bit numbers are untouched by the bound.
        assert_eq!(run_filter(libc::SYS_read, 0), SECCOMP_RET_ALLOW);
        assert_eq!(run_filter(libc::SYS_socket, 0), SECCOMP_RET_ERRNO | EPERM);
    }

    /// The calls Landlock cannot govern are refused here instead, which is the
    /// division of labour the module documents: paths are Landlock's, syscalls
    /// are this filter's.
    #[test]
    fn the_calls_landlock_cannot_govern_are_refused() {
        for (name, nr) in [
            ("ftruncate", libc::SYS_ftruncate),
            ("fchmodat", libc::SYS_fchmodat),
            ("fchmodat2", SYS_FCHMODAT2),
            ("fchownat", libc::SYS_fchownat),
            ("fsetxattr", libc::SYS_fsetxattr),
            ("utimensat", libc::SYS_utimensat),
            ("mount_setattr", libc::SYS_mount_setattr),
            ("newfstatat", SYS_NEWFSTATAT),
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

    /// The call that made the files layer bypassable: Landlock cannot govern a mode change,
    /// and `fchmodat2` is the spelling the older entries missed. Refused for every profile —
    /// an exploited helper could plant a setuid binary or take the server's data.
    #[test]
    fn a_mode_cannot_be_changed_by_any_spelling() {
        for profile in Profile::ALL {
            let denied = denied_syscalls(profile);
            for (name, nr) in [
                ("fchmod", libc::SYS_fchmod),
                ("fchmodat", libc::SYS_fchmodat),
                ("fchmodat2", SYS_FCHMODAT2),
            ] {
                assert!(
                    denied.contains(&nr),
                    "{name} must be refused for {}",
                    profile.as_str()
                );
                assert_eq!(
                    run_filter_for(profile, nr, 0),
                    SECCOMP_RET_ERRNO | EPERM,
                    "{name} must be refused for {}",
                    profile.as_str()
                );
            }
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
            ("rt_sigqueueinfo", libc::SYS_rt_sigqueueinfo),
            ("rt_tgsigqueueinfo", libc::SYS_rt_tgsigqueueinfo),
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

    /// A process that may copy itself must not leave the group the parent started it in:
    /// the helper's descendants are bounded by the timeout and `killpg`, and `setsid` /
    /// `setpgid` are how a copy steps outside. Both profiles refuse them.
    #[test]
    fn the_process_group_cannot_be_left() {
        for profile in Profile::ALL {
            for (name, nr) in [("setsid", libc::SYS_setsid), ("setpgid", libc::SYS_setpgid)] {
                assert!(
                    denied_syscalls(profile).contains(&nr),
                    "{name} must be refused for {}",
                    profile.as_str()
                );
                assert_eq!(
                    run_filter_for(profile, nr, 0),
                    SECCOMP_RET_ERRNO | EPERM,
                    "{name} must be refused for {}",
                    profile.as_str()
                );
            }
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
            ("newfstatat", SYS_NEWFSTATAT),
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

    /// One syscall, called the way an exploited parser would.
    #[derive(Debug, Clone, Copy)]
    enum Attempt {
        Socket,
        Setsid,
        Setpgid,
        Fork,
        MemfdCreate,
        PidfdOpen,
        Fchmodat2,
        Kill,
        Ptrace,
        ProcessVmReadv,
    }

    /// Call one, and answer with the errno it set — or `0` on success.
    ///
    /// It runs in a forked child, so it allocates nothing: raw syscalls and comparisons,
    /// with the errno read back by number.
    fn attempt(what: Attempt, path: *const libc::c_char) -> i32 {
        let result = unsafe {
            match what {
                Attempt::Socket => {
                    libc::syscall(libc::SYS_socket, libc::AF_INET, libc::SOCK_DGRAM, 0)
                }
                Attempt::Setsid => libc::syscall(libc::SYS_setsid),
                Attempt::Setpgid => libc::syscall(libc::SYS_setpgid, 0, 0),
                Attempt::Fork => {
                    libc::syscall(libc::SYS_clone, libc::SIGCHLD as libc::c_ulong, 0, 0, 0, 0)
                }
                Attempt::MemfdCreate => libc::syscall(libc::SYS_memfd_create, path, 0),
                Attempt::PidfdOpen => libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0),
                Attempt::Fchmodat2 => libc::syscall(
                    SYS_FCHMODAT2,
                    libc::AT_FDCWD,
                    path,
                    0o644 as libc::c_uint,
                    0,
                ),
                Attempt::Kill => libc::syscall(libc::SYS_kill, libc::getpid(), 0),
                Attempt::Ptrace => libc::syscall(
                    libc::SYS_ptrace,
                    libc::PTRACE_TRACEME as libc::c_uint,
                    0,
                    0,
                    0,
                ),
                Attempt::ProcessVmReadv => libc::syscall(
                    libc::SYS_process_vm_readv,
                    libc::getppid(),
                    std::ptr::null::<libc::iovec>(),
                    0 as libc::c_ulong,
                    std::ptr::null::<libc::iovec>(),
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                ),
            }
        };
        if result >= 0 {
            0
        } else {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        }
    }

    /// What one syscall answered in a child that installed `program`, or in a bare one.
    fn errno_for(program: Option<&[SockFilter]>, what: Attempt, path: *const libc::c_char) -> i32 {
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(
            unsafe { libc::pipe(fds.as_mut_ptr()) },
            0,
            "the parent's own pipe"
        );
        let (read_end, write_end) = (fds[0], fds[1]);

        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "the parent's own fork");
        if pid == 0 {
            let answer = match program {
                Some(program) if set_no_new_privs() && install_program(program) => {
                    attempt(what, path)
                }
                Some(_) => -1,
                None => attempt(what, path),
            };
            let bytes = answer.to_ne_bytes();
            unsafe {
                libc::write(write_end, bytes.as_ptr().cast(), bytes.len());
                libc::_exit(0);
            }
        }

        let _ = unsafe { libc::close(write_end) };
        let mut bytes = [0u8; 4];
        let mut read = 0usize;
        while read < bytes.len() {
            let got = unsafe {
                libc::read(
                    read_end,
                    bytes[read..].as_mut_ptr().cast(),
                    bytes.len() - read,
                )
            };
            if got <= 0 {
                break;
            }
            read += got as usize;
        }
        let _ = unsafe { libc::close(read_end) };
        let mut status: libc::c_int = 0;
        let _ = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(read, bytes.len(), "{what:?}: the child answered nothing");
        i32::from_ne_bytes(bytes)
    }

    /// Every refusal the filter is built for, against the running kernel.
    ///
    /// The simulator above says what the program *means*; it cannot say that the
    /// numbers in it are this kernel's numbers, because it compares each call
    /// against the same constant the filter was built from. This installs the
    /// real filter in a child and makes the calls, and then makes the same calls
    /// in the same kind of process without it: a refusal on both sides is this
    /// process's own privilege rather than the sandbox, so only a case where the
    /// two disagree is a case about the filter at all.
    #[test]
    fn the_filter_refuses_what_the_kernel_gives_it_to_refuse() {
        let Some((program, _)) = filter_program(Profile::Documents) else {
            return; // an architecture this file does not carry
        };
        let owned = std::env::temp_dir().join(format!("nanofile-fchmodat2-{}", std::process::id()));
        std::fs::write(&owned, b"x").expect("a file this process owns");
        let path = std::ffi::CString::new(owned.as_os_str().as_encoded_bytes())
            .expect("a path without NULs");

        for what in [
            Attempt::Socket,
            Attempt::Setsid,
            Attempt::Setpgid,
            Attempt::Fork,
            Attempt::MemfdCreate,
            Attempt::PidfdOpen,
            Attempt::Fchmodat2,
            Attempt::Kill,
            Attempt::Ptrace,
            Attempt::ProcessVmReadv,
        ] {
            let confined = errno_for(Some(&program), what, path.as_ptr());
            assert_ne!(
                confined, -1,
                "{what:?}: the child could not install the filter at all"
            );
            assert_eq!(
                confined,
                libc::EPERM,
                "{what:?} must be refused by the filter the child installed"
            );

            let bare = errno_for(None, what, path.as_ptr());
            assert_ne!(
                bare,
                libc::EPERM,
                "{what:?} is refused without the filter too, so this case says \
                 nothing about the sandbox"
            );
        }

        let _ = std::fs::remove_file(&owned);
    }
}
