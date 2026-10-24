//! Keep the API reference honest.
//!
//! The reference pages under `docs/reference/api/` are prose, but their endpoint
//! markers and anchors are a contract: every classified route must appear once,
//! no documented `/api2/` or `/api/v2.1/` route may be invented, the English and
//! Chinese pages must describe the same endpoints, every cross-link must land on
//! an existing explicit anchor, and a page must have exactly one level-one
//! heading. This test is the guard for all five.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// `(method, normalized path)`.
type Route = (String, String);

fn api_dir(root: &Path, locale: &str) -> PathBuf {
    root.join("docs").join(locale).join("reference").join("api")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the server crate lives in the repository root")
        .to_path_buf()
}

/// Page name (`index`, `files`, …) → source text, sorted for deterministic output.
fn pages(dir: &Path) -> BTreeMap<String, String> {
    let mut pages = BTreeMap::new();
    for entry in fs::read_dir(dir).expect("the API reference directory exists") {
        let path = entry.expect("readable directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("a UTF-8 page name")
            .to_string();
        let text = fs::read_to_string(&path).expect("readable UTF-8 page");
        pages.insert(name, text);
    }
    assert!(!pages.is_empty(), "no API reference pages found");
    pages
}

/// `/a/{b}/` → `/a/{b}`; `{*path}` and `{path}` compare equal; the query and
/// trailing slash are not part of the route identity.
fn normalize_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let path = path.replace("{*", "{");
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

/// Standalone marker lines: `` `GET /api2/repos/` ``.
fn markers(text: &str) -> BTreeSet<Route> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let inner = line.strip_prefix('`')?.strip_suffix('`')?;
            let (method, path) = inner.split_once(' ')?;
            if !matches!(method, "GET" | "POST" | "PUT" | "DELETE" | "PATCH") {
                return None;
            }
            if !path.starts_with('/') {
                return None;
            }
            Some((method.to_string(), normalize_path(path)))
        })
        .collect()
}

