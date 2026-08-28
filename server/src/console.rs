//! Console handling for Windows GUI-subsystem builds.

/// Best-effort reattachment to the console of the launching terminal.
///
/// Windows tray builds are GUI-subsystem binaries: double-clicking one (or
/// starting it from the Run key) creates no console at all, and even when
/// launched from `cmd`/PowerShell the process is neither attached to the
/// terminal nor waited on. Interactive subcommands (`adduser`) call this
/// first so prompts and results become visible when run from a terminal;
/// when there is no parent console (double-click) it fails silently.
pub fn attach_parent_console() {
    #[cfg(all(target_os = "windows", feature = "tray"))]
    unsafe {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
            STD_OUTPUT_HANDLE, SetStdHandle,
        };

        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        // Nothing to attach to (double-clicked, started by the Run key, or a
        // plain console build) — leave the standard handles alone.
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }

        let in_name = wide("CONIN$");
        let out_name = wide("CONOUT$");
        let std_in = CreateFileW(
            in_name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        let std_out = CreateFileW(
            out_name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        // The handles must stay open for the process lifetime — Rust's std
        // caches them, so they are deliberately never closed.
        if std_in != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_INPUT_HANDLE, std_in);
        }
        if std_out != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, std_out);
            SetStdHandle(STD_ERROR_HANDLE, std_out);
        }
    }
}
