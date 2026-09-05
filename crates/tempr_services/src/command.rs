//! `CommandService` — the registry of every user action and the resolved
//! keybinding map (docs/05-services.md → CommandService).
//!
//! The service owns *metadata and configuration*: ids, titles, categories,
//! key contexts, default keystrokes, and user/workspace overrides. It does
//! not execute anything — commands are GPUI actions dispatched by the UI
//! layer, which then calls `record_executed` so the event bus sees
//! `CommandExecuted` (D23).

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_domain::CommandId;
use tempr_events::{AppEvent, EventBus};

use crate::fuzzy::{FuzzyMatch, fuzzy_match};
use crate::{Service, ServiceError};

/// Registration record for one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContribution {
    pub id: CommandId,
    /// Palette title, e.g. "Run Query".
    pub title: String,
    /// Palette grouping, e.g. "Query", "Edit", "View".
    pub category: String,
    /// GPUI key context the binding applies in (`None` = global).
    pub context: Option<String>,
    /// Default keystrokes in GPUI syntax ("ctrl-enter", "cmd-shift-p").
    pub default_keystrokes: Vec<String>,
    /// Keep out of palette search results (e.g. the palette's own
    /// navigation commands, or "toggle palette" itself). Still bindable.
    pub hidden: bool,
}

/// A command as the palette sees it: contribution + resolved keystrokes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMeta {
    pub id: CommandId,
    pub title: String,
    pub category: String,
    pub context: Option<String>,
    /// Effective keystrokes after user/workspace overrides.
    pub keystrokes: Vec<String>,
    pub hidden: bool,
}

/// A palette search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMatch {
    pub meta: CommandMeta,
    pub score: i32,
    /// Byte offsets into `meta.title` of the matched characters.
    pub indices: Vec<usize>,
}

pub use tempr_domain::KeybindingOverrides;

pub struct CommandService {
    event_bus: Arc<EventBus>,
    commands: RwLock<BTreeMap<CommandId, CommandContribution>>,
    /// Layered lowest → highest; later layers win per command.
    override_layers: RwLock<Vec<KeybindingOverrides>>,
}

impl CommandService {
    pub fn new(event_bus: Arc<EventBus>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            commands: RwLock::new(BTreeMap::new()),
            override_layers: RwLock::new(Vec::new()),
        })
    }

    /// Register (or replace) a command.
    pub fn register(&self, contribution: CommandContribution) {
        self.commands
            .write()
            .insert(contribution.id.clone(), contribution);
    }

    pub fn unregister(&self, id: &CommandId) {
        self.commands.write().remove(id);
    }

    /// Replace the override layers (lowest precedence first): typically
    /// `[user settings, workspace settings]`.
    pub fn set_keybinding_layers(&self, layers: Vec<KeybindingOverrides>) {
        *self.override_layers.write() = layers;
    }

    /// Effective keystrokes for `id`: the highest layer that mentions the
    /// command wins, otherwise the contribution's defaults.
    pub fn keystrokes_for(&self, id: &CommandId) -> Vec<String> {
        let layers = self.override_layers.read();
        if let Some(keys) = layers.iter().rev().find_map(|l| l.get(id.as_str())) {
            return keys.clone();
        }
        self.commands
            .read()
            .get(id)
            .map(|c| c.default_keystrokes.clone())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &CommandId) -> Option<CommandMeta> {
        let c = self.commands.read().get(id)?.clone();
        Some(self.meta(c))
    }

    /// Every command, sorted by category then title.
    pub fn commands(&self) -> Vec<CommandMeta> {
        // Snapshot under the lock, then resolve keys without holding it
        // (`meta` re-reads the map; parking_lot read locks are not reentrant
        // once a writer is queued).
        let snapshot: Vec<CommandContribution> = self.commands.read().values().cloned().collect();
        let mut all: Vec<CommandMeta> = snapshot.into_iter().map(|c| self.meta(c)).collect();
        all.sort_by(|a, b| a.category.cmp(&b.category).then(a.title.cmp(&b.title)));
        all
    }

    /// Fuzzy search over titles (and, as a fallback, ids), best first. An
    /// empty query returns everything in catalog order.
    pub fn search(&self, query: &str) -> Vec<CommandMatch> {
        let query = query.trim();
        let mut hits: Vec<CommandMatch> = self
            .commands()
            .into_iter()
            .filter(|meta| !meta.hidden)
            .filter_map(|meta| {
                let m = fuzzy_match(query, &meta.title).or_else(|| {
                    fuzzy_match(query, meta.id.as_str()).map(|FuzzyMatch { score, .. }| {
                        FuzzyMatch {
                            score: score - 20,
                            indices: Vec::new(),
                        }
                    })
                })?;
                Some(CommandMatch {
                    meta,
                    score: m.score,
                    indices: m.indices,
                })
            })
            .collect();
        if !query.is_empty() {
            hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.meta.title.cmp(&b.meta.title)));
        }
        hits
    }

    /// The UI calls this after dispatching the command's action.
    pub fn record_executed(&self, id: CommandId) {
        self.event_bus.publish(AppEvent::CommandExecuted { id });
    }

    fn meta(&self, c: CommandContribution) -> CommandMeta {
        let keystrokes = self.keystrokes_for(&c.id);
        CommandMeta {
            id: c.id,
            title: c.title,
            category: c.category,
            context: c.context,
            keystrokes,
            hidden: c.hidden,
        }
    }
}

