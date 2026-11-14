//! The HTTP application: which routes exist, and what wraps them.
//!
//! It lives in the library rather than `main.rs` because `tests/` can only link
//! a library target, and a second assembly inside the binary let the tests
//! exercise a router the server does not run. There is one assembly now.
//!
//! [`app_routes`] is the routing surface: every route, the per-group body
//! limits, CORS, the HTML error pages and the fallback. [`build_app`] adds the
//! operational layers (global body limits, tracing, the request timeout,
//! security headers). Neither returns a stated router: the caller applies
//! `.with_state(state)`.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use std::sync::Arc;

use crate::AppState;

/// Path segments immediately preceding a capability token. Routes such as
/// `/f/{token}` and `/zip/{token}` carry the credential itself in the path, and
/// it is deliberately unpaired with a session, so a logged URI is a replay.
const TOKEN_PATH_PREFIXES: &[&str] = &[
    "f",
    "d",
    "u",
    "zip",
    "blks",
    "download-api",
    "upload-api",
    "upload-aj",
    "update-api",
    "update-aj",
    "upload-blks-api",
    "upload-raw-blks-api",
    "client-sso",
    "client-login",
    "client-sso-link",
    // Capability URLs the API itself mints and returns.
    "share-links",
    "shared-links",
    "upload-links",
    // One-shot account credentials carried in a browser path.
    "reset",
    "verify-email",
];

/// Replace capability-token path segments with `{token}` for logging. The query
/// string is dropped by the caller; it may carry a share-link password or other
/// credentials.
pub fn redact_request_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let mut out = String::with_capacity(path.len());
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        let prev = if i > 0 { segs[i - 1] } else { "" };
        if !seg.is_empty() && TOKEN_PATH_PREFIXES.contains(&prev) {
            out.push_str("{token}");
        } else {
            out.push_str(seg);
        }
    }
    out
}

/// `/health` — readiness, used by the container healthcheck and by anything that
/// waits for the listener without touching the API.
///
/// The body stays empty; the generation header is what the "restarting" page
/// watches, because a plain 200 cannot tell the old server from the new.
pub async fn health_check() -> impl axum::response::IntoResponse {
    [(
        axum::http::HeaderName::from_static(crate::restart::GENERATION_HEADER),
        crate::restart::generation().to_string(),
    )]
}

/// The route tree and every middleware that decides what a request reaches.
///
/// Order matters twice: CORS is applied to the API group *before* it is merged,
/// or it would also wrap WebDAV and `CorsLayer` would answer `OPTIONS` itself,
/// shadowing the `DAV:`/`Allow:` responses WebDAV clients expect; and the error
/// middleware wraps the page groups only, since the endpoints the frontend
/// fetches itself must keep answering with the wire protocol.
pub fn app_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    // Upload-capable route groups accept bodies up to `max_upload_size_mb`; the
    // global default in `build_app` is the much smaller JSON/Form cap.
    let upload_body_limit = crate::body_limit::upload_limit();

    Router::new()
        .route("/health", get(health_check))
        .merge(crate::routes::api_routes().layer(cors_layer(&state.config().server)))
        .merge(crate::handler::sync::sync_routes().layer(DefaultBodyLimit::max(upload_body_limit)))
        .merge(
            crate::handler::web::web_api_routes()
                .layer(DefaultBodyLimit::max(upload_body_limit))
                // The upload/download/zip endpoints are fetched directly by the
                // browser (and by clients on another origin), so they need the
                // same CORS handling as the API group: upstream's fileserver
                // answers OPTIONS and sets `Access-Control-Allow-Origin`.
                .layer(cors_layer(&state.config().server)),
        )
        .merge(
            crate::handler::web::web_page_routes()
                .layer(DefaultBodyLimit::max(upload_body_limit))
                .layer(from_fn_with_state(
                    state.clone(),
                    crate::ui::error_page::anonymous_pages,
                )),
        )
        .merge(crate::ui::ui_routes().layer(from_fn_with_state(
            state.clone(),
            crate::ui::error_page::session_pages,
        )))
        .merge(crate::notification::notification_routes())
        .merge(crate::webdav::webdav_routes().layer(DefaultBodyLimit::max(upload_body_limit)))
        .merge(crate::handler::avatar::image_routes())
        // Count work in flight, for the load gauge. Applied here rather than in
        // `build_app` so the static-asset route stays outside it: a page load
        // fetching a dozen assets is not a busy server.
        .layer(from_fn_with_state(
            state.clone(),
            crate::middleware::load_guard,
        ))
        .route("/static/{*path}", get(crate::static_assets::serve_static))
        // No route matched. A browser gets the error page; a client keeps the
        // body-less 404 it has always received.
        .fallback(crate::ui::error_page::unknown_path)
}

