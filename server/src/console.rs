//! Console handling for Windows GUI-subsystem builds.

/// Best-effort reattachment to the console of the launching terminal, so
/// interactive subcommands (`adduser`) can show prompts. A GUI-subsystem build
/// may own no console even when started from a terminal; with no parent console
/// (double-click, Run key) it fails silently.
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

        // Nothing to attach to (double-click, Run key, plain console build).
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
