//! Minimal DAV: XML helpers for WebDAV responses (multistatus, lock, etc.).
//!
//! Uses `quick-xml` to build XML documents. Only what the methods we support
//! need — no full WebDAV property namespace handling.

use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

/// The DAV: namespace prefix used in all responses.
pub const DAV_NS: &str = "DAV:";

/// A single `<D:response>` entry in a PROPFIND multistatus document.
pub struct PropResponse {
    /// Fully-escaped href path (already URL-encoded).
    pub href: String,
    /// Resource properties. `None` produces a 404-status-only response.
    pub props: Option<ResourceProps>,
}

/// Properties returned for a single resource in a multistatus document.
pub struct ResourceProps {
    pub is_collection: bool,
    pub displayname: String,
    pub getcontentlength: i64,
    pub getlastmodified: String,
}

fn w_start(w: &mut Writer<Vec<u8>>, tag: &str) {
    let _ = w.write_event(Event::Start(BytesStart::new(tag)));
}

fn w_end(w: &mut Writer<Vec<u8>>, tag: &str) {
    let _ = w.write_event(Event::End(BytesEnd::new(tag)));
}

fn w_empty(w: &mut Writer<Vec<u8>>, tag: &str) {
    let _ = w.write_event(Event::Empty(BytesStart::new(tag)));
}

fn w_text(w: &mut Writer<Vec<u8>>, tag: &str, value: &str) {
    w_start(w, tag);
    let escaped = quick_xml::escape::escape(value);
    let _ = w.write_event(Event::Text(BytesText::new(escaped.as_ref())));
    w_end(w, tag);
}

/// Build a PROPFIND `<D:multistatus>` document.
///
/// When `propname_only` is true, properties are emitted as bare element names
/// (no values), which is the correct response to `<D:propname/>`.
pub fn build_multistatus(responses: &[PropResponse], propname_only: bool) -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    let _ = w.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)));

    let mut multistatus = BytesStart::new("D:multistatus");
    multistatus.push_attribute(("xmlns:D", DAV_NS));
    let _ = w.write_event(Event::Start(multistatus));

    for resp in responses {
        w_start(&mut w, "D:response");
        w_text(&mut w, "D:href", &resp.href);

        match &resp.props {
            Some(props) => {
                w_start(&mut w, "D:propstat");
                w_start(&mut w, "D:prop");

                if props.is_collection {
                    w_start(&mut w, "D:resourcetype");
                    w_empty(&mut w, "D:collection");
                    w_end(&mut w, "D:resourcetype");
                } else {
                    w_empty(&mut w, "D:resourcetype");
                }

                if propname_only {
                    w_empty(&mut w, "D:getcontentlength");
                    w_empty(&mut w, "D:getlastmodified");
                    w_empty(&mut w, "D:displayname");
                    w_empty(&mut w, "D:supportedlock");
                    w_empty(&mut w, "D:lockdiscovery");
                } else {
                    w_text(&mut w, "D:displayname", &props.displayname);
                    w_text(
                        &mut w,
                        "D:getcontentlength",
                        &props.getcontentlength.to_string(),
                    );
                    w_text(&mut w, "D:getlastmodified", &props.getlastmodified);
                    w_empty(&mut w, "D:supportedlock");
                    w_empty(&mut w, "D:lockdiscovery");
                }

                w_end(&mut w, "D:prop");
                w_text(&mut w, "D:status", "HTTP/1.1 200 OK");
                w_end(&mut w, "D:propstat");
            }
            None => {
                w_text(&mut w, "D:status", "HTTP/1.1 404 Not Found");
            }
        }

        w_end(&mut w, "D:response");
    }

    w_end(&mut w, "D:multistatus");
    w.into_inner()
}

/// Build a no-op LOCK response body containing a fresh lock token.
pub fn build_lock_body(token: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock><D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>infinity</D:depth><D:owner/><D:timeout>Infinite</D:timeout><D:locktoken><D:href>{token}</D:href></D:locktoken></D:activelock></D:lockdiscovery></D:prop>"#
    )
}

/// Build an empty PROPPATCH `<D:multistatus>` response body.
pub fn build_empty_multistatus() -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    let _ = w.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)));
    let mut multistatus = BytesStart::new("D:multistatus");
    multistatus.push_attribute(("xmlns:D", DAV_NS));
    let _ = w.write_event(Event::Empty(multistatus));
    w.into_inner()
}
