//! Command palette: modal overlay of `Input` + a virtualized list of fuzzy
//! matches over `CommandService` (docs/11-gpui.md → Palette). Selection is
//! keyboard-driven; the palette *emits* the chosen command — `MainWindow`
//! dispatches it — so the component holds no business logic.

use std::sync::Arc;

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Subscription,
    Window, actions, div, prelude::*, px, rgb, uniform_list,
};
use tempr_domain::CommandId;
use tempr_services::{CommandMatch, CommandService};

use crate::components::{Input, InputEvent};
use crate::theme;

actions!(palette, [SelectNext, SelectPrev, Dismiss]);

pub const KEY_CONTEXT: &str = "Palette";
/// Shared identifier every overlay adds to its key context so window-level
/// bindings can be suspended with `!Modal` without naming each overlay.
pub const MODAL_CONTEXT: &str = "Modal";
const ROW_HEIGHT: f32 = 28.0;
const MAX_VISIBLE_ROWS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteEvent {
    /// The user confirmed a command.
    Execute(CommandId),
    /// The palette closed without a choice.
    Dismissed,
}

pub struct Palette {
    service: Arc<CommandService>,
    input: Entity<Input>,
    matches: Vec<CommandMatch>,
    selected: usize,
    open: bool,
    /// Focus to restore when the palette closes (whatever was focused when
    /// it opened), so the confirmed command dispatches against it.
    previous_focus: Option<FocusHandle>,
    focus_handle: FocusHandle,
    _input_subscription: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(service: Arc<CommandService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| Input::new("Type a command…", cx));
        let _input_subscription = cx.subscribe_in(
            &input,
            window,
            |this, _input, event, window, cx| match event {
                InputEvent::Changed => this.refresh(cx),
                InputEvent::Submit(_) => this.confirm(window, cx),
            },
        );
        Self {
            service,
            input,
            matches: Vec::new(),
            selected: 0,
            open: false,
            previous_focus: None,
            focus_handle: cx.focus_handle(),
            _input_subscription,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open with an empty query and focus the input; remembers the current
    /// focus for `close`.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.previous_focus = window.focused(cx);
        self.input.update(cx, |input, cx| input.set_text("", cx));
        self.refresh(cx);
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Close and restore the focus captured by `open`.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(prev) = self.previous_focus.take() {
            window.focus(&prev, cx);
        }
        cx.notify();
    }

    /// Current matches (for tests and status UIs).
    pub fn matches(&self) -> &[CommandMatch] {
        &self.matches
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text().to_string();
        self.matches = self.service.search(&query);
        self.selected = 0;
        cx.notify();
    }

    /// Close (restoring focus first, so the owner's dispatch lands on the
    /// element that was focused before the palette opened), then emit.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.matches.get(self.selected) {
            let id = hit.meta.id.clone();
            self.close(window, cx);
            cx.emit(PaletteEvent::Execute(id));
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1) % self.matches.len();
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + self.matches.len() - 1) % self.matches.len();
            cx.notify();
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        cx.emit(PaletteEvent::Dismissed);
    }

    fn render_row(&self, ix: usize) -> gpui::AnyElement {
        let hit = &self.matches[ix];
        let selected = ix == self.selected;
        let keys = hit.meta.keystrokes.join("  ");
        div()
            .id(ix)
            .flex()
            .items_center()
            .justify_between()
            .h(px(ROW_HEIGHT))
            .px_3()
            .bg(rgb(if selected {
                theme::BORDER
            } else {
                theme::SURFACE_RAISED
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(theme::TEXT))
                            .child(hit.meta.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(rgb(theme::TEXT_DIM))
                            .child(hit.meta.category.clone()),
                    ),
            )
            .child(div().text_color(rgb(theme::ACCENT)).child(keys))
            .into_any_element()
    }
}

impl Focusable for Palette {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let rows = self.matches.len().min(MAX_VISIBLE_ROWS);
        div()
            .key_context({
                let mut kc = gpui::KeyContext::new_with_defaults();
                kc.add(KEY_CONTEXT);
                kc.add(MODAL_CONTEXT);
                kc
            })
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::dismiss))
            .absolute()
            .top(px(60.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(620.))
                    .flex()
                    .flex_col()
                    .bg(rgb(theme::SURFACE_RAISED))
                    .border_1()
                    .border_color(rgb(theme::ACCENT))
                    .rounded_md()
                    .shadow_lg()
                    .child(div().p_2().child(self.input.clone()))
                    .child(
                        div().h(px(ROW_HEIGHT * rows as f32)).child(
                            uniform_list(
                                "palette-rows",
                                self.matches.len(),
                                cx.processor(|this, range: std::ops::Range<usize>, _w, _cx| {
                                    range.map(|ix| this.render_row(ix)).collect::<Vec<_>>()
                                }),
                            )
                            .h_full(),
                        ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(theme::TEXT_DIM))
                            .child(if self.matches.is_empty() {
                                "No matching commands".to_string()
                            } else {
                                format!(
                                    "{} of {} · ↑↓ select · enter run · esc close",
                                    self.selected + 1,
                                    self.matches.len()
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}
