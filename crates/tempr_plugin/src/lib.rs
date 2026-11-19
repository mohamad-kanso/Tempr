//! The plugin API (docs/08-plugin-api.md, D8, D31).
//!
//! This crate is the **only** surface a plugin may depend on. It re-exports
//! the GPUI types a panel needs and the database-driver traits, and it owns
//! the manifest, the lifecycle trait, the registration context and the
//! extension-point traits. Nothing from `tempr_services`, `tempr_ui`,
//! `tempr_workspace` or `tempr_intel` may appear here — that rule is what
//! keeps the public API honest, because anything a plugin needs has to be
//! re-exported deliberately.
//!
//! The API version stays `0.x` until the user declares it ready (D31);
//! `0.x` means the surface may change without a compatibility guarantee.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod command;
pub mod context;
pub mod error;
mod extension;
pub mod reexport;

pub use command::PluginCommand;
pub use context::PluginContext;
pub use error::PluginError;
pub use extension::{
    CommandContext, CommandOutcome, CompletionProvider, Panel, PluginCommandSpec, ResultRenderer,
};

use tempr_domain::PluginId;

/// Version of the plugin API the host implements. A plugin declares the
/// version it was written against; activation rejects a different major.
pub const PLUGIN_API_VERSION: (u16, u16) = (0, 1);

/// Highest priority a plugin completion item may claim. The host maps it to
/// `WEIGHT_PLUGIN_BASE (100) + priority`, so plugin items rank between
/// keywords and statement keywords but never above a schema object.
pub const PLUGIN_PRIORITY_MAX: i32 = 300;

/// What a plugin can declare it provides. Advisory: registration happens in
/// [`Plugin::activate`] and is what actually counts. The list exists for
/// discovery and for logging mismatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Driver { engine: &'static str },
    CompletionProvider,
    Theme,
    CommandContribution,
    PanelContribution,
    ResultRenderer,
}

impl Capability {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Driver { .. } => "driver",
            Self::CompletionProvider => "completion-provider",
            Self::Theme => "theme",
            Self::CommandContribution => "command",
            Self::PanelContribution => "panel",
            Self::ResultRenderer => "result-renderer",
        }
    }
}

/// Stable identity and capability metadata for a plugin.
#[derive(Debug, Clone)]
pub struct PluginManifest {
    /// Reverse-domain namespace, unique across all linked plugins:
    /// `dev.tempr.postgres`. Command and panel ids must be namespaced with
    /// it (`dev.tempr.postgres::refresh`).
    pub id: &'static str,
    pub name: &'static str,
    /// The plugin's own version, unrelated to the API version.
    pub version: &'static str,
    /// The API version this plugin was written against.
    pub api_version: (u16, u16),
    /// Plugin ids that must activate first.
    pub requires: &'static [&'static str],
    pub capabilities: &'static [Capability],
}

impl PluginManifest {
    /// The domain id for this plugin, derived from `id` so it is stable
    /// across launches.
    pub fn plugin_id(&self) -> PluginId {
        PluginId::derived(self.id)
    }

    /// Reject a plugin whose namespace is malformed or whose API major is not
    /// the host's. Static plugins cannot actually mismatch today (they are
    /// compiled in), but the check is the contract that dynamic loading will
    /// rely on.
    pub fn validate(&self) -> Result<(), PluginError> {
        if !is_valid_namespace(self.id) {
            return Err(PluginError::InvalidNamespace { id: self.id });
        }
        if self.api_version.0 != PLUGIN_API_VERSION.0 {
            return Err(PluginError::IncompatibleApiVersion {
                plugin: self.id.to_string(),
                plugin_api: self.api_version,
                host_api: PLUGIN_API_VERSION,
            });
        }
        Ok(())
    }

    /// True when `id` (`dev.tempr.x::command`) belongs to this plugin.
    pub fn owns(&self, id: &str) -> bool {
        id.strip_prefix(self.id)
            .and_then(|rest| rest.strip_prefix("::"))
            .is_some_and(|local| !local.is_empty())
    }
}

/// `a-z0-9-` segments separated by dots, at least two segments.
fn is_valid_namespace(id: &str) -> bool {
    let segments: Vec<&str> = id.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// A static plugin: a Rust crate linked into the binary, activated once at
/// startup.
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;

    /// Register capabilities through `cx`. Returning `Err` discards every
    /// registration this plugin made; the host logs it and continues.
    fn activate(&mut self, cx: &mut PluginContext<'_>) -> Result<(), PluginError>;

    /// Release resources. Registrations are dropped whether or not this runs.
    fn deactivate(&mut self) {}
}

/// Everything a plugin may touch. Deliberately narrow: registrations and the
/// event bus, never a service registry or storage handle (08-plugin-api's
/// rationale — a plugin that needs data subscribes to the bus).
pub fn api_version() -> (u16, u16) {
    PLUGIN_API_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &'static str, api: (u16, u16)) -> PluginManifest {
        PluginManifest {
            id,
            name: "Test",
            version: "0.1.0",
            api_version: api,
            requires: &[],
            capabilities: &[],
        }
    }

    #[test]
    fn accepts_a_host_compatible_manifest() {
        manifest("dev.tempr.test", PLUGIN_API_VERSION)
            .validate()
            .expect("valid");
    }

    #[test]
    fn rejects_a_different_api_major() {
        let err = manifest("dev.tempr.test", (1, 0))
            .validate()
            .expect_err("major mismatch");
        assert!(matches!(err, PluginError::IncompatibleApiVersion { .. }));
    }

    #[test]
    fn a_minor_mismatch_is_accepted() {
        manifest("dev.tempr.test", (0, 7))
            .validate()
            .expect("minor ok");
    }

    #[test]
    fn rejects_malformed_namespaces() {
        for id in ["tempr", "", "Dev.tempr", "dev..x", "dev.tempr.x!"] {
            assert!(
                manifest(id, PLUGIN_API_VERSION).validate().is_err(),
                "{id} should be rejected"
            );
        }
    }

    #[test]
    fn ownership_is_namespace_scoped() {
        let m = manifest("dev.tempr.demo", PLUGIN_API_VERSION);
        assert!(m.owns("dev.tempr.demo::run"));
        assert!(!m.owns("dev.tempr.demo"));
        assert!(!m.owns("dev.tempr.demo::"));
        assert!(!m.owns("dev.tempr.demox::run"));
        assert!(!m.owns("other::run"));
    }

    #[test]
    fn plugin_ids_are_derived_from_the_namespace() {
        let a = manifest("dev.tempr.demo", PLUGIN_API_VERSION).plugin_id();
        let b = manifest("dev.tempr.demo", PLUGIN_API_VERSION).plugin_id();
        let c = manifest("dev.tempr.other", PLUGIN_API_VERSION).plugin_id();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