/// What the binary serves: [`app_routes`] plus the operational layers.
/// These are what a *deployment* wants rather than a route needs: the global
/// body limits, request tracing with the path redacted, the whole-request
/// timeout and the security headers. Separable so a route test need not run
/// under a tracer and a 10-minute timeout.
pub fn build_app(state: Arc<AppState>) -> Router {
    let config = &state.config().server;

    let app = app_routes(&state)
        // The default for every route; upload-capable groups raised their own.
        .layer(DefaultBodyLimit::max(
            (config.max_json_body_mb * 1024 * 1024) as usize,
        ))
        // Hard cap on what a client may send at all, checked before anything buffers it.
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            (config.max_upload_size_mb * 1024 * 1024) as usize,
        ))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(RedactingMakeSpan)
                .on_request(TransferAwareOnRequest)
                .on_response(TransferAwareOnResponse)
                .on_failure(tower_http::trace::DefaultOnFailure::new().level(tracing::Level::WARN)),
        )
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(config.request_timeout_secs),
        ))
        .layer(from_fn_with_state(
            state.clone(),
            crate::middleware::security_headers,
        ));

    // Optionally bound how long a client may take to send the request body. Off
    // by default: `request_timeout_secs` already caps the whole handler.
    let app = if config.body_timeout_secs > 0 {
        app.layer(tower_http::timeout::RequestBodyTimeoutLayer::new(
            std::time::Duration::from_secs(config.body_timeout_secs),
        ))
    } else {
        app
    };

    app.with_state(state)
}

/// Whether this is one block transfer of the sync protocol.
///
/// `GET`/`PUT /seafhttp/repo/{id}/block/{block_id}` is the bulk of a sync upload or
/// download: one request per content block, so two INFO lines each turn a large
/// upload into tens of thousands of lines and serialize every request behind the
/// log writer. These are logged at DEBUG instead.
fn is_block_transfer(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    if !matches!(*method, Method::GET | Method::PUT) {
        return false;
    }
    let Some(rest) = path.strip_prefix("/seafhttp/repo/") else {
        return false;
    };
    let mut parts = rest.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(repo), Some("block"), Some(block), None)
            if !repo.is_empty() && !block.is_empty()
    )
}

/// Span target that marks a per-block transfer. The response half cannot see the
/// request, so [`RedactingMakeSpan`] stamps the span with this target and
/// [`TransferAwareOnResponse`] reads it back from the span's metadata.
const BLOCK_TRANSFER_TARGET: &str = "nanofile::sync_block";

/// The `make_span` half of the trace layer: a span named from the request, with
/// the path redacted. A named type so the redaction has somewhere to be tested.
#[derive(Clone, Copy)]
struct RedactingMakeSpan;

impl<B> tower_http::trace::MakeSpan<B> for RedactingMakeSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> tracing::Span {
        let method = request.method();
        let path = redact_request_path(request.uri().path());
        // The span itself stays at INFO so `on_response` can inspect its metadata;
        // only the request/response *events* drop to DEBUG for a block transfer.
        if is_block_transfer(method, request.uri().path()) {
            tracing::info_span!(
                target: BLOCK_TRANSFER_TARGET,
                "request",
                method = %method,
                path = %path,
                latency = tracing::field::Empty,
                status = tracing::field::Empty,
            )
        } else {
            tracing::info_span!(
                "request",
                method = %method,
                path = %path,
                latency = tracing::field::Empty,
                status = tracing::field::Empty,
            )
        }
    }
}

/// The request half of the trace layer: the same line, at the level a block
/// transfer deserves.
#[derive(Clone, Copy)]
struct TransferAwareOnRequest;

impl<B> tower_http::trace::OnRequest<B> for TransferAwareOnRequest {
    fn on_request(&mut self, request: &axum::http::Request<B>, span: &tracing::Span) {
        let level = if is_block_transfer(request.method(), request.uri().path()) {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        };
        tower_http::trace::DefaultOnRequest::new()
            .level(level)
            .on_request(request, span);
    }
}

/// The response half: the span target marks a block transfer (see
/// [`RedactingMakeSpan`]), and its completion line follows the request line down
/// to DEBUG.
#[derive(Clone, Copy)]
struct TransferAwareOnResponse;

impl<B> tower_http::trace::OnResponse<B> for TransferAwareOnResponse {
    fn on_response(
        self,
        response: &axum::http::Response<B>,
        latency: std::time::Duration,
        span: &tracing::Span,
    ) {
        let level = match span.metadata() {
            Some(metadata) if metadata.target() == BLOCK_TRANSFER_TARGET => tracing::Level::DEBUG,
            _ => tracing::Level::INFO,
        };
        tower_http::trace::DefaultOnResponse::new()
            .level(level)
            .on_response(response, latency, span);
    }
}

