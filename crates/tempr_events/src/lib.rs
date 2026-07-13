#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod bus;
pub mod event;

pub use bus::{EventBus, Subscription};
pub use event::{AppEvent, AppEventKind, EventFilter, PluginPayload};
