//! Typed failures. Every one of these is reported per plugin, never fatal to
//! the host: activation marks the plugin failed, discards its registrations
//! and the app continues (docs/08-plugin-api.md "Plugin Lifecycle").

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin `{plugin}` targets plugin API {plugin_api:?}, host is {host_api:?}")]
    IncompatibleApiVersion {
        plugin: String,
        plugin_api: (u16, u16),
        host_api: (u16, u16),
    },

    #[error("plugin id `{id}` is not a lowercase reverse-domain namespace")]
    InvalidNamespace { id: &'static str },

    #[error("duplicate {kind} registration `{id}`")]
    DuplicateRegistration { kind: &'static str, id: String },

    #[error("plugin `{plugin}` tried to register `{id}`, which is not in its namespace")]
    ForeignNamespace { plugin: String, id: String },

    #[error("plugin `{plugin}` requires `{missing}`, which is not linked")]
    MissingDependency { plugin: String, missing: String },

    #[error("plugin dependency cycle between {plugins:?}")]
    DependencyCycle { plugins: Vec<String> },

    #[error("plugin `{plugin}` failed to activate: {message}")]
    ActivationFailed { plugin: String, message: String },

    #[error("plugin `{plugin}` panicked while activating")]
    ActivationPanicked { plugin: String },

    #[error("{0}")]
    InvalidRegistration(String),
}

impl PluginError {
    /// Which registration kind the error names, for logging.
    pub fn registration_kind(&self) -> Option<&'static str> {
        match self {
            Self::DuplicateRegistration { kind, .. } => Some(kind),
            _ => None,
        }
    }
}