/// Build the CORS layer for the REST API. `cors_origins()` returns
/// `[site_url_origin()]` when the configured list is empty, so the same-origin
/// site is always allowed.
fn cors_layer(config: &infra::config::ServerConfig) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{AllowOrigin, CorsLayer};

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(
            config.cors_origins().into_iter().filter_map(|origin| {
                origin
                    .parse()
                    .map_err(|e| {
                        tracing::warn!("Skipping invalid CORS origin '{}': {:?}", origin, e)
                    })
                    .ok()
            }),
        ))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("x-requested-with"),
            header::HeaderName::from_static("x-seafile-otp"),
            header::HeaderName::from_static("x-seafile-s2fa"),
            header::HeaderName::from_static("x-seafile-sharelink-password"),
            header::HeaderName::from_static("x-seafile-2fa-trust-device"),
        ])
        .max_age(std::time::Duration::from_secs(config.cors_max_age_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capability_token_never_reaches_the_log() {
        // The token is the credential; a log line that carries it is a replay.
        assert_eq!(redact_request_path("/f/abc123/"), "/f/{token}/");
        assert_eq!(
            redact_request_path("/d/abc123/files/a.txt"),
            "/d/{token}/files/a.txt"
        );
        assert_eq!(redact_request_path("/zip/abc123"), "/zip/{token}");
        assert_eq!(
            redact_request_path("/download-api/abc123"),
            "/download-api/{token}"
        );
        // ...and the rest of the path still says where the request was going.
        assert_eq!(
            redact_request_path("/blks/abc123/file-id/block-id"),
            "/blks/{token}/file-id/block-id"
        );
        assert_eq!(
            redact_request_path("/api/v2.1/share-links/abc123/"),
            "/api/v2.1/share-links/{token}/"
        );
        assert_eq!(
            redact_request_path("/api2/shared-links/abc123"),
            "/api2/shared-links/{token}"
        );
        assert_eq!(
            redact_request_path("/api/v2.1/upload-links/abc123/upload/"),
            "/api/v2.1/upload-links/{token}/upload/"
        );
        assert_eq!(
            redact_request_path("/api2/upload-links/abc123"),
            "/api2/upload-links/{token}"
        );
        assert_eq!(
            redact_request_path("/accounts/password/reset/abc123/"),
            "/accounts/password/reset/{token}/"
        );
        assert_eq!(
            redact_request_path("/accounts/verify-email/abc123/"),
            "/accounts/verify-email/{token}/"
        );
    }

    #[test]
    fn an_ordinary_path_is_left_alone() {
        assert_eq!(redact_request_path("/libraries/"), "/libraries/");
        assert_eq!(
            redact_request_path("/libraries/abc/files/sub/dir"),
            "/libraries/abc/files/sub/dir"
        );
        // A segment that merely contains a prefix is not a token position.
        assert_eq!(redact_request_path("/files/abc/"), "/files/abc/");
        assert_eq!(redact_request_path("/"), "/");
    }

    /// Only the sync protocol's per-block transfers are demoted to DEBUG: they are
    /// one request per block, while their neighbours in the same protocol are not.
    #[test]
    fn only_block_transfers_are_demoted() {
        use axum::http::Method;

        let repo = "290e789d-ba03-47c3-a4cd-4dba7bb3fff2";
        let block = "531350019959b6817a996fa7b8fc90ed27a05afc";
        assert!(is_block_transfer(
            &Method::PUT,
            &format!("/seafhttp/repo/{repo}/block/{block}")
        ));
        assert!(is_block_transfer(
            &Method::GET,
            &format!("/seafhttp/repo/{repo}/block/{block}")
        ));

        for (method, path) in [
            (Method::POST, format!("/seafhttp/repo/{repo}/check-blocks/")),
            (Method::PUT, format!("/seafhttp/repo/{repo}/commit/HEAD/")),
            (Method::POST, format!("/seafhttp/repo/{repo}/recv-fs/")),
            (Method::PUT, format!("/seafhttp/repo/{repo}/block/")),
            (
                Method::PUT,
                format!("/seafhttp/repo/{repo}/block/{block}/extra"),
            ),
            (Method::POST, format!("/seafhttp/repo/{repo}/block/{block}")),
            (Method::GET, format!("/api2/repos/{repo}/")),
            (Method::PUT, "/seafhttp/repo/block/x".to_string()),
        ] {
            assert!(
                !is_block_transfer(&method, &path),
                "{method} {path} must stay at INFO"
            );
        }
    }
}
