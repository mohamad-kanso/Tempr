//! `tempr_intel` — the SQL intelligence model layer, no UI and no I/O on the
//! request path (docs/12-sql-intelligence.md, ADR-0009).
//!
//! Stage 3 hands over an immutable in-memory `CatalogCache` built from a
//! `SchemaSnapshot`. The `SemanticEngine` (scopes, completion, hover) lands
//! in later stages and holds the cache behind an `Arc` swap.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod analysis;
pub mod catalog;
pub mod semantic;

#[cfg(test)]
mod probe;

pub use analysis::{Analysis, Binding, ColumnRef, Diagnostic, DiagnosticKind, Scope, Severity};
pub use catalog::{CatalogCache, CatalogEntry, CatalogStats};
pub use semantic::SemanticEngine;