/// Explicit heading anchors, `{#like-this}`.
fn anchors(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("{#") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        out.insert(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

/// Every `](/reference/api/...#anchor)` link in a page, as `(page, anchor)`.
fn api_links(text: &str, prefix: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else { break };
        let target = &after[..end];
        rest = &after[end..];
        if !target.starts_with(prefix) {
            continue;
        }
        let rest_of_target = &target[prefix.len()..];
        let (page, anchor) = match rest_of_target.split_once('#') {
            Some((page, anchor)) => (page, Some(anchor.to_string())),
            None => (rest_of_target, None),
        };
        let page = page.trim_matches('/');
        let page = if page.is_empty() { "index" } else { page };
        out.push((page.to_string(), anchor));
    }
    out
}

fn assert_pages_match(en: &BTreeMap<String, String>, zh: &BTreeMap<String, String>) {
    let en_pages: BTreeSet<&str> = en.keys().map(String::as_str).collect();
    let zh_pages: BTreeSet<&str> = zh.keys().map(String::as_str).collect();
    assert_eq!(
        en_pages, zh_pages,
        "the English and Chinese API reference must contain the same pages"
    );
}

#[test]
fn every_classified_route_is_documented() {
    let en = pages(&api_dir(&repo_root(), ""));
    let documented: BTreeSet<Route> = en.values().flat_map(|text| markers(text)).collect();
    let mut missing: Vec<Route> = Vec::new();
    for (method, pattern) in server::domain::capability::route_inventory() {
        let route = (method.to_string(), normalize_path(pattern));
        if !documented.contains(&route) {
            missing.push(route);
        }
    }
    assert!(
        missing.is_empty(),
        "these classified routes have no endpoint section in docs/reference/api: {missing:?}"
    );
}

#[test]
fn documented_api_routes_exist() {
    let root = repo_root();
    let en = pages(&api_dir(&root, ""));
    let routes: BTreeSet<Route> = server::domain::capability::route_inventory()
        .map(|(m, p)| (m.to_string(), normalize_path(p)))
        .collect();
    let mut invented: BTreeSet<Route> = BTreeSet::new();
    for (name, text) in &en {
        // The comparison page names official operations that are not our routes.
        if name == "compatibility" {
            continue;
        }
        for route in markers(text) {
            let is_api = route.1.starts_with("/api2/") || route.1.starts_with("/api/v2.1/");
            if is_api && !routes.contains(&route) {
                invented.insert(route);
            }
        }
    }
    assert!(
        invented.is_empty(),
        "these endpoints are documented but are not classified routes: {invented:?}"
    );
}

#[test]
fn english_and_chinese_pages_describe_the_same_endpoints() {
    let root = repo_root();
    let en = pages(&api_dir(&root, ""));
    let zh = pages(&api_dir(&root, "zh"));
    assert_pages_match(&en, &zh);
    for (name, en_text) in &en {
        let zh_text = zh.get(name).expect("parity was asserted above");
        assert_eq!(
            markers(en_text),
            markers(zh_text),
            "{name}.md: the English and Chinese pages must carry the same endpoint markers"
        );
        assert_eq!(
            anchors(en_text),
            anchors(zh_text),
            "{name}.md: the English and Chinese pages must carry the same explicit anchors"
        );
    }
}

#[test]
fn internal_links_resolve() {
    let root = repo_root();
    for (locale, prefix) in [("", "/reference/api"), ("zh", "/zh/reference/api")] {
        let all = pages(&api_dir(&root, locale));
        for (name, text) in &all {
            for (page, anchor) in api_links(text, prefix) {
                let target = all.get(&page).unwrap_or_else(|| {
                    panic!("{name}.md links to a missing page: {prefix}/{page}")
                });
                if let Some(anchor) = anchor {
                    assert!(
                        anchors(target).contains(&anchor),
                        "{name}.md links to {prefix}/{page}#{anchor}, but that page has no explicit anchor '{anchor}'"
                    );
                }
            }
        }
    }
}

#[test]
fn transfer_endpoints_are_documented() {
    // These routes are served by the transfer group rather than by the classified
    // `/api2/` surface, so `route_inventory()` cannot see them. They are the URLs
    // the API hands out, so a page must keep documenting them.
    let expected: &[(&str, &str)] = &[
        ("GET", "/repos/{repo_id}/files/{path}"),
        ("GET", "/download-api/{token}"),
        ("GET", "/blks/{token}/{file_id}/{block_id}"),
        ("POST", "/upload-api/{token}"),
        ("POST", "/upload-aj/{token}"),
        ("POST", "/upload-aj/"),
        ("POST", "/update-api/{token}"),
        ("POST", "/update-aj/{token}"),
        ("POST", "/update-aj/"),
        ("POST", "/update-api/"),
        ("POST", "/upload-blks-api/{token}"),
        ("GET", "/upload_progress"),
        ("GET", "/idx_progress"),
        ("GET", "/zip/{token}"),
    ];
    let en = pages(&api_dir(&repo_root(), ""));
    let documented: BTreeSet<Route> = en.values().flat_map(|text| markers(text)).collect();
    let missing: Vec<Route> = expected
        .iter()
        .map(|(m, p)| (m.to_string(), normalize_path(p)))
        .filter(|route| !documented.contains(route))
        .collect();
    assert!(
        missing.is_empty(),
        "these transfer endpoints lost their documentation: {missing:?}"
    );
}

#[test]
fn every_page_has_a_single_title() {
    // The Chinese comparison page once had its intro paragraphs written as `#`
    // headings, so the top of the page read as three long titles. Body text
    // stays body text: one level-one heading, at the top, per page. Lines in
    // fenced code blocks are ignored so a `#` comment cannot trip this.
    let root = repo_root();
    for locale in ["", "zh"] {
        for (name, text) in pages(&api_dir(&root, locale)) {
            let title = text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_else(|| panic!("{locale}/{name}.md is empty"));
            assert!(
                title.starts_with("# "),
                "{locale}/{name}.md must open with its level-one heading"
            );
            let mut fenced = false;
            let mut extra = Vec::new();
            for line in text.lines() {
                if line.trim_start().starts_with("```") {
                    fenced = !fenced;
                    continue;
                }
                if !fenced && line != title && line.starts_with("# ") {
                    extra.push(line);
                }
            }
            assert!(
                extra.is_empty(),
                "{locale}/{name}.md has level-one headings beyond its title: {extra:?}"
            );
        }
    }
}
