//! The jobs a confined child can run.
//!
//! Each job owns its own request and reply framing; the transport
//! ([`crate::sandbox::worker`]) carries bytes and one tag, so a single child
//! binary serves every pipeline without any of them being able to read another's
//! reply as its own.
//!
//! What they have in common is why they are here: every one of them turns bytes
//! a user supplied into something the server keeps, and the code that does that
//! runs on the far side of the process boundary, where a parser that is
//! exploited reaches nothing.

pub mod images;
pub mod media;
