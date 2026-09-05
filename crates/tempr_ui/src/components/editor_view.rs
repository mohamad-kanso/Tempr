//! Multi-line SQL editor view over `tempr_editor::Buffer` (docs/10-editor.md
//! → EditorView). Renders only the visible lines (`uniform_list`), paints
//! syntax highlights, selections and cursors, and turns keyboard actions
//! into `Buffer` motions / edit ops. It holds no database or business logic:
//! "run" emits the statement text and the owner executes it.

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point as GpuiPoint, Render,
    ScrollStrategy, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UniformListScrollHandle, Window, actions, div, fill, point, prelude::*, px, relative, rgb,
    size, uniform_list,
};
use tempr_domain::SqlFileId;
use tempr_editor::edit_ops::LineDirection;
use tempr_editor::{Buffer, Highlight, HighlightKind, Selection, StatementKind};

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
    ]
);

pub const KEY_CONTEXT: &str = "Editor";
pub const LINE_HEIGHT: f32 = 22.0;
pub const FONT_SIZE: f32 = 14.0;
const GUTTER_WIDTH: f32 = 56.0;
const TAB: &str = "  ";

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
}

impl EventEmitter<EditorEvent> for EditorView {}

impl EditorView {
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        Self {
            buffer: Buffer::new(SqlFileId::new(), text),
            selections: vec![Selection::cursor(0)],
            goal_column: None,
            marked_range: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            line_layouts: HashMap::new(),
            visible_lines: 0..0,
            visible_highlights: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text().to_string()
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// Replace the whole content; cursor at the end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let len = self.buffer.len();
        if let Ok(Some(_)) = self.buffer.edit(&[(0..len, text)]) {
            cx.emit(EditorEvent::Changed);
        }
        self.set_selections(vec![Selection::cursor(self.buffer.len())], cx);
    }

    /// Emit the statement under the primary cursor (or a `Notice` when there
    /// is none / it is a parse-error fragment).
    pub fn run_statement_under_cursor(&mut self, cx: &mut Context<Self>) {
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
        self.goal_column = None;
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
        let sels = self.selections.clone();
        match op(&mut self.buffer, &sels) {
            Ok(outcome) => {
                if outcome.edit.is_some() {
                    cx.emit(EditorEvent::Changed);
                }
                self.goal_column = None;
                self.set_selections(outcome.selections, cx);
            }
            Err(e) => cx.emit(EditorEvent::Notice(format!("edit rejected: {e}"))),
        }
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.apply(cx, |b, s| b.insert_at(s, &text));
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
        self.set_selections(vec![Selection::cursor(0)], cx);
    }
    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.buffer.len();
        self.set_selections(vec![Selection::cursor(end)], cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
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
            self.apply(cx, |b, s| b.insert_at(s, ""));
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert_text(&text, cx);
        }
    }
    fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx, |b, s| b.delete_lines(s));
    }
    fn duplicate_line(&mut self, _: &DuplicateLine, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx, |b, s| b.duplicate_lines(s));
    }
    fn move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx, |b, s| b.move_lines(s, LineDirection::Up));
    }
    fn move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx, |b, s| b.move_lines(s, LineDirection::Down));
    }
    fn run_all(&mut self, _: &RunAll, _: &mut Window, cx: &mut Context<Self>) {
        let all = self.text();
        if !all.trim().is_empty() {
            cx.emit(EditorEvent::Run(all));
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        let mut runs: Vec<TextRun> = Vec::new();
        for kind in kinds {
            let color: gpui::Hsla = rgb(kind.map_or(theme::TEXT, theme::highlight_color)).into();
            match runs.last_mut() {
                Some(last) if last.color == color => last.len += 1,
                _ => runs.push(TextRun {
                    len: 1,
                    color,
                    ..base.clone()
                }),
            }
        }
        runs
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
                        if outcome.edit.is_some() {
                            cx.emit(EditorEvent::Changed);
                        }
                        self.set_selections(outcome.selections, cx);
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
        div()
            .key_context(KEY_CONTEXT)
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
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
