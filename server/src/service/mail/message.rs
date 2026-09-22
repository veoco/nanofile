//! Rendering one message: subject, plain-text body, HTML body, and the raw
//! RFC 5322 bytes that are handed to the transport (and stored encrypted while
//! the message waits in the queue).

use lettre::message::{Mailbox, MultiPart};
use lettre::{Address, Message};

use super::i18n::MailStrings;
use super::settings::EmailSettings;
use super::{MailError, MailKind};

/// Everything a body may interpolate.
///
/// One struct for every kind (rather than one per kind) so the placeholder set
/// is closed and testable: a body may only name a value listed here.
#[derive(Clone, Debug, Default)]
pub struct MailParams {
    /// Display name of the recipient (`users.nickname()`).
    pub display_name: Option<String>,
    /// Absolute URL of this server, for prose ("open Settings at …").
    pub site_url: Option<String>,
    /// The one-time password-reset link.
    pub link: Option<String>,
    /// How long the reset link stays valid, in days.
    pub ttl_days: Option<i64>,
    /// Client address the action came from.
    pub ip: Option<String>,
    /// Browser label of the session that was created.
    pub browser: Option<String>,
    /// Device name a client application reported.
    pub device: Option<String>,
    /// Platform a client application reported.
    pub platform: Option<String>,
    /// Name of the API key that was created.
    pub key_name: Option<String>,
    /// Unix seconds of the event.
    pub time_ts: Option<i64>,
}

/// A rendered message, before it is put in an envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailContent {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Placeholders a body may use. Anything else left in a template is a typo —
/// it would ship as a literal `{foo}` to the reader — so the tests assert that
/// the shipped strings only name these.
pub const PLACEHOLDERS: &[&str] = &[
    "name", "brand", "site_url", "link", "ttl", "ip", "time", "browser", "device", "platform",
    "key_name",
];

