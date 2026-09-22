//! Server-side settings: the replaceable runtime snapshot and, later, the
//! resolver that layers environment, config file and database values.
//!
//! `infra::settings` owns the catalog and the layering rules; this module owns
//! the live state they are applied to.

pub mod runtime;

pub use runtime::RuntimeConfig;
