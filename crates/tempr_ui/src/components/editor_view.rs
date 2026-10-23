//! Multi-line SQL editor view over `tempr_editor::Buffer` (docs/10-editor.md
//! → EditorView). Renders only the visible lines (`uniform_list`), paints
//! syntax highlights, selections and cursors, and turns keyboard actions
//! into `Buffer` motions / edit ops. It holds no database or business logic:
//! "run" emits the statement text and the owner executes it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, PaintQuad, Pixels,
    Point, Point as GpuiPoint, Render, ScrollStrategy, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, UnderlineStyle, UniformListScrollHandle, Window, actions, anchored, div, fill,
    point, prelude::*, px, relative, rgb, size, uniform_list,
};
use tempr_domain::SqlFileId;
use tempr_editor::edit_ops::LineDirection;
use tempr_editor::{Buffer, Highlight, HighlightKind, Selection, StatementKind};
use tempr_intel::{Analysis, HoverInfo, Severity};
use tempr_services::IntelligenceService;

use crate::components::completion_popup::{self, CompletionPopup};
use crate::gpui_compat;
use crate::theme;

actions!(
    editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocumentStart,
        DocumentEnd,
        SelectAll,
        Backspace,
        Delete,
        Newline,
        Tab,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        DeleteLine,
        DuplicateLine,
        MoveLineUp,
        MoveLineDown,
        RunAll,
        TriggerCompletion,
        Hover,
        NextDiagnostic,
        PrevDiagnostic,
    ]
);

pub const KEY_CONTEXT: &str = "Editor";
pub const LINE_HEIGHT: f32 = 22.0;
pub const FONT_SIZE: f32 = 14.0;
const GUTTER_WIDTH: f32 = 56.0;
const TAB: &str = "  ";
const POPUP_ROWS: usize = 12;
const POPUP_ROW_HEIGHT: f32 = 20.0;
const POPUP_WIDTH: f32 = 420.0;
const ANALYSIS_DEBOUNCE: Duration = Duration::from_millis(100);

struct HoverCard {
    text: String,
    anchor: Option<Point<Pixels>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEvent {
    /// The text changed (owner publishes `BufferChanged`).
    Changed,
    /// Run this SQL (statement under the primary cursor, or everything).
    Run(String),
    /// Something the user should see in the status bar.
    Notice(String),
}

struct LineLayout {
    shaped: ShapedLine,
    bounds: Bounds<Pixels>,
    /// Byte offset of the line start in the buffer.
    start: usize,
}

pub struct EditorView {
    buffer: Buffer,
    intelligence: Arc<IntelligenceService>,
    analysis: Option<(u64, Arc<Analysis>)>,
    selections: Vec<Selection>,
    goal_column: Option<usize>,
    marked_range: Option<Range<usize>>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    /// Layouts of the lines painted last frame, for mouse/IME geometry.
    /// Layouts of the lines painted in the *current* frame (rebuilt by the
    /// `uniform_list` processor, filled by `LineElement::paint`).
    line_layouts: HashMap<usize, LineLayout>,
    /// Line range the list rendered last frame; drives scroll direction
    /// and the IME-handler fallback line.
    visible_lines: Range<usize>,
    /// Highlights for the lines rendered this frame.
    visible_highlights: Vec<Highlight>,
    completion: Option<CompletionPopup>,
    completion_anchor: Option<Point<Pixels>>,
    completion_scroll: UniformListScrollHandle,
    hover: Option<HoverCard>,
    /// Offset under the pointer as of the last mouse move, so a stationary
    /// pointer does not rebuild the card every frame.
    pointer_hover_offset: Option<usize>,
    /// The open card was opened by the pointer (not the keyboard), so a move
    /// onto empty text may dismiss it without closing a keyboard hover.
    hover_from_pointer: bool,
}

impl EventEmitter<EditorEvent> for EditorView {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticsSummary {
    pub errors: usize,
    pub warnings: usize,
}

impl DiagnosticsSummary {
    /// "2 errors · 1 warning" — the status-bar form.
    pub fn text(&self) -> String {
        format!(
            "{} {} · {} {}",
            self.errors,
            plural(self.errors, "error", "errors"),
            self.warnings,
            plural(self.warnings, "warning", "warnings"),
        )
    }
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

impl EditorView {
    pub fn new(text: &str, intelligence: Arc<IntelligenceService>, cx: &mut Context<Self>) -> Self {
        Self {
            buffer: Buffer::new(SqlFileId::new(), text),
            intelligence,
            analysis: None,
            selections: vec![Selection::cursor(0)],
            goal_column: None,
            marked_range: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            line_layouts: HashMap::new(),
            visible_lines: 0..0,
            visible_highlights: Vec::new(),
            completion: None,
            completion_anchor: None,
            completion_scroll: UniformListScrollHandle::new(),
            hover: None,
            pointer_hover_offset: None,
            hover_from_pointer: false,
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text().to_string()
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn file_id(&self) -> SqlFileId {
        self.buffer.file_id()
    }

    pub fn revision(&self) -> u64 {
        self.buffer.revision()
    }

    pub fn intelligence(&self) -> &Arc<IntelligenceService> {
        &self.intelligence
    }

    /// The current analysis, but only while it describes the text on screen.
    pub fn analysis(&self) -> Option<&Arc<Analysis>> {
        match &self.analysis {
            Some((revision, analysis)) if *revision == self.buffer.revision() => Some(analysis),
            _ => None,
        }
    }

    /// Window-space caret box from the last painted line layouts. `None`
    /// before the first frame or when the caret's line was not laid out.
    pub fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        let head = self.primary().head;
        let line = self.buffer.point_for_offset(head).line;
        let layout = self.line_layouts.get(&line)?;
        let index = head.checked_sub(layout.start)?;
        let x = layout.shaped.x_for_index(index);
        Some(Bounds::new(
            point(layout.bounds.left() + x, layout.bounds.top()),
            size(px(2.), layout.bounds.size.height),
        ))
    }

    pub fn diagnostics_summary(&self) -> DiagnosticsSummary {
        let mut summary = DiagnosticsSummary::default();
        if let Some(analysis) = self.analysis() {
            for diagnostic in &analysis.diagnostics {
                match diagnostic.severity {
                    Severity::Error => summary.errors += 1,
                    Severity::Warning => summary.warnings += 1,
                }
            }
        }
        summary
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// Replace the whole content; cursor at the end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let len = self.buffer.len();
        if let Ok(Some(_)) = self.buffer.edit(&[(0..len, text)]) {
            self.close_completion();
            self.hover = None;
            cx.emit(EditorEvent::Changed);
            self.schedule_analysis(cx);
        }
        self.set_selections(vec![Selection::cursor(self.buffer.len())], cx);
    }

    /// Emit the statement under the primary cursor (or a `Notice` when there
    /// is none / it is a parse-error fragment).
    pub fn run_statement_under_cursor(&mut self, cx: &mut Context<Self>) {
        self.close_completion();
        let head = self.primary().head;
        // A non-empty selection runs as-is.
        let primary = self.primary();
        if !primary.is_empty() {
            if let Ok(sql) = self.buffer.slice(primary.range()) {
                cx.emit(EditorEvent::Run(sql));
            }
            return;
        }
        match self.buffer.statement_at(head) {
            Some(range) if range.kind == StatementKind::Error => cx.emit(EditorEvent::Notice(
                "Statement under cursor has a syntax error; not sent.".into(),
            )),
            Some(range) => {
                if let Ok(sql) = self.buffer.slice(range.start..range.end) {
                    cx.emit(EditorEvent::Run(sql));
                }
            }
            None => cx.emit(EditorEvent::Notice("No statement under cursor.".into())),
        }
    }

    fn primary(&self) -> Selection {
        *self.selections.last().unwrap_or(&Selection::cursor(0))
    }

    fn set_selections(&mut self, sels: Vec<Selection>, cx: &mut Context<Self>) {
        self.selections = if sels.is_empty() {
            vec![Selection::cursor(0)]
        } else {
            tempr_editor::selection::normalize(&sels)
        };
        self.hover = None;
        let line = self.buffer.point_for_offset(self.primary().head).line;
        // `visible_lines` is last frame's range: good enough to pick a side.
        if line < self.visible_lines.start {
            self.scroll_handle.scroll_to_item(line, ScrollStrategy::Top);
        } else if line >= self.visible_lines.end {
            self.scroll_handle
                .scroll_to_item(line, ScrollStrategy::Bottom);
        }
        cx.notify();
    }

    /// Undo/redo landed: announce the change and restore the recorded
    /// selections (or a cursor at 0 for edits recorded without any).
    fn restore(&mut self, sels: Option<Vec<Selection>>, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Changed);
        self.close_completion();
        self.goal_column = None;
        self.schedule_analysis(cx);
        self.set_selections(sels.unwrap_or_else(|| vec![Selection::cursor(0)]), cx);
    }