fn value(params: &MailParams, name: &str, site_url: &str) -> String {
    match name {
        "name" => params
            .display_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "there".to_string()),
        "site_url" => params
            .site_url
            .clone()
            .unwrap_or_else(|| site_url.to_string()),
        "link" => params.link.clone().unwrap_or_default(),
        "ttl" => params.ttl_days.unwrap_or(0).to_string(),
        "ip" => params.ip.clone().unwrap_or_else(|| "unknown".to_string()),
        "browser" => params
            .browser
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        "device" => params
            .device
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        "platform" => params
            .platform
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        "key_name" => params
            .key_name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unnamed".to_string()),
        "time" => format_utc(params.time_ts.unwrap_or_else(now)),
        other => format!("{{{other}}}"),
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Render a timestamp for a mail body.
///
/// Unlike the Web UI — where every time ships as raw Unix seconds and the
/// browser localizes it — mail has no browser to consult, so the server has to
/// commit to one rendering. It commits to UTC *and says so*: an unlabelled
/// timestamp would be read as local time and mislead the recipient about
/// whether a sign-in was theirs.
pub fn format_utc(ts: i64) -> String {
    let stamp = chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default();
    format!("{} UTC", stamp.format("%Y-%m-%d %H:%M"))
}

/// Escape a value for interpolation into an HTML body.
///
/// Values reach these bodies from client-supplied fields (device names, the
/// `User-Agent` label, an API key name), so they are escaped even though the
/// templates themselves are trusted.
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Render the subject and both bodies for one message.
pub fn render(strings: &MailStrings, kind: MailKind, params: &MailParams) -> MailContent {
    let site_url = params.site_url.clone().unwrap_or_default();
    let key = |part: &str| format!("email.{}.{}", kind.id(), part);

    // Two substitution passes over one value set: bodies interpolate escaped
    // values, the plain-text alternative interpolates them verbatim (escaping
    // there would print `&amp;` to a terminal).
    let text_args: Vec<(&str, String)> = PLACEHOLDERS
        .iter()
        .map(|name| (*name, value(params, name, &site_url)))
        .collect();
    let html_args: Vec<(&str, String)> = text_args
        .iter()
        .map(|(name, raw)| (*name, escape_html(raw)))
        .collect();

    let brand = strings.tr("email.brand").to_string();
    let mut text_args = text_args;
    let mut html_args = html_args;
    text_args.push(("brand", brand.clone()));
    html_args.push(("brand", escape_html(&brand)));

    // The footer interpolates the brand, so it is expanded before the layout.
    let footer_text = strings.trf("email.footer", &[("site_name", brand.clone())]);
    let footer_html = strings.trf("email.footer", &[("site_name", escape_html(&brand))]);

    let subject = strings
        .trf(&key("subject"), &text_args)
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string();
    let body_text = strings.trf(&key("text"), &text_args);
    let body_html = strings.trf(&key("html"), &html_args);

    let text = format!(
        "{body_text}{}",
        strings.trf(
            "email.layout.text",
            &[("brand", brand.clone()), ("footer", footer_text),]
        )
    );
    let html = strings.trf(
        "email.layout.html",
        &[
            ("brand", escape_html(&brand)),
            ("content", body_html),
            ("footer", footer_html),
        ],
    );

    MailContent {
        subject,
        text,
        html,
    }
}

/// Build the raw RFC 5322 message that will be handed to the SMTP transport.
///
/// The result is what gets encrypted into the queue and sent verbatim, so the
/// message is assembled exactly once per attempt set rather than re-rendered at
/// delivery time (a recipient's language could otherwise change under it).
pub fn format_message(
    settings: &EmailSettings,
    to: &str,
    content: &MailContent,
    message_id_domain: &str,
) -> Result<Vec<u8>, MailError> {
    let from_address: Address = settings
        .from_address
        .trim()
        .parse()
        .map_err(|e| MailError::new(format!("invalid sender address: {e}")))?;
    let from_name = settings.from_name.trim();
    let from = Mailbox::new(
        (!from_name.is_empty()).then(|| from_name.to_string()),
        from_address,
    );
    let to_address: Address = to
        .trim()
        .parse()
        .map_err(|e| MailError::new(format!("invalid recipient address: {e}")))?;

    // A generated id is better than the feature-less fallback (`@localhost`),
    // and the domain comes from the configured site URL rather than the host
    // the process happens to run on.
    let message_id = format!("<{}@{}>", uuid::Uuid::new_v4(), message_id_domain);

    let message = Message::builder()
        .from(from)
        .to(Mailbox::new(None, to_address))
        .subject(content.subject.clone())
        .date_now()
        .message_id(Some(message_id))
        .multipart(MultiPart::alternative_plain_html(
            // Both parts are built with `charset=utf-8`, which is what lets a
            // Chinese body survive without a hand-written encoder.
            content.text.clone(),
            content.html.clone(),
        ))
        .map_err(|e| MailError::new(format!("could not build the message: {e}")))?;

    Ok(message.formatted())
}

/// The domain used for the generated `Message-ID`.
pub fn message_id_domain(site_url: &str) -> String {
    let trimmed = site_url.trim();
    let without_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme);
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        "localhost".to_string()
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::mail::settings::{EmailSettings, TlsMode};

    fn params() -> MailParams {
        MailParams {
            display_name: Some("Ada".to_string()),
            site_url: Some("https://files.example.com".to_string()),
            link: Some("https://files.example.com/accounts/password/reset/abc/".to_string()),
            ttl_days: Some(3),
            ip: Some("203.0.113.7".to_string()),
            browser: Some("Firefox on Linux".to_string()),
            device: Some("Pixel 8".to_string()),
            platform: Some("android".to_string()),
            key_name: Some("backup script".to_string()),
            time_ts: Some(1_700_000_000),
        }
    }

    fn settings() -> EmailSettings {
        EmailSettings {
            enabled: true,
            paused: false,
            host: "smtp.example.com".to_string(),
            port: 587,
            tls: TlsMode::StartTls,
            username: String::new(),
            password: None,
            password_set: false,
            from_address: "nanofile@example.com".to_string(),
            from_name: "Nanofile".to_string(),
            timeout_secs: 10,
            max_attempts: 5,
            notify_new_device: true,
            notify_api_key_created: true,
            notify_new_login: true,
        }
    }

    const KINDS: [MailKind; 5] = [
        MailKind::PasswordReset,
        MailKind::NewDevice,
        MailKind::ApiKeyCreated,
        MailKind::NewLogin,
        MailKind::Test,
    ];

    /// Every shipped string may only use placeholders the renderer supplies;
    /// a typo would otherwise ship a literal `{foo}` to the reader.
    #[test]
    fn bodies_only_use_known_placeholders() {
        for lang in ["en", "zh"] {
            let strings = MailStrings::get(Some(lang), lang);
            for kind in KINDS {
                for part in ["subject", "html", "text"] {
                    let key = format!("email.{}.{part}", kind.id());
                    let body = strings.tr(&key);
                    assert_ne!(body, key, "{key} is missing from email_{lang}.toml");
                    for fragment in body.split('{').skip(1) {
                        let Some(name) = fragment.split('}').next() else {
                            continue;
                        };
                        assert!(
                            PLACEHOLDERS.contains(&name),
                            "{key} uses unknown placeholder {{{name}}}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_kind_renders_in_both_languages() {
        for lang in ["en", "zh"] {
            let strings = MailStrings::get(Some(lang), lang);
            for kind in KINDS {
                let content = render(strings, kind, &params());
                assert!(!content.subject.is_empty());
                assert!(
                    !content.subject.contains(['\r', '\n']),
                    "a subject must stay one header line"
                );
                if kind != MailKind::Test {
                    assert!(content.text.contains("Ada"), "text greets the recipient");
                }
                // Neither body may keep an unsubstituted placeholder.
                for body in [&content.text, &content.html] {
                    assert!(
                        !body.contains("{name}") && !body.contains("{time}"),
                        "unsubstituted placeholder left in {body}"
                    );
                }
                assert!(content.html.contains("<!DOCTYPE html>"));
                assert!(content.text.contains("Nanofile"));
                assert_ne!(content.subject, "");
            }
        }
    }

    #[test]
    fn the_reset_link_reaches_both_alternatives() {
        let strings = MailStrings::get(Some("en"), "en");
        let content = render(strings, MailKind::PasswordReset, &params());
        let link = "https://files.example.com/accounts/password/reset/abc/";
        assert!(content.text.contains(link));
        assert!(content.html.contains(link));
        assert!(content.text.contains("3 days"));
        assert!(content.html.contains("203.0.113.7"));
    }

    /// Values a client controls are escaped in HTML but not in text: an
    /// unescaped device name would be an HTML injection into the owner's mail.
    #[test]
    fn client_supplied_values_are_escaped_in_html_only() {
        let strings = MailStrings::get(Some("en"), "en");
        let hostile = MailParams {
            display_name: Some("<script>alert(1)</script>".to_string()),
            device: Some("\"Pixel\" <b>".to_string()),
            key_name: Some("<img src=x onerror=alert(1)>".to_string()),
            ..params()
        };
        for kind in [
            MailKind::NewDevice,
            MailKind::ApiKeyCreated,
            MailKind::NewLogin,
        ] {
            let content = render(strings, kind, &hostile);
            assert!(
                !content.html.contains("<script>"),
                "html body must escape a hostile display name"
            );
            assert!(
                !content.html.contains("onerror=alert(1)>"),
                "html body must escape a hostile key name"
            );
            // The text alternative keeps the raw value: it is not markup, and
            // escaping there would print entities to a terminal.
            assert!(content.text.contains("<script>alert(1)</script>"));
        }
    }

    #[test]
    fn missing_optional_values_fall_back_instead_of_printing_nothing() {
        let strings = MailStrings::get(Some("en"), "en");
        let content = render(strings, MailKind::NewLogin, &MailParams::default());
        assert!(content.text.contains("there"), "unnamed recipient");
        assert!(content.text.contains("unknown"), "unknown ip/browser");
        assert!(
            !content.text.contains('{'),
            "unsubstituted placeholder left in the text body: {}",
            content.text
        );
    }

    #[test]
    fn utc_timestamps_are_labelled() {
        assert_eq!(format_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14 22:13 UTC");
        // A wildly out-of-range value must not panic.
        assert_eq!(format_utc(i64::MAX), "1970-01-01 00:00 UTC");
    }

    #[test]
    fn message_id_domain_comes_from_the_site_url() {
        assert_eq!(
            message_id_domain("https://files.example.com:8443/base"),
            "files.example.com"
        );
        assert_eq!(message_id_domain("http://127.0.0.1:8082"), "127.0.0.1");
        assert_eq!(message_id_domain(""), "localhost");
    }

    #[test]
    fn the_built_message_carries_both_alternatives_and_survives_utf8() {
        let strings = MailStrings::get(Some("zh"), "zh");
        let content = render(strings, MailKind::NewLogin, &params());
        let raw = format_message(
            &settings(),
            "recipient@example.com",
            &content,
            "files.example.com",
        )
        .expect("message builds");

        let text = String::from_utf8(raw).expect("raw message is utf-8");
        assert!(text.contains("From: Nanofile <nanofile@example.com>"));
        assert!(text.contains("To: recipient@example.com"));
        assert!(text.contains("multipart/alternative"));
        assert!(text.contains("charset=utf-8"));
        assert!(
            text.contains("Message-ID: <") && text.contains("@files.example.com>"),
            "the message id names this server"
        );
        assert!(
            text.to_lowercase().contains("date:"),
            "a Date header is required by RFC 5322"
        );
    }

    #[test]
    fn an_invalid_recipient_is_refused_before_any_smtp_traffic() {
        let strings = MailStrings::get(Some("en"), "en");
        let content = render(strings, MailKind::Test, &params());
        let error = format_message(&settings(), "not-an-address", &content, "localhost")
            .expect_err("an invalid recipient must not be queued");
        assert!(error.to_string().contains("recipient"));
    }
}
