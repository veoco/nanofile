//! `config.toml.example` is deliberately a small file.
//!
//! The settings catalog plus the admin pages and locales are the reference for every
//! key; the template only carries read-only keys and values an operator must decide
//! before the interface is reachable. The tests below hold that line: the file names
//! nothing else, and still documents every file-only key.

use std::collections::BTreeSet;
use std::path::PathBuf;

use infra::settings::{Apply, CATALOG};

/// Keys the template keeps although a page can store them: values decided before the
/// page is reachable (listener, external URL, state paths, language). Removing one is
/// a documentation decision — drop its entry here in the same commit.
const DEPLOYMENT_KEYS: &[&str] = &[
    "server.addr",
    "server.port",
    "server.site_url",
    "server.tray",
    "storage.block_dir",
    "storage.temp_dir",
    "storage.thumbnail_dir",
    "storage.avatar_dir",
    "index.index_dir",
    "ui.default_language",
];

/// `[settings]` decides what a stored value means, so it is not a catalog entry and can
/// only come from the file or environment; the template is its only prose home.
const POLICY_KEYS: &[&str] = &[
    "settings.config_policy",
    "settings.config_override_keys",
    "settings.refresh_interval_secs",
];

fn example_text() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config.toml.example");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// Every `section.key` the file mentions, commented out or not. A section header must
/// be a line of nothing but `[name]` (optionally commented): prose starting with a
/// bracketed name must not move following keys into the wrong section.
fn keys_in(text: &str) -> BTreeSet<String> {
    let mut section: Option<String> = None;
    let mut keys = BTreeSet::new();
    for raw in text.lines() {
        let line = raw.trim();
        let line = line.strip_prefix('#').map(str::trim).unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            section = Some(name.to_string());
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        if let Some(section) = &section {
            keys.insert(format!("{section}.{name}"));
        }
    }
    keys
}

/// The keys the template may name: catalog read-only entries, deployment keys, policy.
fn allowed_keys() -> BTreeSet<String> {
    let mut allowed: BTreeSet<String> = CATALOG
        .iter()
        .filter(|def| def.apply == Apply::ReadOnly)
        .map(|def| def.key.to_string())
        .collect();
    allowed.extend(DEPLOYMENT_KEYS.iter().map(|key| (*key).to_string()));
    allowed.extend(POLICY_KEYS.iter().map(|key| (*key).to_string()));
    allowed
}

#[test]
fn the_example_names_nothing_a_settings_page_can_store() {
    let text = example_text();
    let actual = keys_in(&text);
    let allowed = allowed_keys();

    let unexpected: Vec<&String> = actual.difference(&allowed).collect();
    assert!(
        unexpected.is_empty(),
        "config.toml.example names keys a settings page already stores: {unexpected:?}. \
         Delete them — the page labels, help text and NANOFILE_* variable are the reference — \
         or, if a value genuinely has to be decided before the page is reachable, add it to \
         DEPLOYMENT_KEYS with that reason."
    );
}

#[test]
fn the_example_documents_every_file_only_key() {
    let text = example_text();
    let actual = keys_in(&text);

    let missing: Vec<&str> = CATALOG
        .iter()
        .filter(|def| def.apply == Apply::ReadOnly)
        .map(|def| def.key)
        .filter(|key| !actual.contains(*key))
        .collect();
    assert!(
        missing.is_empty(),
        "config.toml.example no longer documents {missing:?}; a file-only key has no other \
         place to be set, so the template is where it has to be visible"
    );

    let missing: Vec<&str> = DEPLOYMENT_KEYS
        .iter()
        .chain(POLICY_KEYS)
        .copied()
        .filter(|key| !actual.contains(*key))
        .collect();
    assert!(
        missing.is_empty(),
        "config.toml.example dropped {missing:?}, which its allow-list still claims to carry"
    );
}

/// Shipped to operators, so it must be a valid configuration before editing: every key
/// commented out, and the two active ones (`secret_key`, `config_policy`) parse.
#[test]
fn the_example_parses_as_a_config() {
    let text = example_text();
    let parsed: infra::config::Config =
        toml::from_str(&text).expect("config.toml.example must deserialize into Config");
    // An empty master key is the point: a release build refuses to start and names the
    // variable to set. Pinned so a future edit cannot put a working-looking placeholder there.
    assert_eq!(parsed.server.secret_key, "");
    assert_eq!(parsed.settings.config_policy, "bootstrap");
}
