//! The jobs a confined child can run.
//!
//! Each owns its request and reply framing; the transport
//! ([`crate::sandbox::worker`]) carries bytes and one tag, so one child binary serves
//! every pipeline and no job can read another's reply. All of them turn user bytes
//! into server state on the far side of the process boundary.

pub mod images;
pub mod media;