#[async_trait::async_trait]
impl Service for CommandService {
    fn name(&self) -> &'static str {
        "CommandService"
    }

    async fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use tempr_events::EventFilter;

    fn contrib(id: &str, title: &str, cat: &str, keys: &[&str]) -> CommandContribution {
        CommandContribution {
            id: CommandId::from(id),
            title: title.into(),
            category: cat.into(),
            context: Some("MainWindow".into()),
            default_keystrokes: keys.iter().map(|k| k.to_string()).collect(),
            hidden: false,
        }
    }

    fn service() -> Arc<CommandService> {
        let svc = CommandService::new(Arc::new(EventBus::new()));
        svc.register(contrib(
            "main_window::RunQuery",
            "Run Query",
            "Query",
            &["ctrl-enter"],
        ));
        svc.register(contrib(
            "main_window::CancelQuery",
            "Cancel Query",
            "Query",
            &["escape"],
        ));
        svc.register(contrib(
            "main_window::Quit",
            "Quit",
            "Application",
            &["ctrl-q"],
        ));
        svc
    }

    #[test]
    fn commands_are_sorted_by_category_then_title() {
        let titles: Vec<String> = service().commands().into_iter().map(|c| c.title).collect();
        assert_eq!(titles, vec!["Quit", "Cancel Query", "Run Query"]);
    }

    #[test]
    fn overrides_win_by_layer_and_empty_unbinds() {
        let svc = service();
        let run = CommandId::from("main_window::RunQuery");
        assert_eq!(svc.keystrokes_for(&run), vec!["ctrl-enter"]);
        let mut user = KeybindingOverrides::new();
        user.insert("main_window::RunQuery".into(), vec!["f5".into()]);
        let mut ws = KeybindingOverrides::new();
        ws.insert("main_window::Quit".into(), vec![]);
        svc.set_keybinding_layers(vec![user.clone(), ws.clone()]);
        assert_eq!(svc.keystrokes_for(&run), vec!["f5"]);
        assert!(
            svc.keystrokes_for(&CommandId::from("main_window::Quit"))
                .is_empty()
        );
        assert_eq!(
            svc.keystrokes_for(&CommandId::from("main_window::CancelQuery")),
            vec!["escape"],
            "untouched command keeps defaults"
        );
        // Workspace layer beats user layer for the same command.
        ws.insert("main_window::RunQuery".into(), vec!["f9".into()]);
        svc.set_keybinding_layers(vec![user, ws]);
        assert_eq!(svc.keystrokes_for(&run), vec!["f9"]);
        assert_eq!(svc.get(&run).unwrap().keystrokes, vec!["f9"]);
    }

    #[test]
    fn search_is_fuzzy_ranked_and_falls_back_to_ids() {
        let svc = service();
        let hits = svc.search("rq");
        assert_eq!(hits[0].meta.title, "Run Query");
        assert_eq!(hits[0].indices, vec![0, 4]);
        assert!(svc.search("zzz").is_empty());
        assert_eq!(svc.search("").len(), 3, "empty query lists everything");
        let mut hidden = contrib("palette::Dismiss", "Palette: Close", "Palette", &["escape"]);
        hidden.hidden = true;
        svc.register(hidden);
        assert_eq!(
            svc.search("").len(),
            3,
            "hidden commands are not searchable"
        );
        assert_eq!(svc.commands().len(), 4, "but still listed in the catalog");
        // id fallback: "main_window" is not in any title
        assert_eq!(svc.search("main_window").len(), 3);
    }

    #[test]
    fn record_executed_publishes_event() {
        let bus = Arc::new(EventBus::new());
        let seen: Arc<Mutex<Vec<CommandId>>> = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let _sub = bus.subscribe(EventFilter::All, move |ev| {
            if let AppEvent::CommandExecuted { id } = ev {
                s.lock().push(id.clone());
            }
        });
        let svc = CommandService::new(bus);
        svc.record_executed(CommandId::from("main_window::RunQuery"));
        assert_eq!(*seen.lock(), vec![CommandId::from("main_window::RunQuery")]);
        assert_eq!(svc.name(), "CommandService");
    }
}
