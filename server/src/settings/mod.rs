//! Server-side settings service: the layers (environment, config file, saved
//! rows), the effective-value computation and the admin save path.
//!
//! `infra::settings` owns the catalog, the layering rules and the replaceable
//! snapshot (`RuntimeConfig`); this module owns the service that applies them.

pub mod service;

pub use infra::settings::RuntimeConfig;
pub use service::{SaveOutcome, SecretState, SettingsForm, SettingsLayers, SettingsService};
