//! Server-side settings: the replaceable runtime snapshot and the service that
//! layers the environment, the config file and the saved rows.
//!
//! `infra::settings` owns the catalog and the layering rules; this module owns
//! the live state they are applied to and the persistence an administrator's
//! save goes through.

pub mod runtime;
pub mod service;

pub use runtime::RuntimeConfig;
pub use service::{SaveOutcome, SecretState, SettingsForm, SettingsLayers, SettingsService};