    /// Apply a motion to every selection head. `collapse_to_edge`: a plain
    /// left/right on a non-empty selection lands on its edge instead of
    /// moving from the head (grapheme motions only; word/line/document
    /// motions always move from the head, as in Zed).
    fn move_each(
        &mut self,
        extend: bool,
        collapse_to_edge: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&Buffer, usize) -> usize,
    ) {
        self.close_completion();
        self.goal_column = None;
        let sels: Vec<Selection> = self
            .selections
            .iter()
            .map(|s| {
                let target = if collapse_to_edge && !extend && !s.is_empty() {
                    let edge = f(&self.buffer, s.head);
                    if edge < s.head { s.start() } else { s.end() }
                } else {
                    f(&self.buffer, s.head)
                };
                s.with_head(target, extend)
            })
            .collect();
        self.set_selections(sels, cx);
    }

    fn move_vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        self.close_completion();
        let goal = self.goal_column;
        let mut new_goal = None;
        let sels: Vec<Selection> = self
            .selections
            .iter()
            .map(|s| {
                let (target, g) = self.buffer.move_vertically(s.head, delta, goal);
                new_goal = Some(g);
                s.with_head(target, extend)
            })
            .collect();
        self.set_selections(sels, cx);
        self.goal_column = new_goal;
    }

    fn apply(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(
            &mut Buffer,
            &[Selection],
        ) -> Result<tempr_editor::EditOutcome, tempr_editor::EditError>,
    ) {
        self.apply_with(cx, true, op);
    }

    fn apply_structural(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(
            &mut Buffer,
            &[Selection],
        ) -> Result<tempr_editor::EditOutcome, tempr_editor::EditError>,
    ) {
        self.apply_with(cx, false, op);
    }

    fn apply_with(
        &mut self,
        cx: &mut Context<Self>,
        auto_complete: bool,
        op: impl FnOnce(
            &mut Buffer,
            &[Selection],
        ) -> Result<tempr_editor::EditOutcome, tempr_editor::EditError>,
    ) {
        let sels = self.selections.clone();
        match op(&mut self.buffer, &sels) {
            Ok(outcome) => {
                let changed = outcome.edit.is_some();
                self.goal_column = None;
                self.set_selections(outcome.selections, cx);
                if changed {
                    cx.emit(EditorEvent::Changed);
                    self.after_edit(auto_complete, cx);
                }
            }
            Err(e) => cx.emit(EditorEvent::Notice(format!("edit rejected: {e}"))),
        }
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.apply(cx, |b, s| b.insert_at(s, &text));
    }

