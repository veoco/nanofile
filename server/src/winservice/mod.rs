//! Windows service support: running nanofile under the Service Control Manager
//! (SCM), and registering/removing that service.
//!
//! Why a service: the launch-at-login entry (`HKCU\…\Run`) only starts when
//! somebody logs in. A service starts at boot with no session at all, which is
//! what a file-sync server wants on a machine nobody sits in front of.
//!
//! Shape of this module:
//!
//! * The command line, the comparison against a registered `ImagePath` and the
//!   action enum are platform-independent, so their unit tests run everywhere
//!   (`cargo test -p server --bin nanofile`).
//! * Everything that talks to the SCM or the registry is `#[cfg(windows)]`.
//! * The service *registers* itself; it never starts or stops itself. Switching
//!   from a logged-in tray instance to the service therefore takes effect at the
//!   next system start: handing the listening port over immediately is not
//!   reliable on Windows, because a port whose connections were just closed
//!   stays in `TIME_WAIT` for minutes and the new process would fail to bind it.

use std::path::Path;

/// The SCM name of the service. Fixed (not derived from the install path) so
/// `net stop Nanofile`/`sc query Nanofile` are predictable, and so the tray can
/// find the service it manages. One service per machine.
#[allow(dead_code)] // only read by the Windows backend
pub(crate) const SERVICE_NAME: &str = "Nanofile";

/// What the service is called in `services.msc`.
#[allow(dead_code)] // only read by the Windows backend
pub(crate) const DISPLAY_NAME: &str = "Nanofile Sync Server";

/// What `nanofile service …` can be asked to do.
#[cfg(target_os = "windows")]
#[derive(clap::Subcommand, Debug)]
pub(crate) enum ServiceAction {
    /// Run as a service. Called by the Service Control Manager (the registered
    /// binary path), never by hand: without the SCM this exits immediately.
    Run,
    /// Register the auto-start service (or update its command line). Needs
    /// administrator rights.
    Install,
    /// Stop and remove the service. Needs administrator rights.
    Uninstall,
    /// Report whether the service is registered. Exit code 0 when it is.
    Status,
}

/// What is registered under [`SERVICE_NAME`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // only constructed by the Windows backend
pub(crate) enum ServiceProbe {
    /// No service with our name.
    NotInstalled,
    /// Registered, but its binary path belongs to a different installation
    /// (another copy of nanofile, or another config file).
    ///
    /// `missing` means the executable that path names no longer exists, which is
    /// what a moved or renamed installation leaves behind: the service then
    /// fails to start and nothing on this side says why. The replacement
    /// confirmation names it.
    OtherInstall {
        image_path: String,
        running: bool,
        missing: bool,
    },
    /// Registered for this executable and config file.
    Ours { running: bool },
    /// The SCM could not be asked (rights, or an unexpected error).
    Unknown,
}

#[allow(dead_code)] // only read by the Windows backend
impl ServiceProbe {
    pub fn is_ours(&self) -> bool {
        matches!(self, ServiceProbe::Ours { .. })
    }

    pub fn is_running(&self) -> bool {
        match self {
            ServiceProbe::Ours { running } | ServiceProbe::OtherInstall { running, .. } => *running,
            _ => false,
        }
    }

    /// Whether the registered executable is gone (a moved or renamed install).
    pub fn is_broken(&self) -> bool {
        match self {
            ServiceProbe::OtherInstall { missing, .. } => *missing,
            _ => false,
        }
    }

    pub fn is_installed(&self) -> bool {
        !matches!(self, ServiceProbe::NotInstalled | ServiceProbe::Unknown)
    }
}

// ── Platform-independent helpers ─────────────────────────────────────────────

/// Quote one argument for a Windows command line.
///
/// Only double quotes need escaping (backslashes are literal except immediately
/// before a quote, and doubling them would corrupt plain paths) — the same rule
/// the launch-at-login entry uses.
#[allow(dead_code)] // only read by the Windows backend
fn win_cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// The command line registered as the service's binary path: the same
/// executable, told explicitly which config file to use (a service starts with
/// a working directory of `C:\Windows\System32`, where a relative `config.toml`
/// means nothing).
#[allow(dead_code)] // only read by the Windows backend
pub(crate) fn service_command_line(exe: &Path, config: &Path) -> String {
    format!(
        "{} service run --config {}",
        win_cmd_quote(&exe.to_string_lossy()),
        win_cmd_quote(&config.to_string_lossy())
    )
}

