//! The account the Windows service runs under.
//!
//! Microsoft's guidance is to prefer a per-service virtual account (or
//! `LocalService`) over `LocalSystem`, which is "powerful and convenient, but
//! rarely appropriate: a compromise grants near-total control of the computer".
//! A virtual account (`NT SERVICE\<ServiceName>`) is created on demand, has its
//! own SID, needs no password or rotation, and reaches the network as the
//! computer account — which is what a file server with no domain identity needs.
//! It is the default here; `LocalSystem` stays available for a deployment whose
//! state directory is somewhere a virtual account cannot be granted access to.

/// `NT SERVICE\Nanofile`. The name is written out rather than derived because
/// the SDK's `%SERVICE_NAME%` placeholder is not expandable here.
pub(crate) const VIRTUAL_ACCOUNT: &str = r"NT SERVICE\Nanofile";
/// The machine account on the network, with minimal local privileges.
pub(crate) const NETWORK_ACCOUNT: &str = r"NT AUTHORITY\NetworkService";

/// The account a Windows service runs under.
///
/// `Virtual` is the default: a per-service virtual account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ServiceAccount {
    /// A per-service virtual account (the default).
    #[default]
    Virtual,
    /// `LocalSystem`: registered as no `ObjectName` at all, which is how the SCM
    /// records the default account.
    System,
    /// `NT AUTHORITY\NetworkService`.
    Network,
    /// A local or domain account, which needs a password and the "Log on as a
    /// service" right.
    Named { name: String, password: String },
}

impl ServiceAccount {
    /// Parse `--account`, with the password a named account needs.
    ///
    /// A password is required exactly when the value is not a built-in identity:
    /// `virtual`, `system` and `network` have none, and neither does a name from
    /// the `NT SERVICE\` / `NT AUTHORITY\` namespaces.
    pub(crate) fn parse(value: &str, password: Option<String>) -> Result<Self, String> {
        let value = value.trim();
        let upper = value.to_ascii_uppercase();
        match value.to_ascii_lowercase().as_str() {
            "" | "virtual" | "auto" => Ok(Self::Virtual),
            "system" | "local-system" | "localsystem" => Ok(Self::System),
            "network" | "network-service" | "networkservice" => Ok(Self::Network),
            _ if upper.starts_with(r"NT SERVICE\") || upper.starts_with(r"NT AUTHORITY\") => {
                Ok(Self::Named {
                    name: value.to_string(),
                    password: String::new(),
                })
            }
            _ => match password.filter(|p| !p.is_empty()) {
                Some(password) => Ok(Self::Named {
                    name: value.to_string(),
                    password,
                }),
                None => Err(format!(
                    "the account '{value}' needs a password (`--password`, `--password-stdin`, \
                     or the interactive prompt)"
                )),
            },
        }
    }

    /// The `lpServiceStartName` to register; `None` means `LocalSystem`.
    pub(crate) fn name(&self) -> Option<&str> {
        match self {
            Self::Virtual => Some(VIRTUAL_ACCOUNT),
            Self::System => None,
            Self::Network => Some(NETWORK_ACCOUNT),
            Self::Named { name, .. } => Some(name),
        }
    }

    /// The `lpPassword`; only a named account has one.
    pub(crate) fn password(&self) -> Option<&str> {
        match self {
            Self::Named { password, .. } => Some(password),
            _ => None,
        }
    }

    /// How the account is shown to a person, in a dialog or in `service status`.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Virtual => VIRTUAL_ACCOUNT.to_string(),
            Self::System => "LocalSystem".to_string(),
            Self::Network => NETWORK_ACCOUNT.to_string(),
            Self::Named { name, .. } => name.clone(),
        }
    }

    /// The name to resolve to a SID for the per-directory grant.
    ///
    /// `None` for `LocalSystem`, which already has access to everything: a grant
    /// would change a system directory's access control for no reason.
    pub(crate) fn grant_target(&self) -> Option<String> {
        match self {
            Self::System => None,
            _ => self.name().map(str::to_string),
        }
    }

    /// Whether the account needs the "Log on as a service" right granted
    /// explicitly. The built-in and virtual identities have it implicitly.
    pub(crate) fn needs_service_logon_right(&self) -> bool {
        match self {
            Self::Named { name, .. } => {
                let upper = name.to_ascii_uppercase();
                !upper.starts_with(r"NT SERVICE\") && !upper.starts_with(r"NT AUTHORITY\")
            }
            _ => false,
        }
    }
}

/// Where a named account's password comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasswordSource {
    /// `--password <value>`.
    Inline,
    /// `--password-stdin`.
    Stdin,
    /// Nothing was given: prompt, but only if the account turns out to need one.
    Prompt,
}

/// Resolve `--account` and its password into an account.
///
/// A prompt only happens for a named account that was given no password, so
/// `service install` with the default (virtual) account never blocks on input.
pub(crate) fn resolve(
    value: &str,
    password: Option<String>,
    source: PasswordSource,
) -> anyhow::Result<ServiceAccount> {
    let supplied = match source {
        PasswordSource::Inline => password,
        PasswordSource::Stdin => Some(read_password_stdin()?),
        PasswordSource::Prompt => {
            if ServiceAccount::parse(value, None).is_err() {
                Some(prompt_password(value)?)
            } else {
                None
            }
        }
    };
    ServiceAccount::parse(value, supplied).map_err(|e| anyhow::anyhow!(e))
}

