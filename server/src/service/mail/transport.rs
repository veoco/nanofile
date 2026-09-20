//! The SMTP transport: how a rendered message actually leaves the process.
//!
//! Split from [`super::message`] so the message builder can be tested without a
//! socket, and so the one place that decides TLS/authentication is small enough
//! to read in full. Certificate validation is always on: there is deliberately
//! no "accept any certificate" switch, because a mail server that hands a reset
//! link to a man in the middle is worse than one that refuses to send.

use std::time::Duration;

use lettre::AsyncTransport as _;
use lettre::Tokio1Executor;
use lettre::address::{Address, Envelope};
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;

use super::MailError;
use super::settings::{EmailSettings, TlsMode};

/// A transport bound to one snapshot of the settings.
///
/// Rebuilt per attempt rather than cached: a connection pool would keep working
/// after the administrator changed the host, and `lettre`'s pool feature is off
/// for that reason. One connection per attempt is exactly the volume a
/// notification system produces.
pub struct MailTransport {
    inner: AsyncSmtpTransport<Tokio1Executor>,
    timeout: Duration,
}

impl MailTransport {
    /// Build a transport for these settings.
    ///
    /// `hello_name` is the domain announced in `EHLO`; it cannot come from the
    /// machine's hostname because the `hostname` feature is off, and the site
    /// URL is a better answer anyway (a container's hostname means nothing to
    /// the relay).
    pub fn new(settings: &EmailSettings, hello_name: &str) -> Result<Self, MailError> {
        let host = settings.host.trim();
        if host.is_empty() {
            return Err(MailError::new("no SMTP host is configured"));
        }

        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(settings.port)
            .hello_name(ClientId::Domain(hello_name.to_string()))
            .timeout(Some(Duration::from_secs(settings.timeout_secs)));

        builder = match settings.tls {
            TlsMode::None => builder.tls(Tls::None),
            // The TLS parameters are built against the configured host, so a
            // certificate for another name is rejected.
            TlsMode::StartTls => builder.tls(Tls::Required(tls_parameters(host)?)),
            TlsMode::ImplicitTls => builder.tls(Tls::Wrapper(tls_parameters(host)?)),
        };

        if let (Some(username), Some(password)) =
            (non_empty(&settings.username), settings.password.as_deref())
        {
            builder = builder
                .credentials(Credentials::new(username, password.to_string()))
                // PLAIN first, LOGIN as the fallback the older servers need;
                // XOAUTH2 is not offered because there is no token to use.
                .authentication(vec![Mechanism::Plain, Mechanism::Login]);
        }

        Ok(Self {
            inner: builder.build(),
            timeout: Duration::from_secs(settings.timeout_secs),
        })
    }

    /// Send raw RFC 5322 bytes to one recipient.
    ///
    /// The envelope is derived from the settings and the recipient rather than
    /// from the message headers, so a `Bcc`-like header injected into a body
    /// cannot add recipients.
    pub async fn send_raw(&self, from: &str, to: &str, raw: Vec<u8>) -> Result<(), MailError> {
        let from_address: Address = from
            .trim()
            .parse()
            .map_err(|e| MailError::new(format!("invalid sender address: {e}")))?;
        let to_address: Address = to
            .trim()
            .parse()
            .map_err(|e| MailError::new(format!("invalid recipient address: {e}")))?;
        let envelope = Envelope::new(Some(from_address), vec![to_address])
            .map_err(|e| MailError::new(format!("invalid envelope: {e}")))?;

        // The builder's own timeout covers the socket; this outer bound also
        // covers a server that accepts the connection and then stalls during
        // the conversation, so a login request can never wait forever on mail.
        let send = self.inner.send_raw(&envelope, &raw);
        match tokio::time::timeout(self.timeout, send).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(MailError::new(format!("SMTP delivery failed: {e}"))),
            Err(_) => Err(MailError::new(format!(
                "SMTP delivery timed out after {}s",
                self.timeout.as_secs()
            ))),
        }
    }
}