/// The service's description in `services.msc`.
#[allow(dead_code)] // only read by the Windows backend
pub(crate) fn service_description(config: &Path) -> String {
    format!(
        "Nanofile — a Seafile-compatible file sync server. Starts at boot without a user \
         login. Configuration: {}",
        config.display()
    )
}

/// Normalize an `ImagePath` for comparison: no surrounding quotes, one folder
/// separator, no case difference (Windows paths are case-insensitive).
#[allow(dead_code)] // only read by the Windows backend
pub(crate) fn normalize_image_path(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .replace('/', "\\")
        .to_lowercase()
}

/// The executable an `ImagePath` (or any command line of ours) names: the first
/// quoted token when the path is quoted, otherwise everything up to the first
/// space.
///
/// It is only used to ask "does the file this registration launches still
/// exist?", so a hand-edited value that parses oddly can only make the answer
/// less precise — never destructive.
#[allow(dead_code)] // only read by the Windows backend
pub(crate) fn executable_in(command_line: &str) -> Option<String> {
    let text = command_line.trim();
    let token = match text.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => text.split_whitespace().next()?,
    };
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Whether a registered `ImagePath` is the command line this installation would
/// write.
///
/// Not an exact-equality test on purpose: a service registered by hand (or by an
/// older build) may carry fewer arguments, and reporting "someone else's
/// install" for a service that starts exactly this binary would be wrong. What
/// has to differ for the answer to be "not ours" is the executable or the
/// config file, and both show up as a difference in the middle of the string.
#[allow(dead_code)] // only read by the Windows backend
pub(crate) fn image_path_matches(registered: &str, expected: &str) -> bool {
    let registered = normalize_image_path(registered);
    let expected = normalize_image_path(expected);
    !registered.is_empty()
        && !expected.is_empty()
        && (registered == expected
            || registered.starts_with(&expected)
            || expected.starts_with(&registered))
}

