//! Minimal DAV: XML helpers (multistatus, lock) built on `quick-xml`: only
//! what the supported methods need — no full property namespace handling.

use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

pub const DAV_NS: &str = "DAV:";

pub struct PropResponse {
    /// Fully-escaped href path (already URL-encoded).
    pub href: String,
    /// Resource properties. `None` produces a 404-status-only response.
    pub props: Option<ResourceProps>,
}

pub struct ResourceProps {
    pub is_collection: bool,
    pub displayname: String,
    /// Absent for collections, like wsgidav's `getcontentlength` (it has no
    /// meaningful size).
    pub getcontentlength: Option<i64>,
    pub getlastmodified: String,
    /// The `getetag` property, quoted like the `ETag` response header. Files only:
    /// the value is the FS object id, which is what `GET` answers with.
    pub etag: Option<String>,
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
    // `BytesText::new` escapes exactly once; escaping here too turned `&` into
    // `&amp;amp;` and an ETag's quotes into `&amp;quot;`, i.e. a different value for
    // any client that compares it with the header.
    let _ = w.write_event(Event::Text(BytesText::new(value)));
    w_end(w, tag);
}

/// Build a PROPFIND `<D:multistatus>` document.
///
/// With `propname_only`, properties are emitted as bare element names (no
/// values), the correct response to `<D:propname/>`.
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
                    if props.getcontentlength.is_some() {
                        w_empty(&mut w, "D:getcontentlength");
                    }
                    if props.etag.is_some() {
                        w_empty(&mut w, "D:getetag");
                    }
                    w_empty(&mut w, "D:getlastmodified");
                    w_empty(&mut w, "D:displayname");
                    w_empty(&mut w, "D:supportedlock");
                    w_empty(&mut w, "D:lockdiscovery");
                } else {
                    w_text(&mut w, "D:displayname", &props.displayname);
                    if let Some(length) = props.getcontentlength {
                        w_text(&mut w, "D:getcontentlength", &length.to_string());
                    }
                    w_text(&mut w, "D:getlastmodified", &props.getlastmodified);
                    if let Some(etag) = props.etag.as_deref() {
                        w_text(&mut w, "D:getetag", etag);
                    }
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
    let token = quick_xml::escape::escape(token);
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock><D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>infinity</D:depth><D:owner/><D:timeout>Infinite</D:timeout><D:locktoken><D:href>{token}</D:href></D:locktoken></D:activelock></D:lockdiscovery></D:prop>"#
    )
}

pub fn build_empty_multistatus() -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    let _ = w.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)));
    let mut multistatus = BytesStart::new("D:multistatus");
    multistatus.push_attribute(("xmlns:D", DAV_NS));
    let _ = w.write_event(Event::Empty(multistatus));
    w.into_inner()
}

/// The RFC 4918 §9.1 refusal for a `Depth: infinity` listing a server will not expand:
/// 403 with the `propfind-finite-depth` precondition tells the client to retry with
/// `Depth: 1` (wsgidav/seafdav emit the same body when infinite depth is disabled).
pub fn build_finite_depth_error() -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    let _ = w.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)));
    let mut error = BytesStart::new("D:error");
    error.push_attribute(("xmlns:D", DAV_NS));
    let _ = w.write_event(Event::Start(error));
    w_empty(&mut w, "D:propfind-finite-depth");
    let _ = w.write_event(Event::End(BytesEnd::new("D:error")));
    w.into_inner()
}