fn tls_parameters(host: &str) -> Result<TlsParameters, MailError> {
    TlsParameters::new(host.to_string())
        .map_err(|e| MailError::new(format!("could not prepare TLS for {host}: {e}")))
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::mail::settings::SettingsOrigin;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    /// A scripted SMTP server good enough for one delivery.
    ///
    /// Written by hand rather than pulled from a crate so the test can assert on
    /// exactly what the client sent — including whether it tried to authenticate
    /// or upgrade to TLS — without a network or a fixture binary.
    struct SmtpStub {
        port: u16,
        received: Arc<Mutex<Vec<String>>>,
        /// Reply given to `RCPT TO`, so a refusal can be exercised.
        rcpt_reply: &'static str,
    }

    impl SmtpStub {
        async fn start(rcpt_reply: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let port = listener.local_addr().expect("addr").port();
            let received = Arc::new(Mutex::new(Vec::new()));
            let stub = Self {
                port,
                received: received.clone(),
                rcpt_reply,
            };
            let rcpt_reply = stub.rcpt_reply;
            tokio::spawn(async move {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                serve(socket, &received, rcpt_reply).await;
            });
            stub
        }

        fn commands(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }

        /// Everything between `DATA` and the terminating dot.
        fn data(&self) -> String {
            self.commands()
                .iter()
                .find(|line| line.starts_with("__data__"))
                .cloned()
                .unwrap_or_default()
        }
    }

    async fn serve(socket: TcpStream, received: &Arc<Mutex<Vec<String>>>, rcpt_reply: &str) {
        let (read_half, mut write_half) = socket.into_split();
        let mut lines = BufReader::new(read_half).lines();

        let _ = write_half.write_all(b"220 stub.test ESMTP\r\n").await;
        while let Ok(Some(line)) = lines.next_line().await {
            received.lock().unwrap().push(line.clone());
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                // Advertise nothing: no STARTTLS, no AUTH, no PIPELINING. The
                // client must then send the message without either.
                let _ = write_half
                    .write_all(b"250-stub.test\r\n250 8BITMIME\r\n")
                    .await;
            } else if upper.starts_with("MAIL FROM") {
                let _ = write_half.write_all(b"250 OK\r\n").await;
            } else if upper.starts_with("RCPT TO") {
                let reply = format!("{rcpt_reply}\r\n");
                let _ = write_half.write_all(reply.as_bytes()).await;
            } else if upper.starts_with("DATA") {
                let _ = write_half
                    .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                    .await;
                let mut body = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line == "." {
                        break;
                    }
                    body.push_str(&line);
                    body.push('\n');
                }
                received.lock().unwrap().push(format!("__data__{body}"));
                let _ = write_half.write_all(b"250 OK queued\r\n").await;
            } else if upper.starts_with("QUIT") {
                let _ = write_half.write_all(b"221 Bye\r\n").await;
                break;
            } else {
                let _ = write_half.write_all(b"250 OK\r\n").await;
            }
        }
    }

    fn plaintext_settings(port: u16) -> EmailSettings {
        EmailSettings {
            origin: SettingsOrigin::Stored,
            enabled: true,
            paused: false,
            host: "127.0.0.1".to_string(),
            port,
            tls: TlsMode::None,
            username: String::new(),
            password: None,
            password_set: false,
            password_broken: false,
            from_address: "nanofile@example.com".to_string(),
            from_name: "Nanofile".to_string(),
            timeout_secs: 5,
            max_attempts: 3,
            notify_new_device: true,
            notify_api_key_created: true,
            notify_new_login: true,
            updated_at: 0,
            updated_by: None,
        }
    }

    /// The client must not talk TLS or authenticate on its own initiative: a
    /// server that advertises neither must still receive the message, and no
    /// credentials may be volunteered.
    #[tokio::test]
    async fn delivers_a_message_and_never_upgrades_on_its_own() {
        let stub = SmtpStub::start("250 OK").await;
        let settings = plaintext_settings(stub.port);
        let transport = MailTransport::new(&settings, "files.example.com").expect("transport");

        let raw = b"Subject: hello\r\n\r\nbody line\r\n".to_vec();
        transport
            .send_raw("nanofile@example.com", "user@example.com", raw)
            .await
            .expect("delivered");

        let commands = stub.commands();
        let joined = commands.join("\n").to_ascii_uppercase();
        assert!(joined.contains("EHLO FILES.EXAMPLE.COM"), "{joined}");
        assert!(
            joined.contains("MAIL FROM:<NANOFILE@EXAMPLE.COM>"),
            "{joined}"
        );
        assert!(joined.contains("RCPT TO:<USER@EXAMPLE.COM>"), "{joined}");
        assert!(
            !joined.contains("STARTTLS"),
            "TLS must be required explicitly, never negotiated by default"
        );
        assert!(!joined.contains("AUTH"), "no credentials were configured");
        // The body arrived intact, after the dot-stuffing rules.
        assert!(stub.data().contains("Subject: hello"));
        assert!(stub.data().contains("body line"));
    }

    #[tokio::test]
    async fn a_refused_recipient_is_an_error_not_a_silent_success() {
        let stub = SmtpStub::start("550 No such user here").await;
        let settings = plaintext_settings(stub.port);
        let transport = MailTransport::new(&settings, "files.example.com").expect("transport");

        let error = transport
            .send_raw(
                "nanofile@example.com",
                "missing@example.com",
                b"Subject: x\r\n\r\nx\r\n".to_vec(),
            )
            .await
            .expect_err("a 550 must surface");
        assert!(
            error.to_string().to_lowercase().contains("550")
                || error.to_string().to_lowercase().contains("delivery"),
            "the reason must be actionable: {error}"
        );
    }

    #[tokio::test]
    async fn bad_addresses_are_refused_before_connecting() {
        let settings = plaintext_settings(9);
        let transport = MailTransport::new(&settings, "localhost").expect("transport");
        assert!(
            transport
                .send_raw("nanofile@example.com", "not-an-address", Vec::new())
                .await
                .is_err()
        );
        assert!(
            transport
                .send_raw("also-not-an-address", "user@example.com", Vec::new())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_missing_host_is_refused_rather_than_defaulted() {
        let mut settings = plaintext_settings(25);
        settings.host = "   ".to_string();
        let error = match MailTransport::new(&settings, "localhost") {
            Ok(_) => panic!("an empty host must not build a transport"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("SMTP host"));
    }

    #[tokio::test]
    async fn an_unreachable_server_fails_instead_of_hanging() {
        // Port 1 is not listening, and the settings bound the attempt.
        let mut settings = plaintext_settings(1);
        settings.timeout_secs = 2;
        let transport = MailTransport::new(&settings, "localhost").expect("transport");
        let error = transport
            .send_raw(
                "nanofile@example.com",
                "user@example.com",
                b"Subject: x\r\n\r\nx\r\n".to_vec(),
            )
            .await
            .expect_err("nothing listens there");
        assert!(!error.to_string().is_empty());
    }
}