// ── Windows implementation ───────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use infra::config::{Config, EnvKeys};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE,
        ERROR_SERVICE_SPECIFIC_ERROR, NO_ERROR,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    use super::{
        DISPLAY_NAME, SERVICE_NAME, ServiceProbe, image_path_matches, service_command_line,
        service_description,
    };

    /// Where the SCM keeps a service's registration, read directly to compare
    /// the binary path. `QueryServiceConfigW` would work too, but reading the
    /// value is what a user without `SERVICE_QUERY_CONFIG` can still do.
    const SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\Nanofile";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    #[allow(dead_code)]
    fn message_box(title: &str, text: &str, flags: u32) -> i32 {
        let title = wide(title);
        let text = wide(text);
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) }
    }

    /// Report a failure the way a GUI-subsystem process can: a message box the
    /// user will actually see, plus the log file.
    fn report_failure(context: &str, error: &anyhow::Error) {
        tracing::error!("{context}: {error:#}");
        let title = "Nanofile";
        let body = format!("{context}.\n\n{error:#}");
        message_box(title, &body, MB_OK | MB_ICONERROR | MB_SETFOREGROUND);
    }

    // ── Running as a service ─────────────────────────────────────────────

    /// The config the `ServiceMain` callback (which is called by the SCM, on a
    /// thread this process does not control) has to pick up.
    static SERVICE_INPUT: std::sync::Mutex<Option<(Config, EnvKeys)>> = std::sync::Mutex::new(None);

    struct ServiceRuntime {
        /// `SERVICE_STATUS_HANDLE`, kept as an integer: a raw pointer is neither
        /// `Send` nor `Sync`, so it cannot live in a `static` directly.
        status_handle: isize,
        state: u32,
        exit_code: u32,
        quit: Option<tokio::sync::mpsc::UnboundedSender<crate::TrayCommand>>,
    }

    static SERVICE_RUNTIME: std::sync::Mutex<Option<ServiceRuntime>> = std::sync::Mutex::new(None);

    fn report(state: u32, exit_code: u32, wait_hint_ms: u32) {
        let mut guard = SERVICE_RUNTIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        runtime.state = state;
        if exit_code != 0 {
            runtime.exit_code = exit_code;
        }
        let accepted = if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_PRESHUTDOWN
        } else {
            0
        };
        let status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: accepted,
            dwWin32ExitCode: if runtime.exit_code == 0 {
                NO_ERROR
            } else {
                ERROR_SERVICE_SPECIFIC_ERROR
            },
            dwServiceSpecificExitCode: runtime.exit_code,
            dwCheckPoint: 0,
            dwWaitHint: wait_hint_ms,
        };
        unsafe { SetServiceStatus(runtime.status_handle as SERVICE_STATUS_HANDLE, &status) };
    }

    /// The SCM's control callback. Runs on a thread the SCM owns, so it only
    /// reports status and posts the stop request onto the server's channel.
    unsafe extern "system" fn control_handler(
        control: u32,
        _event_type: u32,
        _event_data: *mut c_void,
        _context: *mut c_void,
    ) -> u32 {
        match control {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN | SERVICE_CONTROL_PRESHUTDOWN => {
                report(SERVICE_STOP_PENDING, 0, 30_000);
                let quit = SERVICE_RUNTIME
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .and_then(|runtime| runtime.quit.clone());
                if let Some(quit) = quit {
                    let _ = quit.send(crate::TrayCommand::Quit);
                }
                NO_ERROR
            }
            // The SCM asks for the last reported status; answering is enough.
            SERVICE_CONTROL_INTERROGATE => NO_ERROR,
            _ => windows_sys::Win32::Foundation::ERROR_CALL_NOT_IMPLEMENTED,
        }
    }

    /// The service's entry point, called by the SCM on its own thread.
    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
        let name = wide(SERVICE_NAME);
        let handle = unsafe {
            RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), std::ptr::null())
        };
        if handle.is_null() {
            tracing::error!("the service control manager refused the handler");
            return;
        }
        {
            let mut guard = SERVICE_RUNTIME
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = Some(ServiceRuntime {
                status_handle: handle as isize,
                state: SERVICE_START_PENDING,
                exit_code: 0,
                quit: None,
            });
        }
        report(SERVICE_START_PENDING, 0, 30_000);

        let (quit_tx, quit_rx) = tokio::sync::mpsc::unbounded_channel();
        SERVICE_RUNTIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .expect("set above")
            .quit = Some(quit_tx);

        let input = SERVICE_INPUT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some((config, env_keys)) = input else {
            tracing::error!("the service was dispatched without a configuration");
            report(SERVICE_STOPPED, 1, 0);
            return;
        };

        report(SERVICE_RUNNING, 0, 0);
        tracing::info!("running as the Windows service '{SERVICE_NAME}'");

        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(e) => {
                tracing::error!("failed to create the tokio runtime: {e}");
                report(SERVICE_STOPPED, 1, 0);
                return;
            }
        };
        match runtime.block_on(crate::run_server_flow(config, env_keys, Some(quit_rx))) {
            Ok(()) => {
                tracing::info!("service stopped");
                report(SERVICE_STOPPED, 0, 0);
            }
            Err(e) => {
                tracing::error!("service failed: {e:#}");
                report(SERVICE_STOPPED, 1, 0);
            }
        }
    }

    /// Hand this process to the SCM. Blocks until the service stops.
    ///
    /// Fails immediately when the process was not started by the SCM, which is
    /// what a user typing `nanofile service run` gets — a clear error instead of
    /// a process that hangs waiting for a dispatcher that will never call.
    pub(crate) fn run_as_service(config: Config, env_keys: EnvKeys) -> anyhow::Result<()> {
        *SERVICE_INPUT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((config, env_keys));
        let name = wide(SERVICE_NAME);
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: name.as_ptr() as *mut u16,
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: std::ptr::null_mut(),
                lpServiceProc: None,
            },
        ];
        if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            anyhow::bail!(
                "not started by the Service Control Manager (error {error}); `nanofile service \
                 run` is only meaningful as the registered binary path of a service"
            );
        }
        Ok(())
    }

    // ── Install / uninstall / status ─────────────────────────────────────

    fn open_manager(access: u32) -> anyhow::Result<SC_HANDLE> {
        let handle = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
        if handle.is_null() {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if error == ERROR_ACCESS_DENIED {
                anyhow::bail!(
                    "access denied opening the service control manager; administrator rights are \
                     required"
                );
            }
            anyhow::bail!("opening the service control manager failed (error {error})");
        }
        Ok(handle)
    }

    /// Read the service's registered `ImagePath` from the registry.
    fn read_image_path() -> Option<String> {
        let subkey = wide(SERVICE_KEY);
        let value = wide("ImagePath");
        let mut size = 0u32;
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let probe = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if probe != NO_ERROR || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize) / 2 + 1];
        let read = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                flags,
                std::ptr::null_mut(),
                buffer.as_mut_ptr() as *mut c_void,
                &mut size,
            )
        };
        if read != NO_ERROR {
            return None;
        }
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        String::from_utf16(&buffer[..end]).ok()
    }

    /// What is registered under our service name, and whether it belongs to this
    /// installation.
    pub(crate) fn probe(config_path: &Path) -> ServiceProbe {
        let expected = match std::env::current_exe() {
            Ok(exe) => service_command_line(&exe, config_path),
            Err(_) => return ServiceProbe::Unknown,
        };
        let Ok(manager) = open_manager(SC_MANAGER_CONNECT) else {
            return ServiceProbe::Unknown;
        };
        let name = wide(SERVICE_NAME);
        let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS) };
        let mut running = false;
        let mut query_failed = false;
        if !service.is_null() {
            let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
            if unsafe { QueryServiceStatus(service, &mut status) } != 0 {
                running = status.dwCurrentState == SERVICE_RUNNING;
            } else {
                query_failed = true;
            }
            unsafe { CloseServiceHandle(service) };
        }
        unsafe { CloseServiceHandle(manager) };

        match read_image_path() {
            Some(path) => {
                if image_path_matches(&path, &expected) {
                    ServiceProbe::Ours { running }
                } else {
                    // A moved installation leaves the registration pointing at a
                    // file that is gone, and the service then fails to start
                    // with no nanofile UI left to explain it.
                    let missing = super::executable_in(&path)
                        .map(|exe| !Path::new(exe.as_str()).exists())
                        .unwrap_or(false);
                    ServiceProbe::OtherInstall {
                        image_path: path,
                        running,
                        missing,
                    }
                }
            }
            // No registry value: either the service does not exist, or it exists
            // and the value is unreadable. `OpenServiceW` failing to find it is
            // the ordinary "not installed" case.
            None => {
                if service.is_null() && !query_failed {
                    ServiceProbe::NotInstalled
                } else {
                    ServiceProbe::Unknown
                }
            }
        }
    }

    /// Create the service, or update an existing one's command line.
    pub(crate) fn install(config_path: &Path) -> anyhow::Result<()> {
        let exe = std::env::current_exe()
            .map_err(|e| anyhow::anyhow!("failed to locate the running executable: {e}"))?;
        let bin_path = service_command_line(&exe, config_path);
        let manager = open_manager(SC_MANAGER_ALL_ACCESS)?;
        let name = wide(SERVICE_NAME);
        let display = wide(DISPLAY_NAME);
        let bin = wide(&bin_path);

        let existing = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_CHANGE_CONFIG) };
        let service = if existing.is_null() {
            unsafe {
                CreateServiceW(
                    manager,
                    name.as_ptr(),
                    display.as_ptr(),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    bin.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            }
        } else {
            // Already registered (possibly for another install, which the user
            // confirmed): point it at this executable and config.
            let ok = unsafe {
                ChangeServiceConfigW(
                    existing,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    bin.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    display.as_ptr(),
                )
            };
            if ok == 0 {
                let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                unsafe {
                    CloseServiceHandle(existing);
                    CloseServiceHandle(manager);
                }
                anyhow::bail!("updating the service failed (error {error})");
            }
            let _ = unsafe { CloseServiceHandle(existing) };
            let reopened = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
            if reopened.is_null() {
                let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                unsafe { CloseServiceHandle(manager) };
                anyhow::bail!("re-opening the updated service failed (error {error})");
            }
            reopened
        };

        if service.is_null() {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            unsafe { CloseServiceHandle(manager) };
            anyhow::bail!("creating the service failed (error {error})");
        }

        let mut description = wide(&service_description(config_path));
        let description = SERVICE_DESCRIPTIONW {
            lpDescription: description.as_mut_ptr(),
        };
        let described = unsafe {
            ChangeServiceConfig2W(
                service,
                SERVICE_CONFIG_DESCRIPTION,
                &description as *const _ as *const c_void,
            )
        };
        if described == 0 {
            // Not fatal: the service works without a description.
            tracing::warn!("could not set the service description");
        }

        // Crashes and non-zero exits are retried with a growing delay, then left
        // stopped: `INFINITE` as the reset period is what stops a permanently
        // broken configuration from restarting forever.
        let mut actions = [
            SC_ACTION {
                Type: SC_ACTION_RESTART,
                Delay: 5_000,
            },
            SC_ACTION {
                Type: SC_ACTION_RESTART,
                Delay: 10_000,
            },
            SC_ACTION {
                Type: SC_ACTION_RESTART,
                Delay: 30_000,
            },
        ];
        let failures = SERVICE_FAILURE_ACTIONSW {
            dwResetPeriod: u32::MAX,
            lpRebootMsg: std::ptr::null_mut(),
            lpCommand: std::ptr::null_mut(),
            cActions: actions.len() as u32,
            lpsaActions: actions.as_mut_ptr(),
        };
        let configured = unsafe {
            ChangeServiceConfig2W(
                service,
                SERVICE_CONFIG_FAILURE_ACTIONS,
                &failures as *const _ as *const c_void,
            )
        };
        if configured == 0 {
            tracing::warn!("could not set the service failure actions");
        }
        let flag = SERVICE_FAILURE_ACTIONS_FLAG {
            // A clean stop (exit code 0) is still a clean stop; only a failure
            // triggers the actions above.
            fFailureActionsOnNonCrashFailures: 1,
        };
        if unsafe {
            ChangeServiceConfig2W(
                service,
                SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
                &flag as *const _ as *const c_void,
            )
        } == 0
        {
            tracing::warn!("could not set the non-crash failure flag");
        }

        unsafe {
            CloseServiceHandle(service);
            CloseServiceHandle(manager);
        }
        tracing::info!(bin_path = %bin_path, "service registered");
        Ok(())
    }

    /// Stop and remove the service. Removing a service that is not there is a
    /// success: the caller's intent ("it must not be registered") already holds.
    pub(crate) fn uninstall() -> anyhow::Result<()> {
        let manager = open_manager(SC_MANAGER_CONNECT)?;
        let name = wide(SERVICE_NAME);
        let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
        if service.is_null() {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            unsafe { CloseServiceHandle(manager) };
            if error == ERROR_SERVICE_DOES_NOT_EXIST {
                return Ok(());
            }
            anyhow::bail!("opening the service failed (error {error})");
        }

        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { QueryServiceStatus(service, &mut status) } != 0
            && status.dwCurrentState != SERVICE_STOPPED
        {
            let mut ignored: SERVICE_STATUS = unsafe { std::mem::zeroed() };
            let stopped = unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut ignored) };
            if stopped == 0 {
                let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                if error != ERROR_SERVICE_NOT_ACTIVE {
                    unsafe {
                        CloseServiceHandle(service);
                        CloseServiceHandle(manager);
                    }
                    anyhow::bail!("stopping the service failed (error {error})");
                }
            } else if !wait_for_stopped(service, std::time::Duration::from_secs(60)) {
                // Deleting a service that is still running only marks it for
                // deletion while it keeps serving, which would make "removed" a
                // lie. A graceful stop can legitimately take the server's
                // 25-second drain, so this is a real failure only after a
                // generous wait.
                unsafe {
                    CloseServiceHandle(service);
                    CloseServiceHandle(manager);
                }
                anyhow::bail!(
                    "the service did not report STOPPED within 60s; it is still installed and \
                     may still be serving"
                );
            }
        }

        let deleted = unsafe { DeleteService(service) };
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        unsafe {
            CloseServiceHandle(service);
            CloseServiceHandle(manager);
        }
        if deleted == 0 && error != ERROR_SERVICE_DOES_NOT_EXIST {
            anyhow::bail!("deleting the service failed (error {error})");
        }
        tracing::info!("service removed");
        Ok(())
    }

    /// Stop the running service and wait for it to report `STOPPED`.
    ///
    /// Idempotent: a service that is not running (or not installed at all) is
    /// already in the state the caller wants. Used by the tray's Quit, which in
    /// client mode means "stop the server", exactly as it does in the ordinary
    /// tray.
    ///
    /// Stopping a service needs the `SERVICE_STOP` right, so the caller has to
    /// handle the access-denied case rather than assume it.
    pub(crate) fn stop_service() -> anyhow::Result<()> {
        let manager = open_manager(SC_MANAGER_CONNECT)?;
        let name = wide(SERVICE_NAME);
        let service =
            unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_STOP | SERVICE_QUERY_STATUS) };
        if service.is_null() {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            unsafe { CloseServiceHandle(manager) };
            if error == ERROR_SERVICE_DOES_NOT_EXIST {
                return Ok(());
            }
            anyhow::bail!("opening the service failed (error {error})");
        }

        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { QueryServiceStatus(service, &mut status) } == 0 {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            unsafe {
                CloseServiceHandle(service);
                CloseServiceHandle(manager);
            }
            anyhow::bail!("querying the service failed (error {error})");
        }
        if status.dwCurrentState == SERVICE_STOPPED {
            unsafe {
                CloseServiceHandle(service);
                CloseServiceHandle(manager);
            }
            return Ok(());
        }

        let mut ignored: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut ignored) } == 0 {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            unsafe {
                CloseServiceHandle(service);
                CloseServiceHandle(manager);
            }
            if error == ERROR_SERVICE_NOT_ACTIVE {
                return Ok(());
            }
            anyhow::bail!("stopping the service failed (error {error})");
        }

        let stopped = wait_for_stopped(service, std::time::Duration::from_secs(60));
        unsafe {
            CloseServiceHandle(service);
            CloseServiceHandle(manager);
        }
        if !stopped {
            anyhow::bail!("the service did not report STOPPED within 60s");
        }
        tracing::info!("service stopped");
        Ok(())
    }

    /// Wait for the service to report `STOPPED`. `false` means it did not.
    fn wait_for_stopped(service: SC_HANDLE, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
            if unsafe { QueryServiceStatus(service, &mut status) } == 0 {
                return false;
            }
            if status.dwCurrentState == SERVICE_STOPPED {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                tracing::warn!("the service did not report STOPPED within {timeout:?}");
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    /// `nanofile service …` entry point.
    pub(crate) fn run_cli(
        action: super::ServiceAction,
        config: &Config,
        config_path: &Path,
        env_keys: EnvKeys,
    ) -> anyhow::Result<()> {
        match action {
            super::ServiceAction::Run => run_as_service(config.clone(), env_keys),
            super::ServiceAction::Install => match install(config_path) {
                Ok(()) => {
                    println!(
                        "Nanofile registered as a Windows service; it starts at the next system \
                         start, without a login."
                    );
                    Ok(())
                }
                Err(e) => {
                    report_failure("Nanofile could not be registered as a Windows service", &e);
                    Err(e)
                }
            },
            super::ServiceAction::Uninstall => match uninstall() {
                Ok(()) => {
                    println!("Nanofile is no longer registered as a Windows service.");
                    Ok(())
                }
                Err(e) => {
                    report_failure("The Nanofile service could not be removed", &e);
                    Err(e)
                }
            },
            super::ServiceAction::Status => {
                let state = probe(config_path);
                match &state {
                    ServiceProbe::NotInstalled => {
                        println!("not installed");
                        std::process::exit(1);
                    }
                    ServiceProbe::Ours { running } => println!(
                        "installed for this installation ({})",
                        if *running { "running" } else { "stopped" }
                    ),
                    ServiceProbe::OtherInstall {
                        image_path,
                        running,
                        missing,
                    } => println!(
                        "installed for another installation: {image_path} ({}{})",
                        if *running { "running" } else { "stopped" },
                        if *missing {
                            "; the registered executable no longer exists"
                        } else {
                            ""
                        }
                    ),
                    ServiceProbe::Unknown => {
                        println!(
                            "state unknown (the service control manager could not be queried)"
                        );
                        std::process::exit(1);
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub(crate) use windows_impl::probe as probe_service;
#[cfg(target_os = "windows")]
pub(crate) use windows_impl::run_cli;
#[cfg(target_os = "windows")]
pub(crate) use windows_impl::stop_service;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_command_line_quotes_both_paths() {
        let line = service_command_line(
            &PathBuf::from(r"C:\Apps\Na nofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Na nofile\config.toml"),
        );
        assert_eq!(
            line,
            r#""C:\Apps\Na nofile\nanofile.exe" service run --config "C:\Apps\Na nofile\config.toml""#
        );
    }

    #[test]
    fn the_command_line_escapes_inner_quotes() {
        let line = service_command_line(
            &PathBuf::from(r#"C:\we"ird\nanofile.exe"#),
            &PathBuf::from(r"C:\cfg.toml"),
        );
        assert!(line.contains(r#""C:\we\"ird\nanofile.exe""#));
    }

    #[test]
    fn the_description_names_the_config_file() {
        let described = service_description(&PathBuf::from(r"C:\nanofile\config.toml"));
        assert!(described.contains(r"C:\nanofile\config.toml"));
        assert!(described.contains("without a user login"));
    }

    #[test]
    fn an_image_path_is_compared_ignoring_case_and_quotes() {
        let expected = service_command_line(
            &PathBuf::from(r"C:\Apps\Nanofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Nanofile\config.toml"),
        );
        // What the registry holds after our own write.
        assert!(image_path_matches(&expected, &expected));
        // Case and a forward slash are the same path on Windows.
        let noisy = expected.to_lowercase().replace('\\', "/");
        assert!(image_path_matches(&noisy, &expected));
        // A service registered with only the executable still starts this
        // install, so it must not be reported as somebody else's.
        assert!(image_path_matches(
            r#""C:\Apps\Nanofile\nanofile.exe""#,
            &expected
        ));
    }

    #[test]
    fn another_install_is_not_ours() {
        let expected = service_command_line(
            &PathBuf::from(r"C:\Apps\Nanofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Nanofile\config.toml"),
        );
        // Same executable, different config file: it is not this installation's
        // service, and toggling the menu item has to say so.
        let other_config = service_command_line(
            &PathBuf::from(r"C:\Apps\Nanofile\nanofile.exe"),
            &PathBuf::from(r"D:\other\config.toml"),
        );
        assert!(!image_path_matches(&other_config, &expected));
        // A completely different binary.
        assert!(!image_path_matches(
            r#""C:\Elsewhere\nanofile.exe" --config "x""#,
            &expected
        ));
        assert!(!image_path_matches("", &expected));
    }

    #[test]
    fn the_executable_is_read_out_of_an_image_path() {
        assert_eq!(
            executable_in(r#""C:\Apps\Nanofile\nanofile.exe" service run --config "x""#),
            Some(r"C:\Apps\Nanofile\nanofile.exe".to_string())
        );
        // Unquoted (what the SCM writes for a path without spaces).
        assert_eq!(
            executable_in(r"C:\nanofile.exe service run"),
            Some(r"C:\nanofile.exe".to_string())
        );
        assert_eq!(executable_in(""), None);
        assert_eq!(executable_in("   "), None);
        assert_eq!(executable_in(r#"""#), None);
    }

    #[test]
    fn a_probe_knows_whether_it_is_ours() {
        assert!(ServiceProbe::Ours { running: true }.is_ours());
        assert!(ServiceProbe::Ours { running: true }.is_running());
        assert!(!ServiceProbe::Ours { running: false }.is_running());
        assert!(ServiceProbe::Ours { running: false }.is_installed());
        let other = ServiceProbe::OtherInstall {
            image_path: "x".into(),
            running: false,
            missing: true,
        };
        assert!(!other.is_ours());
        assert!(other.is_installed());
        assert!(other.is_broken(), "the registered file is gone");
        assert!(!ServiceProbe::Ours { running: true }.is_broken());
        assert!(!ServiceProbe::NotInstalled.is_installed());
        assert!(!ServiceProbe::Unknown.is_installed());
    }
}