fn read_password_stdin() -> anyhow::Result<String> {
    use std::io::BufRead as _;

    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        anyhow::bail!("--password-stdin read an empty password");
    }
    Ok(password)
}

fn prompt_password(account: &str) -> anyhow::Result<String> {
    let password = rpassword::prompt_password(format!("Password for {account}: "))?;
    if password.is_empty() {
        anyhow::bail!("no password entered for {account}");
    }
    Ok(password)
}

/// Grant the "Log on as a service" right to a named account.
///
/// Without it the service registers successfully and then fails at start with
/// error 1069, which is a bad way to learn about it. The built-in and virtual
/// identities have the right implicitly and must not come here.
#[cfg(target_os = "windows")]
pub(crate) fn grant_service_logon_right(name: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::Security::Authentication::Identity::{
        LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, LsaAddAccountRights, LsaClose, LsaOpenPolicy,
        POLICY_LOOKUP_NAMES,
    };
    use windows_sys::Win32::Security::{LookupAccountNameW, SID_NAME_USE};

    let name_w = crate::startup::win32::wide(name);

    // The account has to exist: resolving it also produces the SID the right is
    // granted to.
    let mut sid_len = 0u32;
    let mut domain_len = 0u32;
    let mut use_: SID_NAME_USE = 0;
    unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut sid_len,
            std::ptr::null_mut(),
            &mut domain_len,
            &mut use_,
        );
    }
    if sid_len == 0 {
        anyhow::bail!("the account '{name}' could not be resolved on this machine");
    }
    let mut sid = vec![0u8; sid_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    let found = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name_w.as_ptr(),
            sid.as_mut_ptr() as *mut core::ffi::c_void,
            &mut sid_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_,
        )
    };
    if found == 0 {
        anyhow::bail!("the account '{name}' could not be resolved on this machine");
    }

    // "SeServiceLogonRight" as the LSA expects it: counted, not NUL-terminated.
    let mut right_name = crate::startup::win32::wide("SeServiceLogonRight");
    right_name.pop();
    let right = LSA_UNICODE_STRING {
        Length: (right_name.len() * 2) as u16,
        MaximumLength: (right_name.len() * 2) as u16,
        Buffer: right_name.as_mut_ptr(),
    };

    unsafe {
        let mut policy: isize = 0;
        let attributes: LSA_OBJECT_ATTRIBUTES = std::mem::zeroed();
        let status = LsaOpenPolicy(
            std::ptr::null(),
            &attributes,
            POLICY_LOOKUP_NAMES as u32,
            &mut policy,
        );
        if status != 0 {
            anyhow::bail!("opening the local security policy failed (status {status:#x})");
        }
        let status = LsaAddAccountRights(
            policy,
            sid.as_mut_ptr() as *mut core::ffi::c_void,
            &right,
            1,
        );
        LsaClose(policy);
        if status != 0 {
            anyhow::bail!(
                "granting \"Log on as a service\" to '{name}' failed (status {status:#x}); grant \
                 it by hand in secpol.msc (Local Policies → User Rights Assignment)"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_virtual_account() {
        assert_eq!(ServiceAccount::default(), ServiceAccount::Virtual);
        assert_eq!(ServiceAccount::default().label(), VIRTUAL_ACCOUNT);
        assert_eq!(ServiceAccount::default().name(), Some(VIRTUAL_ACCOUNT));
        assert_eq!(ServiceAccount::default().password(), None);
        assert!(ServiceAccount::default().grant_target().is_some());
        assert!(!ServiceAccount::default().needs_service_logon_right());
    }

    #[test]
    fn the_built_in_identities_need_no_password() {
        let system = ServiceAccount::parse("system", None).unwrap();
        assert_eq!(system.name(), None, "LocalSystem is registered as no name");
        assert_eq!(system.label(), "LocalSystem");
        assert_eq!(
            system.grant_target(),
            None,
            "LocalSystem already has access"
        );
        assert!(!system.needs_service_logon_right());

        let network = ServiceAccount::parse("network", None).unwrap();
        assert_eq!(network.name(), Some(NETWORK_ACCOUNT));
        assert!(network.grant_target().is_some());
        assert!(!network.needs_service_logon_right());
    }

    #[test]
    fn a_named_account_needs_a_password_and_the_logon_right() {
        assert!(ServiceAccount::parse(r".\nanofile", None).is_err());
        assert!(ServiceAccount::parse(r".\nanofile", Some(String::new())).is_err());

        let named = ServiceAccount::parse(r".\nanofile", Some("secret".into())).unwrap();
        assert_eq!(named.name(), Some(r".\nanofile"));
        assert_eq!(named.password(), Some("secret"));
        assert!(named.needs_service_logon_right());

        // A service SID needs neither a password nor the right.
        let virtual_named = ServiceAccount::parse(r"NT SERVICE\Other", None).unwrap();
        assert_eq!(virtual_named.password(), Some(""));
        assert!(!virtual_named.needs_service_logon_right());
    }

    #[test]
    fn the_password_source_decides_whether_a_prompt_is_needed() {
        // The default account never prompts, so `service install` cannot block
        // waiting for input nobody is there to type.
        assert!(resolve("virtual", None, PasswordSource::Prompt).is_ok());
        // An inline password is passed through to a named account.
        let named = resolve(r".\nanofile", Some("secret".into()), PasswordSource::Inline).unwrap();
        assert_eq!(named.password(), Some("secret"));
        // A named account with nothing to read the password from is an error,
        // never an empty password.
        assert!(resolve(r".\nanofile", None, PasswordSource::Inline).is_err());
    }
}
