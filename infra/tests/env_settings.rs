//! The environment layer, end to end.
//!
//! A single test in its own binary on purpose: environment variables are
//! process-global, so a second test in this file could run concurrently and see
//! half of this one's overrides. `server/tests` covers the integration paths;
//! this pins the contract the admin UI depends on — which variable feeds which
//! setting, and which catalog keys report the environment as their origin.

use std::collections::BTreeSet;

#[test]
fn every_catalog_env_variable_is_read_and_tracked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[server]\nport = 8082\n").unwrap();

    // One variable per value shape the catalog can parse, including keys that
    // had no environment support before this change and are documented as if
    // they did.
    let overrides = [
        ("NANOFILE_SERVER_SHARE_LINK_ENABLED", "false"),
        ("NANOFILE_SERVER_TRUST_REQUEST_HOST", "false"),
        ("NANOFILE_SERVER_HSTS_INCLUDE_SUBDOMAINS", "true"),
        ("NANOFILE_SERVER_MAX_PROPFIND_ENTRIES", "1234"),
        ("NANOFILE_AUTH_MAX_LOGIN_ATTEMPTS", "1"),
        ("NANOFILE_AUTH_SEARCH_MAX_PER_MINUTE", "9"),
        ("NANOFILE_SERVER_TRUSTED_PROXIES", "10.0.0.1, 10.0.0.2"),
        ("NANOFILE_UI_DEFAULT_LANGUAGE", "zh"),
        ("NANOFILE_EMAIL_NOTIFY_NEW_LOGIN", "false"),
        ("NANOFILE_EMAIL_PAUSED", "true"),
        ("NANOFILE_SYNC_VERIFY_FS_OBJECTS", "log"),
        ("NANOFILE_LOG_MAX_BACKUPS", "9"),
    ];
    for (name, value) in overrides {
        // SAFETY: this integration test binary contains a single test, so no
        // other thread is reading the environment while these are set.
        unsafe { std::env::set_var(name, value) };
    }
    // A typo must be ignored, not fatal, and must not be reported as a source.
    unsafe { std::env::set_var("NANOFILE_SERVER_PORT", "not-a-port") };

    let loaded = infra::config::Config::load_from_tracked(&path).expect("load");
    let config = &loaded.config;

    assert!(!config.server.share_link_enabled);
    assert!(!config.server.trust_request_host);
    assert!(config.server.hsts_include_subdomains);
    assert_eq!(config.server.max_propfind_entries, 1234);
    assert_eq!(config.auth.max_login_attempts, 1);
    assert_eq!(config.auth.search_max_per_minute, 9);
    assert_eq!(config.server.trusted_proxies, vec!["10.0.0.1", "10.0.0.2"]);
    assert_eq!(config.ui.default_language, "zh");
    assert!(!config.email.notify_new_login);
    assert!(config.email.paused);
    assert_eq!(config.sync.verify_fs_objects, "log");
    assert_eq!(config.logging.max_backups, 9);
    // The unparsable port kept its configured value and is not reported as
    // environment-supplied.
    assert_eq!(config.server.port, 8082);

    let expected: BTreeSet<&str> = overrides.iter().map(|(name, _)| *name).collect();
    assert_eq!(expected.len(), overrides.len(), "names must be distinct");
    assert_eq!(
        loaded.env_keys.len(),
        overrides.len(),
        "nothing extra tracked"
    );
    for key in [
        "server.share_link_enabled",
        "server.trust_request_host",
        "server.hsts_include_subdomains",
        "server.max_propfind_entries",
        "auth.max_login_attempts",
        "auth.search_max_per_minute",
        "server.trusted_proxies",
        "ui.default_language",
        "email.notify_new_login",
        "email.paused",
        "sync.verify_fs_objects",
        "logging.max_backups",
    ] {
        assert!(loaded.env_keys.contains(key), "{key} not tracked");
    }
    assert!(!loaded.env_keys.contains("server.port"));

    for (name, _) in overrides {
        unsafe { std::env::remove_var(name) };
    }
    unsafe { std::env::remove_var("NANOFILE_SERVER_PORT") };
}
