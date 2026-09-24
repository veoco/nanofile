//! What the tray should do about the two automatic-start mechanisms.
//!
//! On Windows a login entry and an SCM service are two ways to say "start
//! Nanofile automatically", and they are **alternatives**: registering the
//! service removes the login entry, and leaving both in place starts a second
//! instance at every login that can only come up as a client-mode tray.
//!
//! That rule lives here, as one pure function over the probed state, so the
//! menu's construction, every toggle's re-synchronisation and the startup repair
//! cannot drift apart. Nothing in this module touches the machine.

use super::login::LoginState;

/// Everything the plan needs to know about the machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StartupState {
    pub login: LoginState,
    /// This installation's Windows service is registered. Always `false` where
    /// there is no service concept.
    pub service_ours: bool,
}

/// What to apply before the menu is shown, and how to show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartupPlan {
    /// Repoint the login entry at the copy that is running.
    pub repair_login: bool,
    /// Remove our login entry: the service has replaced it.
    pub retire_login: bool,
    /// Whether the login item accepts a click.
    pub login_enabled: bool,
    /// The checkmark the login item shows (the registration as it really is).
    pub login_checked: bool,
    /// The checkmark the service item shows.
    pub service_checked: bool,
}

/// Decide what the tray does about both mechanisms.
pub(crate) fn plan(state: StartupState) -> StartupPlan {
    StartupPlan {
        // A stale entry is only repaired while the service is *not* ours:
        // otherwise the repair would re-create the very entry the service
        // replaced (and the next login would start a second instance).
        repair_login: state.login.stale && !state.service_ours,
        // The same rule from the other side: an entry of ours that survived an
        // install (an older build, or `service install` run outside the tray)
        // is removed, so "alternatives" is enforced rather than assumed. An
        // entry belonging to another installation is left alone.
        retire_login: state.login.ours && state.service_ours,
        login_enabled: !state.service_ours,
        // The checkmark always reports the registration as it is, even while
        // the item is disabled, so the menu never claims a state the machine is
        // not in.
        login_checked: state.login.present,
        service_checked: state.service_ours,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(present: bool, ours: bool, stale: bool) -> LoginState {
        LoginState {
            present,
            ours,
            stale,
        }
    }

    #[test]
    fn nothing_registered_leaves_both_items_off() {
        let plan = plan(StartupState::default());
        assert_eq!(
            plan,
            StartupPlan {
                repair_login: false,
                retire_login: false,
                login_enabled: true,
                login_checked: false,
                service_checked: false,
            }
        );
    }

    #[test]
    fn a_stale_login_entry_is_repaired_only_while_no_service_is_ours() {
        let stale = StartupState {
            login: login(true, true, true),
            service_ours: false,
        };
        assert!(plan(stale).repair_login);

        // The regression: the service replaced the login entry, so repairing a
        // stale entry next to it would start two instances at the next login.
        let with_service = StartupState {
            service_ours: true,
            ..stale
        };
        let plan = plan(with_service);
        assert!(
            !plan.repair_login,
            "the service must not get a login partner"
        );
        assert!(!plan.login_enabled, "the login item is disabled");
        assert!(plan.login_checked, "it still reports the entry that exists");
        assert!(plan.service_checked);
    }

    #[test]
    fn our_login_entry_is_retired_once_the_service_is_ours() {
        let both = StartupState {
            login: login(true, true, false),
            service_ours: true,
        };
        assert!(plan(both).retire_login);

        // Somebody else's entry is not ours to delete.
        let foreign = StartupState {
            login: login(true, false, false),
            service_ours: true,
        };
        assert!(!plan(foreign).retire_login);
        // ... and with no service, nothing is retired either.
        let alone = StartupState {
            login: login(true, true, false),
            service_ours: false,
        };
        assert!(!plan(alone).retire_login);
    }

    #[test]
    fn a_login_entry_that_belongs_to_another_install_is_shown_but_not_claimed() {
        let other = StartupState {
            login: login(true, false, false),
            service_ours: false,
        };
        let plan = plan(other);
        assert!(plan.login_enabled);
        assert!(plan.login_checked, "the registration is reported as it is");
        assert!(!plan.retire_login);
        assert!(!plan.repair_login);
    }

    #[test]
    fn an_unreadable_entry_is_never_repaired_or_retired() {
        // Present, but it cannot be read back as an (executable, config) pair:
        // `ours` and `stale` are both false by construction.
        let unreadable = StartupState {
            login: login(true, false, false),
            service_ours: true,
        };
        assert!(!plan(unreadable).retire_login);
        assert!(!plan(unreadable).repair_login);
    }
}
