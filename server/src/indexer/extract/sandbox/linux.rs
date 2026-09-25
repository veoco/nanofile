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

use super::Layers;

// ── prctl ───────────────────────────────────────────────────────────────────
const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;
const PR_CAP_AMBIENT: libc::c_int = 47;
const PR_CAP_AMBIENT_CLEAR_ALL: libc::c_ulong = 4;

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
/// Newest ABI this file knows about; the mask scales down to the running one.
const LANDLOCK_MAX_ABI: i64 = 5;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
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
pub(super) fn confine() -> (Layers, Vec<String>) {
    let mut layers = Layers::default();
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

    detail.push(drop_capabilities().to_string());

    match install_landlock() {
        Ok(abi) => {
            layers.files = true;
            detail.push(format!("landlock_abi={abi}"));
        }
        Err(reason) => detail.push(format!("landlock={reason}")),
    }

    if install_seccomp() {
        layers.network = true;
        layers.process = true;
        detail.push("seccomp=on".to_string());
    } else {
        detail.push("seccomp=off".to_string());
    }

    (layers, detail)
}

/// Close every descriptor above stderr.
///
/// The parent's descriptors are close-on-exec where the standard library
/// created them, but a parser exploited for a file read would still rather be
/// handed one than have to open it. `close_range` cannot be raced against a
/// concurrent open; the loop is the fallback for kernels before 5.9.
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
    for descriptor in 3..1024 {
        unsafe { libc::close(descriptor) };
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

/// Install a Landlock ruleset that grants nothing.
///
/// Every filesystem access the running kernel's ABI can govern is handled, and
/// no path-beneath rule is added: on Linux an unhandled access is simply
/// allowed, so handling the full mask *is* the denial. This is stricter than
/// the launcher this mirrors, which grants read access to `/` because it wraps
/// commands that need a filesystem; this process gets its document on stdin.
fn install_landlock() -> Result<i64, &'static str> {
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

    let attr = LandlockRulesetAttr {
        handled_access_fs: fs_mask_for_abi(abi),
    };
    let ruleset = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const LandlockRulesetAttr,
            size_of::<LandlockRulesetAttr>() as libc::size_t,
            0 as libc::c_uint,
        )
    };
    if ruleset < 0 {
        return Err("ruleset");
    }

    let restricted =
        unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0 as libc::c_uint) };
    unsafe { libc::close(ruleset as libc::c_int) };
    if restricted != 0 {
        return Err("restrict");
    }
    Ok(abi.min(LANDLOCK_MAX_ABI))
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

/// Install the seccomp filter, returning whether the kernel accepted it.
fn install_seccomp() -> bool {
    let Some(program) = filter_program() else {
        return false;
    };
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

/// The filter: allow everything the parser needs, refuse the rest with `EPERM`.
///
/// A denylist rather than an allowlist, following Codex's Linux sandbox: an
/// allowlist has to enumerate every syscall a Rust runtime and two parsers may
/// legitimately make, and a name missing from it turns into a document that
/// cannot be read. `EPERM` rather than a kill is the same choice: a denied
/// call is an error the parser can survive, not a crash to attribute later.
///
/// Returns `None` on an architecture whose audit code this file does not carry.
fn filter_program() -> Option<Vec<SockFilter>> {
    let arch = AUDIT_ARCH?;
    let mut program = Vec::with_capacity(4 + 2 * denied_syscalls().len());

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
    for number in denied_syscalls() {
        program.push(jump(BPF_JEQ_K, number as u32, 0, 1));
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
    #[cfg(target_endian = "little")]
    {
        program.push(jump(BPF_JEQ_K, libc::SYS_clone as u32, 0, 3));
        program.push(stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARGS0_LOW));
        program.push(jump(BPF_JSET_K, CLONE_THREAD, 1, 0));
        program.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    program.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
    Some(program)
}

/// The syscalls the child never needs.
fn denied_syscalls() -> Vec<libc::c_long> {
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
    ];

    // `fork` and `vfork` only exist where the kernel has them; the architectures
    // that do not (aarch64, loongarch64) reach process creation through `clone`,
    // which the argument check covers.
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        denied.push(libc::SYS_fork);
        denied.push(libc::SYS_vfork);
    }

    denied
}

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
        let Some(program) = filter_program() else {
            return; // an architecture this file does not carry
        };

        assert_eq!(program[0].code, BPF_LD_W_ABS);
        assert_eq!(program[0].k, SECCOMP_DATA_ARCH);
        assert_eq!(program[1].code, BPF_JEQ_K);
        assert_eq!(program[1].k, AUDIT_ARCH.expect("checked above"));
        assert_eq!(program[2].k, SECCOMP_RET_KILL_PROCESS);
        assert_eq!(program.last().expect("non-empty").k, SECCOMP_RET_ALLOW);

        for number in denied_syscalls() {
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
        let denied = denied_syscalls();
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
        let program = filter_program().expect("a program for this architecture");
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
            libc::SYS_prlimit64,
        ] {
            assert_eq!(
                run_filter(nr, 1),
                SECCOMP_RET_ALLOW,
                "syscall {nr} must pass"
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
        let Some(program) = filter_program() else {
            return;
        };
        assert_eq!(
            evaluate(&program, 0x4000_0003, libc::SYS_read as u32, 0),
            SECCOMP_RET_KILL_PROCESS
        );
    }
}
