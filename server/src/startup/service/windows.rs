//! The Windows side of the service: the Service Control Manager registration
//! (install, uninstall, stop, probe) and the service control loop itself.
//!
//! Nothing here shows a dialog: the tray owns the confirmations, and a headless
//! `nanofile service install` has a console to report on.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

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
    DISPLAY_NAME, EXIT_FAILED, EXIT_REFUSED, SERVICE_NAME, ServiceProbe, image_path_matches,
    service_command_line, service_description,
};
use crate::startup::win32::wide;

/// Where the SCM keeps a service's registration, read directly to compare the
/// binary path. `QueryServiceConfigW` would work too, but reading the value is
/// what a user without `SERVICE_QUERY_CONFIG` can still do.
const SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\Nanofile";

pub(crate) fn message_box(title: &str, text: &str, flags: u32) -> i32 {
    let title = wide(title);
    let text = wide(text);
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) }
}

/// Report a failure the way a GUI-subsystem process can: a message box the
/// user will actually see, plus the log file.
pub(super) fn report_failure(context: &str, error: &anyhow::Error) {
    tracing::error!("{context}: {error:#}");
    let title = "Nanofile";
    let body = format!("{context}.\n\n{error:#}");
    message_box(title, &body, MB_OK | MB_ICONERROR | MB_SETFOREGROUND);
}

// ── Running as a service ─────────────────────────────────────────────────────

/// The config the `ServiceMain` callback (which is called by the SCM, on a
/// thread this process does not control) has to pick up: the loaded
/// configuration, the environment keys it was layered with, and the path of the
/// file it came from (the preflight checks the last one directly).
static SERVICE_INPUT: std::sync::Mutex<Option<(Config, EnvKeys, PathBuf)>> =
    std::sync::Mutex::new(None);

struct ServiceRuntime {
    /// `SERVICE_STATUS_HANDLE`, kept as an integer: a raw pointer is neither
    /// `Send` nor `Sync`, so it cannot live in a `static` directly.
    status_handle: isize,
    state: u32,
    exit_code: u32,
    quit: Option<tokio::sync::mpsc::UnboundedSender<crate::TrayCommand>>,
}

static SERVICE_RUNTIME: std::sync::Mutex<Option<ServiceRuntime>> = std::sync::Mutex::new(None);

fn report(state: u32, exit_code: i32, wait_hint_ms: u32) {
    let mut guard = SERVICE_RUNTIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(runtime) = guard.as_mut() else {
        return;
    };
    runtime.state = state;
    if exit_code != 0 {
        runtime.exit_code = exit_code.max(0) as u32;
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
    let Some((config, env_keys, config_path)) = input else {
        tracing::error!("the service was dispatched without a configuration");
        report(SERVICE_STOPPED, EXIT_FAILED, 0);
        return;
    };

    // Say who this is before anything else: every permission question below is
    // about *this* account, and the log is the only place the answer appears.
    tracing::info!(
        account = crate::startup::win32::current_account_name().unwrap_or_default(),
        exe = %std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        config = %config_path.display(),
        cwd = %std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        "the service is starting"
    );

    // The authoritative preflight: it runs as the account that will serve, which
    // is the one whose access to the data directories actually matters.
    let preflight =
        super::preflight::run(&super::preflight::Input::from_config(&config, &config_path));
    preflight.log();
    if !preflight.is_ok() {
        tracing::error!(
            "refusing to start: {} preflight check(s) failed; the directories above have to be \
             writable by this account",
            preflight.of(super::checks::Severity::Fail).count()
        );
        report(SERVICE_STOPPED, EXIT_REFUSED, 0);
        return;
    }

    report(SERVICE_RUNNING, 0, 0);
    tracing::info!("running as the Windows service '{SERVICE_NAME}'");

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("failed to create the tokio runtime: {e}");
            report(SERVICE_STOPPED, EXIT_FAILED, 0);
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
            report(SERVICE_STOPPED, EXIT_FAILED, 0);
        }
    }
}

/// Hand this process to the SCM. Blocks until the service stops.
///
/// Fails immediately when the process was not started by the SCM, which is
/// what a user typing `nanofile service run` gets — a clear error instead of
/// a process that hangs waiting for a dispatcher that will never call.
pub(crate) fn run_as_service(
    config: Config,
    env_keys: EnvKeys,
    config_path: PathBuf,
) -> anyhow::Result<()> {
    *SERVICE_INPUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((config, env_keys, config_path));
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

// ── Install / uninstall / status ─────────────────────────────────────────────

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

/// Read a `REG_SZ`/`REG_EXPAND_SZ` value out of the service's registry key.
fn read_service_string(value_name: &str) -> Option<String> {
    let subkey = wide(SERVICE_KEY);
    let value = wide(value_name);
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

/// Read the service's registered `ImagePath` from the registry.
fn read_image_path() -> Option<String> {
    read_service_string("ImagePath")
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
    let mut exists = false;
    if !service.is_null() {
        exists = true;
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
            if !exists && !query_failed {
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
#[cfg(feature = "tray")]
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