    fn schedule_analysis(&self, cx: &mut Context<Self>) {
        let revision = self.buffer.revision();
        let file = self.buffer.file_id();
        let intelligence = self.intelligence.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ANALYSIS_DEBOUNCE).await;
            let _ = this.update(cx, |view, cx| {
                if view.buffer.revision() != revision {
                    return;
                }
                let text = view.buffer.text().to_string();
                let service = intelligence.clone();
                gpui_compat::spawn_tokio(cx, async move {
                    service.analyze(file, revision, &text);
                })
                .detach();
            });
        })
        .detach();
    }

    /// Store the analysis for the revision on screen after
    /// `UiEvent::DiagnosticsReady`; returns whether it was current.
    pub fn on_diagnostics(&mut self, cx: &mut Context<Self>) -> bool {
        let revision = self.buffer.revision();
        if let Some(analysis) = self.intelligence.analysis(self.buffer.file_id(), revision) {
            self.analysis = Some((revision, analysis));
            cx.notify();
            true
        } else {
            false
        }
    }

    /// The catalog changed: analyses built against the previous one are
    /// invalid, so drop the rendered copy and schedule a fresh one.
    pub fn on_schema_refreshed(&mut self, cx: &mut Context<Self>) {
        self.analysis = None;
        self.hover = None;
        self.schedule_analysis(cx);
        cx.notify();
    }

    fn after_edit(&mut self, auto_complete: bool, cx: &mut Context<Self>) {
        self.hover = None;
        if !auto_complete || !self.should_auto_complete() {
            self.close_completion();
        } else if self.completion.is_some() {
            self.refilter_completion();
        } else {
            self.open_completion(false, cx);
        }
        self.schedule_analysis(cx);
        cx.notify();
    }

    fn should_auto_complete(&self) -> bool {
        let head = self.primary().head;
        if head == 0 {
            return false;
        }
        let start = self.buffer.prev_grapheme_boundary(head);
        self.buffer
            .slice(start..head)
            .ok()
            .and_then(|text| text.chars().next_back())
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"' || c == '.')
    }

    fn open_completion(&mut self, explicit: bool, cx: &mut Context<Self>) {
        if !explicit && !self.should_auto_complete() {
            self.close_completion();
            return;
        }
        let head = self.primary().head;
        let items = self.intelligence.complete(&mut self.buffer, head, explicit);
        if items.is_empty() {
            self.close_completion();
        } else {
            self.completion = Some(CompletionPopup::new(items));
            self.completion_anchor = self.caret_anchor();
            self.hover = None;
        }
        self.completion_scroll = UniformListScrollHandle::new();
        cx.notify();
    }

    fn refilter_completion(&mut self) {
        let head = self.primary().head;
        let items = self.intelligence.complete(&mut self.buffer, head, false);
        if items.is_empty() {
            self.close_completion();
            return;
        }
        if let Some(popup) = &mut self.completion {
            popup.refilter(items);
            self.completion_anchor = self.caret_anchor();
        }
    }

    fn caret_anchor(&self) -> Option<Point<Pixels>> {
        self.caret_bounds()
            .map(|bounds| point(bounds.left(), bounds.bottom() + px(4.)))
    }

    /// The popup is open only while it has rows *and* an anchor. The key
    /// context, render path and accept guard share this predicate, so a
    /// popup the user cannot see can never capture enter/tab/escape.
    fn completion_open(&self) -> bool {
        self.completion
            .as_ref()
            .is_some_and(|popup| !popup.is_empty())
            && self.completion_anchor.is_some()
    }

    fn close_completion(&mut self) {
        self.completion = None;
        self.completion_anchor = None;
    }

    fn show_hover(&mut self, cx: &mut Context<Self>) {
        let head = self.primary().head;
        let hover = self.intelligence.hover(&mut self.buffer, head);
        let text = match hover {
            Some(HoverInfo { contents, .. }) => contents,
            None => match self.diagnostic_at(head) {
                Some(message) => message,
                None => {
                    self.hover = None;
                    cx.notify();
                    cx.emit(EditorEvent::Notice(
                        "No symbol or diagnostic at the cursor.".into(),
                    ));
                    return;
                }
            },
        };
        self.hover = Some(HoverCard {
            text,
            anchor: self.caret_anchor(),
        });
        self.hover_from_pointer = false;
        cx.notify();
    }

    fn diagnostic_at(&self, offset: usize) -> Option<String> {
        self.analysis()?
            .diagnostics
            .iter()
            .find(|d| d.range.start <= offset && offset <= d.range.end)
            .map(|d| d.message.clone())
    }

    fn move_to_diagnostic(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .analysis()
            .and_then(|analysis| diagnostic_target(analysis, self.primary().head, forward))
        else {
            cx.emit(EditorEvent::Notice("No diagnostics.".into()));
            return;
        };
        self.close_completion();
        self.set_selections(vec![Selection::cursor(target.0)], cx);
        // The new line has not been laid out yet (it may have just been
        // scrolled into view), so anchor the card after the frame paints.
        let text = target.1;
        let revision = self.buffer.revision();
        cx.on_next_frame(window, move |this, _window, cx| {
            // The user typed before the frame landed: the message describes
            // text that is no longer on screen.
            if this.buffer.revision() != revision {
                return;
            }
            this.hover = Some(HoverCard {
                text,
                anchor: this.caret_anchor(),
            });
            cx.notify();
        });
    }

    // ── actions ──────────────────────────────────────────────────────────

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(false, true, cx, |b, o| b.prev_grapheme_boundary(o));
    }
    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(false, true, cx, |b, o| b.next_grapheme_boundary(o));
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(true, true, cx, |b, o| b.prev_grapheme_boundary(o));
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(true, true, cx, |b, o| b.next_grapheme_boundary(o));
    }
    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, false, cx);
    }
    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, false, cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, cx);
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(false, false, cx, |b, o| b.prev_word_boundary(o));
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(false, false, cx, |b, o| b.next_word_boundary(o));
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(true, false, cx, |b, o| b.prev_word_boundary(o));
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(true, false, cx, |b, o| b.next_word_boundary(o));
    }
    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(false, false, cx, |b, o| b.line_start(o));
    }
    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(false, false, cx, |b, o| b.line_end(o));
    }
    fn select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(true, false, cx, |b, o| b.line_start(o));
    }
    fn select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_each(true, false, cx, |b, o| b.line_end(o));
    }
    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        self.set_selections(vec![Selection::cursor(0)], cx);
    }
    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        let end = self.buffer.len();
        self.set_selections(vec![Selection::cursor(end)], cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        let end = self.buffer.len();
        self.set_selections(vec![Selection::new(0, end)], cx);
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx, |b, s| b.backspace(s));
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx, |b, s| b.delete_forward(s));
    }
    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text("\n", cx);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text(TAB, cx);
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, sels)) = self.buffer.undo_with_selections() {
            self.restore(sels, cx);
        }
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, sels)) = self.buffer.redo_with_selections() {
            self.restore(sels, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(text) = self.buffer.selected_text(&self.selections)
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        if self.selections.iter().any(|s| !s.is_empty()) {
            self.apply_structural(cx, |b, s| b.insert_at(s, ""));
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert_text(&text, cx);
        }
    }
    fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_structural(cx, |b, s| b.delete_lines(s));
    }
    fn duplicate_line(&mut self, _: &DuplicateLine, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_structural(cx, |b, s| b.duplicate_lines(s));
    }
    fn move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_structural(cx, |b, s| b.move_lines(s, LineDirection::Up));
    }
    fn move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.apply_structural(cx, |b, s| b.move_lines(s, LineDirection::Down));
    }
    fn run_all(&mut self, _: &RunAll, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        let all = self.text();
        if !all.trim().is_empty() {
            cx.emit(EditorEvent::Run(all));
        }
    }

    fn trigger_completion(
        &mut self,
        _: &TriggerCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_completion(true, cx);
    }

    fn completion_next(
        &mut self,
        _: &completion_popup::Next,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(popup) = &mut self.completion {
            popup.move_selection(1);
            self.completion_scroll
                .scroll_to_item(popup.selected, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn completion_prev(
        &mut self,
        _: &completion_popup::Prev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(popup) = &mut self.completion {
            popup.move_selection(-1);
            self.completion_scroll
                .scroll_to_item(popup.selected, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn completion_accept(
        &mut self,
        _: &completion_popup::Accept,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.completion_open() {
            return;
        }
        let head = self.primary().head;
        let Some(item) = self
            .completion
            .as_ref()
            .and_then(|popup| popup.selected_item().cloned())
        else {
            return;
        };
        if item.replace.end != head {
            // The item was ranked for a caret that has since moved; applying
            // its range now could replace unrelated text.
            self.close_completion();
            cx.notify();
            return;
        }
        let (range, cursor) = completion_popup::accept_edit(&item);
        self.close_completion();
        match self.buffer.edit(&[(range, item.insert.as_str())]) {
            Ok(changed) => {
                self.set_selections(vec![Selection::cursor(cursor)], cx);
                if changed.is_some() {
                    cx.emit(EditorEvent::Changed);
                    // An accept completes one word: do not immediately
                    // reopen on the identifier the insert just produced.
                    self.hover = None;
                    self.close_completion();
                    self.schedule_analysis(cx);
                }
                cx.notify();
            }
            Err(e) => {
                cx.emit(EditorEvent::Notice(format!("completion rejected: {e}")));
                cx.notify();
            }
        }
    }

    fn completion_dismiss(
        &mut self,
        _: &completion_popup::Dismiss,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion.take().is_some() {
            self.completion_anchor = None;
            cx.notify();
        }
    }

    fn hover_action(&mut self, _: &Hover, _: &mut Window, cx: &mut Context<Self>) {
        self.show_hover(cx);
    }

    fn next_diagnostic(&mut self, _: &NextDiagnostic, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to_diagnostic(true, window, cx);
    }

    fn previous_diagnostic(
        &mut self,
        _: &PrevDiagnostic,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to_diagnostic(false, window, cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // A drag (select, scrollbar) is not a rest: do not open a card that
        // would sit under the pointer while the selection grows.
        if event.pressed_button.is_some() {
            self.dismiss_pointer_hover(cx);
            return;
        }
        let offset = self.offset_for_position(event.position);
        if offset == self.pointer_hover_offset {
            return;
        }
        self.pointer_hover_offset = offset;
        let Some(offset) = offset else {
            self.dismiss_pointer_hover(cx);
            return;
        };
        let Some(HoverInfo { contents, .. }) = self.intelligence.hover(&mut self.buffer, offset)
        else {
            self.dismiss_pointer_hover(cx);
            return;
        };
        self.hover = Some(HoverCard {
            text: contents,
            anchor: Some(point(
                event.position.x + px(12.),
                event.position.y + px(18.),
            )),
        });
        self.hover_from_pointer = true;
        cx.notify();
    }

    fn on_mouse_exit(&mut self, _: &MouseExitEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_pointer_hover(cx);
    }

    /// Close only a card the pointer opened: moving off an identifier must not
    /// take away a card the reader asked for by keystroke.
    fn dismiss_pointer_hover(&mut self, cx: &mut Context<Self>) {
        self.pointer_hover_offset = None;
        if self.hover_from_pointer {
            self.hover = None;
            self.hover_from_pointer = false;
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        if let Some(offset) = self.offset_for_position(event.position) {
            let sel = if event.modifiers.shift {
                self.primary().with_head(offset, true)
            } else {
                Selection::cursor(offset)
            };
            self.goal_column = None;
            self.set_selections(vec![sel], cx);
        }
    }

    fn offset_for_position(&self, position: GpuiPoint<Pixels>) -> Option<usize> {
        let layout = self
            .line_layouts
            .values()
            .find(|l| position.y >= l.bounds.top() && position.y < l.bounds.bottom())?;
        let rel = layout
            .shaped
            .closest_index_for_x(position.x - layout.bounds.left());
        Some(layout.start + rel)
    }

    /// Text runs for one line from the cached visible highlights. Captures
    /// are applied in query order onto a per-byte map, so a nested capture
    /// (e.g. a string inside a function call) overrides the enclosing one
    /// instead of being dropped.
    fn runs_for_line(&self, line_start: usize, text: &str, font: gpui::Font) -> Vec<TextRun> {
        let base = TextRun {
            len: 0,
            font,
            color: rgb(theme::TEXT).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if text.is_empty() {
            return Vec::new();
        }
        let line_end = line_start + text.len();
        let mut kinds: Vec<Option<HighlightKind>> = vec![None; text.len()];
        // `visible_highlights` is sorted by start: stop at the first capture
        // beginning after this line.
        let upper = self
            .visible_highlights
            .partition_point(|h| h.range.start < line_end);
        for h in self.visible_highlights[..upper]
            .iter()
            .filter(|h| h.range.end > line_start)
        {
            let a = h.range.start.max(line_start) - line_start;
            let b = h.range.end.min(line_end) - line_start;
            kinds[a..b].fill(Some(h.kind));
        }

        let mut underlines: Vec<Option<(u32, bool)>> = vec![None; text.len()];
        if let Some(analysis) = self.analysis() {
            for diagnostic in analysis.diagnostics_in(line_start..line_end) {
                let a = diagnostic.range.start.max(line_start) - line_start;
                let b = diagnostic.range.end.min(line_end) - line_start;
                let style = match diagnostic.severity {
                    Severity::Error => (theme::ERROR, false),
                    Severity::Warning => (theme::WARNING, true),
                };
                for slot in &mut underlines[a..b] {
                    match slot {
                        Some((_, false)) => {}
                        _ => *slot = Some(style),
                    }
                }
            }
        }

        let mut runs: Vec<TextRun> = Vec::new();
        for (index, kind) in kinds.into_iter().enumerate() {
            let color: gpui::Hsla = rgb(kind.map_or(theme::TEXT, theme::highlight_color)).into();
            let underline = underlines[index].map(|(color, wavy)| UnderlineStyle {
                thickness: px(1.),
                color: Some(rgb(color).into()),
                wavy,
            });
            match runs.last_mut() {
                Some(last) if last.color == color && last.underline == underline => last.len += 1,
                _ => runs.push(TextRun {
                    len: 1,
                    color,
                    underline,
                    ..base.clone()
                }),
            }
        }
        runs
    }

    fn render_completion_popup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (anchor, rows) = {
            let popup = self.completion.as_ref()?;
            (self.completion_anchor?, popup.items.len())
        };
        let height = px(rows.min(POPUP_ROWS) as f32 * POPUP_ROW_HEIGHT);
        Some(
            anchored()
                .position(anchor)
                .child(
                    div()
                        .w(px(POPUP_WIDTH))
                        .h(height)
                        .bg(rgb(theme::SURFACE_INPUT))
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .rounded(px(4.))
                        .overflow_hidden()
                        .child(
                            uniform_list(
                                "completion-items",
                                rows,
                                cx.processor(|this, range: Range<usize>, _window, _cx| {
                                    range
                                        .map(|index| this.render_completion_row(index))
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(&self.completion_scroll)
                            .h_full(),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_completion_row(&self, index: usize) -> AnyElement {
        let Some(popup) = &self.completion else {
            return div().into_any_element();
        };
        let Some(item) = popup.items.get(index) else {
            return div().into_any_element();
        };
        let selected = popup.selected == index;
        div()
            .id(index)
            .flex()
            .items_center()
            .gap_2()
            .h(px(POPUP_ROW_HEIGHT))
            .px_2()
            .bg(rgb(if selected {
                theme::SURFACE_RAISED
            } else {
                theme::SURFACE_INPUT
            }))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(if selected { theme::ACCENT } else { theme::TEXT }))
                    .child(item.label.clone()),
            )
            .child(
                div()
                    .w(px(52.))
                    .text_size(px(11.))
                    .text_color(rgb(theme::TEXT_DIM))
                    .child(CompletionPopup::kind_label(item.kind)),
            )
            .child(
                div()
                    .max_w(px(180.))
                    .text_size(px(11.))
                    .text_color(rgb(theme::TEXT_DIM))
                    .child(item.detail.clone()),
            )
            .into_any_element()
    }

    fn render_hover_card(&self) -> Option<AnyElement> {
        let card = self.hover.as_ref()?;
        let anchor = card.anchor?;
        Some(
            anchored()
                .position(anchor)
                .child(
                    div()
                        .w(px(440.))
                        .p_2()
                        .bg(rgb(theme::SURFACE_RAISED))
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .rounded(px(4.))
                        .text_size(px(12.))
                        .text_color(rgb(theme::TEXT))
                        .flex()
                        .flex_col()
                        .children(
                            card.text
                                .lines()
                                .map(|line| div().child(line.to_string()))
                                .collect::<Vec<_>>(),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_line(&self, line: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let primary_line = self.buffer.point_for_offset(self.primary().head).line;
        let current = line == primary_line;
        div()
            .id(line)
            .flex()
            .h(px(LINE_HEIGHT))
            .w_full()
            .bg(rgb(if current {
                theme::EDITOR_CURRENT_LINE
            } else {
                theme::SURFACE
            }))
            .child(
                div()
                    .w(px(GUTTER_WIDTH))
                    .pr_3()
                    .text_color(rgb(if current {
                        theme::TEXT_DIM
                    } else {
                        theme::EDITOR_GUTTER
                    }))
                    .flex()
                    .justify_end()
                    .child(format!("{}", line + 1)),
            )
            .child(div().flex_1().child(LineElement {
                view: cx.entity(),
                line,
            }))
            .into_any_element()
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.offset_from_utf16(range_utf16.start)
            ..self.buffer.offset_from_utf16(range_utf16.end);
        actual_range.replace(
            self.buffer.offset_to_utf16(range.start)..self.buffer.offset_to_utf16(range.end),
        );
        self.buffer.slice(range).ok()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let p = self.primary();
        Some(UTF16Selection {
            range: self.buffer.offset_to_utf16(p.start())..self.buffer.offset_to_utf16(p.end()),
            reversed: p.head < p.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|r| self.buffer.offset_to_utf16(r.start)..self.buffer.offset_to_utf16(r.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = range_utf16
            .map(|r| self.buffer.offset_from_utf16(r.start)..self.buffer.offset_from_utf16(r.end))
            .or_else(|| self.marked_range.clone());
        self.marked_range = None;
        match target {
            Some(r) => {
                let sel = Selection::new(r.start, r.end);
                let text = new_text.to_string();
                let sels = vec![sel];
                match self.buffer.insert_at(&sels, &text) {
                    Ok(outcome) => {
                        let changed = outcome.edit.is_some();
                        self.set_selections(outcome.selections, cx);
                        if changed {
                            cx.emit(EditorEvent::Changed);
                            self.after_edit(true, cx);
                        }
                    }
                    Err(e) => cx.emit(EditorEvent::Notice(format!("edit rejected: {e}"))),
                }
            }
            None => self.insert_text(new_text, cx),
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Minimal IME: insert the composition and mark it; the marked text is
        // replaced by the next call. The mark is derived from where the
        // primary cursor actually landed, so it stays right with multiple
        // cursors or a rejected edit.
        self.replace_text_in_range(range_utf16, new_text, window, cx);
        let head = self.primary().head;
        self.marked_range =
            (!new_text.is_empty() && head >= new_text.len()).then(|| head - new_text.len()..head);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let start = self.buffer.offset_from_utf16(range_utf16.start);
        let line = self.buffer.point_for_offset(start).line;
        let layout = self.line_layouts.get(&line)?;
        let x = layout.shaped.x_for_index(start - layout.start);
        Some(Bounds::new(
            point(layout.bounds.left() + x, layout.bounds.top()),
            size(px(2.), layout.bounds.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: GpuiPoint<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_position(point)?;
        Some(self.buffer.offset_to_utf16(offset))
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_count = self.buffer.len_lines();
        let popup = self.render_completion_popup(cx);
        let hover = self.render_hover_card();
        div()
            .key_context(if self.completion_open() {
                completion_popup::KEY_CONTEXT
            } else {
                KEY_CONTEXT
            })
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::delete_line))
            .on_action(cx.listener(Self::duplicate_line))
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::run_all))
            .on_action(cx.listener(Self::trigger_completion))
            .on_action(cx.listener(Self::completion_next))
            .on_action(cx.listener(Self::completion_prev))
            .on_action(cx.listener(Self::completion_accept))
            .on_action(cx.listener(Self::completion_dismiss))
            .on_action(cx.listener(Self::hover_action))
            .on_action(cx.listener(Self::next_diagnostic))
            .on_action(cx.listener(Self::previous_diagnostic))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_exit(cx.listener(Self::on_mouse_exit))
            .size_full()
            .bg(rgb(theme::SURFACE))
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .font_family("monospace")
            .child(
                uniform_list(
                    "editor-lines",
                    line_count,
                    cx.processor(|this, range: Range<usize>, _window, cx| {
                        // Per-frame state: layouts are re-filled by paint.
                        this.line_layouts.clear();
                        this.visible_lines = range.clone();
                        if range.is_empty() {
                            this.visible_highlights.clear();
                            return Vec::new();
                        }
                        // Highlights for exactly the lines about to be rendered.
                        let start = this.buffer.line_start_of(range.start);
                        let end = this.buffer.line_end_of(range.end - 1);
                        this.visible_highlights = this.buffer.highlights(start..end);
                        this.visible_highlights.sort_by_key(|h| h.range.start);
                        range
                            .map(|line| this.render_line(line, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll_handle)
                .h_full(),
            )
            .children(popup)
            .children(hover)
    }
}

/// Paints one line: text with highlight runs, selection backgrounds, and
/// cursors; registers the IME handler (see `LinePrepaint::hosts_ime`).
struct LineElement {
    view: Entity<EditorView>,
    line: usize,
}

struct LinePrepaint {
    shaped: ShapedLine,
    selections: Vec<PaintQuad>,
    cursors: Vec<PaintQuad>,
    start: usize,
    /// Registers the IME/input handler: the primary cursor's line, or the
    /// first visible line when that cursor is scrolled out of view (typing
    /// must never be dropped).
    hosts_ime: bool,
}

impl IntoElement for LineElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for LineElement {
    type RequestLayoutState = ();
    type PrepaintState = LinePrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(LINE_HEIGHT).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> LinePrepaint {
        let view = self.view.read(cx);
        let text: SharedString = view.buffer.line(self.line).unwrap_or_default().into();
        let start = view.buffer.line_start_of(self.line);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let runs = view.runs_for_line(start, &text, style.font());
        let shaped = window
            .text_system()
            .shape_line(text.clone(), font_size, &runs, None);
        let line_end = start + text.len();

        let mut selections = Vec::new();
        let mut cursors = Vec::new();
        let primary = view.primary();
        for s in &view.selections {
            // Overlaps this line's text or its newline (a selection covering
            // only the newline still paints, as a strip to the right edge).
            if !s.is_empty() && s.start() <= line_end && s.end() > start && s.start() < s.end() {
                let a = s.start().clamp(start, line_end) - start;
                let b = s.end().clamp(start, line_end) - start;
                let x0 = shaped.x_for_index(a);
                let x1 = if s.end() > line_end {
                    // Selection continues past this line: extend to the edge.
                    bounds.size.width
                } else {
                    shaped.x_for_index(b)
                };
                selections.push(fill(
                    Bounds::from_corners(
                        point(bounds.left() + x0, bounds.top()),
                        point(bounds.left() + x1.max(x0 + px(4.)), bounds.bottom()),
                    ),
                    theme::selection_fill(),
                ));
            }
            if s.head >= start && s.head <= line_end {
                let x = shaped.x_for_index(s.head - start);
                cursors.push(fill(
                    Bounds::new(
                        point(bounds.left() + x, bounds.top()),
                        size(px(2.), bounds.size.height),
                    ),
                    rgb(theme::ACCENT),
                ));
            }
        }
        let primary_line = view.buffer.point_for_offset(primary.head).line;
        let hosts_ime = primary_line == self.line
            || (!view.visible_lines.contains(&primary_line)
                && self.line == view.visible_lines.start);
        LinePrepaint {
            shaped,
            selections,
            cursors,
            start,
            hosts_ime,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut LinePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();
        if prepaint.hosts_ime {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
        }
        for q in prepaint.selections.drain(..) {
            window.paint_quad(q);
        }
        if let Err(e) = prepaint.shaped.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        ) {
            tracing::warn!(error = %e, "EditorView: failed to paint line");
        }
        if focus_handle.is_focused(window) {
            for q in prepaint.cursors.drain(..) {
                window.paint_quad(q);
            }
        }
        let shaped = prepaint.shaped.clone();
        let start = prepaint.start;
        let line = self.line;
        self.view.update(cx, |view, _| {
            view.line_layouts.insert(
                line,
                LineLayout {
                    shaped,
                    bounds,
                    start,
                },
            );
        });
    }
}

fn diagnostic_target(analysis: &Analysis, from: usize, forward: bool) -> Option<(usize, String)> {
    let from = if forward {
        from
    } else {
        analysis
            .diagnostics
            .iter()
            .find(|d| d.range.start < from && from < d.range.end)
            .map_or(from, |d| d.range.start)
    };
    let diagnostic = if forward {
        analysis
            .diagnostics
            .iter()
            .find(|d| d.range.start > from)
            .or_else(|| analysis.diagnostics.first())?
    } else {
        analysis
            .previous_diagnostic(from)
            .or_else(|| analysis.diagnostics.last())?
    };
    Some((diagnostic.range.start, diagnostic.message.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_intel::{Diagnostic, DiagnosticKind};

    fn analysis(diagnostics: Vec<Diagnostic>) -> Analysis {
        Analysis {
            scopes: Vec::new(),
            diagnostics,
        }
    }

    fn error(start: usize, message: &str) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::UnknownColumn,
            start..start + 2,
            message.into(),
        )
    }

    #[test]
    fn diagnostic_navigation_walks_and_wraps() {
        let analysis = analysis(vec![error(2, "first"), error(10, "second")]);
        assert_eq!(
            diagnostic_target(&analysis, 0, true),
            Some((2, "first".into()))
        );
        assert_eq!(
            diagnostic_target(&analysis, 2, true),
            Some((10, "second".into())),
            "the diagnostic at the cursor is skipped"
        );
        assert_eq!(
            diagnostic_target(&analysis, 10, true),
            Some((2, "first".into())),
            "forward wraps to the first"
        );
        assert_eq!(
            diagnostic_target(&analysis, 10, false),
            Some((2, "first".into())),
            "backward skips the one at the cursor"
        );
        assert_eq!(
            diagnostic_target(&analysis, 11, false),
            Some((2, "first".into())),
            "backward skips the diagnostic the cursor sits inside"
        );
        assert_eq!(
            diagnostic_target(&analysis, 2, false),
            Some((10, "second".into())),
            "backward wraps to the last"
        );
        assert_eq!(
            diagnostic_target(&analysis, 0, false),
            Some((10, "second".into()))
        );
    }

    #[test]
    fn no_diagnostics_is_no_target() {
        assert_eq!(diagnostic_target(&analysis(Vec::new()), 0, true), None);
        assert_eq!(diagnostic_target(&analysis(Vec::new()), 0, false), None);
    }

    #[test]
    fn diagnostics_summary_pluralizes() {
        assert_eq!(
            DiagnosticsSummary {
                errors: 2,
                warnings: 1
            }
            .text(),
            "2 errors · 1 warning"
        );
        assert_eq!(
            DiagnosticsSummary {
                errors: 1,
                warnings: 3
            }
            .text(),
            "1 error · 3 warnings"
        );
        assert_eq!(
            DiagnosticsSummary::default().text(),
            "0 errors · 0 warnings"
        );
    }
}

#[cfg(test)]
mod view_tests {
    use chrono::Utc;
    use gpui::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Keystroke, Modifiers,
        Render, TestAppContext, VisualTestContext, Window, div, prelude::*,
    };
    use tempr_domain::{
        ConnectionId, SchemaObject, SchemaObjectId, SchemaSnapshot, SchemaSnapshotId,
    };
    use tempr_events::EventBus;
    use tempr_services::{CommandService, ConnectionService, SchemaService};

    use super::*;
    use crate::commands;

    struct Harness {
        editor: Entity<EditorView>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.editor.clone())
        }
    }

    /// `public.users(id, email, "createdAt")` — `createdAt` needs quoting, so
    /// accepting it proves the candidate's `insert` is used, not its label.
    fn users_snapshot() -> SchemaSnapshot {
        let table = SchemaObjectId::new();
        let column = |name: &str, ordinal: usize| SchemaObject::Column {
            id: SchemaObjectId::new(),
            parent_id: table,
            name: name.into(),
            data_type: "text".into(),
            nullable: true,
            ordinal,
            default: None,
        };
        SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: ConnectionId::new(),
            version: 1,
            fetched_at: Utc::now(),
            objects: vec![
                SchemaObject::Table {
                    id: table,
                    schema: "public".into(),
                    name: "users".into(),
                    estimated_rows: Some(1),
                },
                column("id", 0),
                column("email", 1),
                column("createdAt", 2),
            ],
            keywords: vec!["select".into(), "from".into(), "where".into()],
            fingerprints: Vec::new(),
        }
    }

    fn setup(cx: &mut TestAppContext, text: &str) -> (Entity<EditorView>, AnyWindowHandle) {
        let service = CommandService::new(Arc::new(EventBus::new()));
        cx.update(|cx| commands::install(cx, &service));

        let bus = Arc::new(EventBus::new());
        let connections = ConnectionService::new(bus.clone());
        let schema = SchemaService::new(bus.clone(), connections);
        let intelligence = IntelligenceService::new(bus, schema);
        intelligence.install_snapshot(&users_snapshot());

        let root = cx.add_window(|_, cx| {
            let editor = cx.new(|cx| EditorView::new(text, intelligence, cx));
            Harness { editor }
        });
        let editor = cx.read(|cx| root.read(cx).expect("window root").editor.clone());
        (editor, root.into())
    }

    fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    fn press(cx: &mut TestAppContext, window: AnyWindowHandle, keystroke: &str) {
        let keystroke = Keystroke::parse(keystroke).expect("valid keystroke");
        cx.update_window(window, |_, window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        })
        .unwrap();
    }

    fn labels(editor: &Entity<EditorView>, cx: &TestAppContext) -> Vec<String> {
        let mut labels = editor.read_with(cx, |editor, _| {
            editor
                .completion
                .as_ref()
                .map(|popup| {
                    popup
                        .items
                        .iter()
                        .map(|item| item.label.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        labels.sort_unstable();
        labels
    }

    /// End to end: the catalog snapshot feeds the popup, typing after `u.`
    /// re-filters it, and enter replaces the typed prefix with the *quoted*
    /// insert. Every step is a keystroke, so this also covers the editor and
    /// completion contexts resolving their own bindings.
    #[gpui::test]
    fn completion_accept_inserts_the_quoted_column_after_a_qualifier(cx: &mut TestAppContext) {
        let (editor, window) = setup(cx, "select * from users u where u.");
        let focus = editor.read_with(cx, |editor, cx| editor.focus_handle(cx));
        cx.update_window(window, |_, window, cx| {
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();

        // `ctrl-end` is the editor's own `DocumentEnd` binding.
        press(cx, window, "ctrl-end");
        // `ctrl-space` is `TriggerCompletion`; after a dot it offers the
        // binding's columns only.
        press(cx, window, "ctrl-space");
        draw(cx, window);
        assert_eq!(
            labels(&editor, cx),
            vec!["createdAt", "email", "id"],
            "the popup must offer the `u` binding's columns"
        );

        // Typing narrows the popup to the camel-case column.
        for ch in "cr".chars() {
            press(cx, window, &ch.to_string());
        }
        draw(cx, window);
        assert_eq!(
            labels(&editor, cx),
            vec!["createdAt"],
            "typing must re-filter the popup"
        );

        press(cx, window, "enter");

        assert_eq!(
            editor.read_with(cx, |editor, _| editor.text()),
            "select * from users u where u.\"createdAt\"",
            "accept must insert the quoted identifier, not the label"
        );
        assert!(
            !editor.read_with(cx, |editor, _| editor.completion_open()),
            "accepting must close the popup"
        );
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.primary().head),
            "select * from users u where u.\"createdAt\"".len(),
            "the caret belongs after the inserted identifier"
        );
    }

    /// `ctrl-k ctrl-i` is a chord in the catalog, so this also covers the
    /// pending-chord dispatch path. The card is resolved synchronously from
    /// the catalog; nothing was typed, so no analysis task is scheduled.
    #[gpui::test]
    fn ctrl_k_ctrl_i_shows_the_hover_card_for_the_word_at_the_caret(cx: &mut TestAppContext) {
        let (editor, window) = setup(cx, "select id from users");
        let focus = editor.read_with(cx, |editor, cx| editor.focus_handle(cx));
        cx.update_window(window, |_, window, cx| {
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();

        // Caret after `id` (the engine accepts the offset on the space).
        for _ in 0..9 {
            press(cx, window, "right");
        }
        press(cx, window, "ctrl-k");
        press(cx, window, "ctrl-i");

        let (text, anchored) = editor.read_with(cx, |editor, _| {
            let hover = editor.hover.as_ref().expect("hover card is open");
            (hover.text.clone(), hover.anchor.is_some())
        });
        assert!(
            text.contains("public.users.id"),
            "the card describes the column under the caret: {text}"
        );
        assert!(anchored, "the card is anchored to the painted caret line");
    }

    /// f8 walks to the diagnostic and defers the card to the next frame, so
    /// the test must deliver one with `simulate_next_frame`.
    #[gpui::test]
    fn f8_jumps_to_the_next_diagnostic_and_shows_its_message(cx: &mut TestAppContext) {
        let text = "select zz from users";
        let (editor, window) = setup(cx, text);
        let focus = editor.read_with(cx, |editor, cx| editor.focus_handle(cx));
        cx.update_window(window, |_, window, cx| {
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();

        let (file, revision) =
            editor.read_with(cx, |editor, _| (editor.file_id(), editor.revision()));
        editor
            .read_with(cx, |editor, _| editor.intelligence().clone())
            .analyze(file, revision, text);
        assert!(
            editor.update(cx, |editor, cx| editor.on_diagnostics(cx)),
            "the analysis belongs to the revision on screen"
        );
        let diagnostic = editor.read_with(cx, |editor, _| {
            editor.analysis().expect("analysis").diagnostics[0].clone()
        });
        assert_eq!(
            diagnostic.range.start, 7,
            "setup: `zz` starts after `select `"
        );

        press(cx, window, "f8");
        // `on_next_frame` runs only when the test delivers a frame; there is
        // no platform frame loop under `TestAppContext`.
        cx.update_window(window, |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 1, "one deferred card");
        })
        .unwrap();

        let (head, card) = editor.read_with(cx, |editor, _| {
            (
                editor.primary().head,
                editor.hover.as_ref().expect("card").text.clone(),
            )
        });
        assert_eq!(
            head, diagnostic.range.start,
            "f8 puts the caret on the diagnostic"
        );
        assert_eq!(
            card, diagnostic.message,
            "the card carries the diagnostic message"
        );
    }

    /// The prepaint path builds one `TextRun` per styled byte. Nothing was
    /// drawn, so the syntax pass is empty and the diagnostics pass alone
    /// decides the underline geometry.
    #[gpui::test]
    fn the_error_underline_covers_only_the_unknown_column(cx: &mut TestAppContext) {
        let text = "select zz from users";
        let (editor, _window) = setup(cx, text);
        let (file, revision) =
            editor.read_with(cx, |editor, _| (editor.file_id(), editor.revision()));
        editor
            .read_with(cx, |editor, _| editor.intelligence().clone())
            .analyze(file, revision, text);
        assert!(editor.update(cx, |editor, cx| editor.on_diagnostics(cx)));

        let runs = editor.read_with(cx, |editor, _| {
            editor.runs_for_line(0, text, gpui::Font::default())
        });
        let mut underlined = Vec::new();
        let mut offset = 0;
        for run in runs {
            if let Some(style) = run.underline {
                underlined.push((offset..offset + run.len, style));
            }
            offset += run.len;
        }
        assert_eq!(underlined.len(), 1, "one run carries the underline");
        let (range, style) = &underlined[0];
        assert_eq!(range, &(7..9), "only `zz` is underlined");
        assert_eq!(
            style.color,
            Some(gpui::rgb(crate::theme::ERROR).into()),
            "errors use the error color"
        );
        assert!(!style.wavy, "errors are solid");
    }

    /// Clicking maps the painted line geometry back to a byte offset: the
    /// caret sits in the middle of `id`, the click lands on the caret box
    /// saved for offset 9, and the selection must move there.
    #[gpui::test]
    fn a_click_selects_the_offset_under_the_pointer(cx: &mut TestAppContext) {
        let (editor, window) = setup(cx, "select id from users");
        let focus = editor.read_with(cx, |editor, cx| editor.focus_handle(cx));
        cx.update_window(window, |_, window, cx| {
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();

        for _ in 0..9 {
            press(cx, window, "right");
        }
        let caret = editor.read_with(cx, |editor, _| editor.caret_bounds().expect("caret box"));
        press(cx, window, "ctrl-home");
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.primary().head),
            0,
            "setup: the click must do the moving"
        );

        VisualTestContext::from_window(window, cx).simulate_click(
            gpui::point(caret.left() + gpui::px(1.), caret.center().y),
            Modifiers::default(),
        );

        assert_eq!(
            editor.read_with(cx, |editor, _| editor.primary().head),
            9,
            "the click lands after `id`"
        );
    }

    /// The pointer resting on a reference opens the same card the keyboard
    /// command produces (exit criterion 7's "cursor rests").
    #[gpui::test]
    fn resting_the_pointer_on_a_reference_opens_the_hover_card(cx: &mut TestAppContext) {
        let (editor, window) = setup(cx, "select id from users");
        let focus = editor.read_with(cx, |editor, cx| editor.focus_handle(cx));
        cx.update_window(window, |_, window, cx| {
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();

        // Read the painted box of `id` via the caret, then park the caret away.
        for _ in 0..9 {
            press(cx, window, "right");
        }
        let caret = editor.read_with(cx, |editor, _| editor.caret_bounds().expect("caret box"));
        press(cx, window, "ctrl-home");

        let position = gpui::point(caret.left() + gpui::px(1.), caret.center().y);
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_move(position, None::<MouseButton>, Modifiers::default());

        let (text, anchor) = editor.read_with(cx, |editor, _| {
            let card = editor.hover.as_ref().expect("pointer hover card");
            (card.text.clone(), card.anchor)
        });
        assert!(
            text.contains("public.users.id"),
            "the card describes the column under the pointer: {text}"
        );
        assert_eq!(
            anchor,
            Some(gpui::point(
                position.x + gpui::px(12.),
                position.y + gpui::px(18.)
            )),
            "the card is anchored beside the pointer, not the caret"
        );

        // A drag is not a rest.
        visual.simulate_mouse_move(position, Some(MouseButton::Left), Modifiers::default());
        assert!(
            editor.read_with(cx, |editor, _| editor.hover.is_none()),
            "dragging dismisses the pointer card"
        );
    }
}
