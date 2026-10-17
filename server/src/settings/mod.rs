//! Server-side settings: the replaceable runtime snapshot and the service that
//! layers the environment, the config file and the saved rows.
//!
//! `infra::settings` owns the catalog and layering rules; this module owns the state.

pub mod runtime;
pub mod service;

pub use runtime::RuntimeConfig;
pub use service::{SaveOutcome, SecretState, SettingsForm, SettingsLayers, SettingsService};
