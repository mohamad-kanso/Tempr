//! Command palette: modal overlay of `Input` + a virtualized list of fuzzy
//! matches over `CommandService` (docs/11-gpui.md → Palette). Selection is
//! keyboard-driven; the palette *emits* the chosen command — `MainWindow`
//! dispatches it — so the component holds no business logic.

use std::sync::Arc;

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, ScrollStrategy,
    Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px, rgb, uniform_list,
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
    /// Keeps the selected row inside the viewport when the list is longer
    /// than `MAX_VISIBLE_ROWS`.
    scroll_handle: UniformListScrollHandle,
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
            scroll_handle: UniformListScrollHandle::new(),
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
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
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
            self.reveal_selected();
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + self.matches.len() - 1) % self.matches.len();
            self.reveal_selected();
            cx.notify();
        }
    }

    /// Scroll the selected row into view; `Nearest` is a no-op when it is
    /// already visible, so arrow keys inside the viewport do not jump.
    fn reveal_selected(&self) {
        self.scroll_handle
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
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
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
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
                            .track_scroll(&self.scroll_handle)
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        AnyWindowHandle, AppContext as _, Context, Entity, FocusHandle, IntoElement, Keystroke,
        Render, TestAppContext, Window, div, prelude::*, px,
    };
    use tempr_events::EventBus;

    use super::*;
    use crate::commands;

    struct Harness {
        palette: Entity<Palette>,
        /// Focus target that owns focus before the palette opens.
        focus: FocusHandle,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .child(self.palette.clone())
        }
    }

    struct Fixture {
        palette: Entity<Palette>,
        focus: FocusHandle,
        window: AnyWindowHandle,
    }

    fn setup(cx: &mut TestAppContext) -> Fixture {
        let service = CommandService::new(Arc::new(EventBus::new()));
        cx.update(|cx| commands::install(cx, &service));
        let root = cx.add_window(|window, cx| {
            let palette = cx.new(|cx| Palette::new(service, window, cx));
            let focus = cx.focus_handle();
            Harness { palette, focus }
        });
        let (palette, focus) = cx.read(|cx| {
            let root = root.read(cx).expect("window root");
            (root.palette.clone(), root.focus.clone())
        });
        Fixture {
            palette,
            focus,
            window: root.into(),
        }
    }

    impl Fixture {
        /// Focus the element that owned focus before the palette and open it,
        /// drawing the frame that installs the palette's input handler.
        fn open(&self, cx: &mut TestAppContext) {
            cx.update_window(self.window, |_, window, cx| {
                window.focus(&self.focus, cx);
                self.palette
                    .update(cx, |palette, cx| palette.open(window, cx));
            })
            .unwrap();
            self.draw(cx);
        }

        fn draw(&self, cx: &mut TestAppContext) {
            cx.update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }

        fn press(&self, cx: &mut TestAppContext, keystroke: &str) {
            let keystroke = Keystroke::parse(keystroke).expect("valid keystroke");
            cx.update_window(self.window, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .unwrap();
        }

        fn type_str(&self, cx: &mut TestAppContext, text: &str) {
            for ch in text.chars() {
                self.press(cx, &ch.to_string());
            }
        }

        fn selected(&self, cx: &TestAppContext) -> usize {
            self.palette
                .read_with(cx, |palette, _| palette.selected_index())
        }

        fn scroll_offset(&self, cx: &TestAppContext) -> gpui::Point<gpui::Pixels> {
            self.palette.read_with(cx, |palette, _| {
                palette.scroll_handle.0.borrow().base_handle.offset()
            })
        }

        fn focused(&self, cx: &mut TestAppContext) -> Option<FocusHandle> {
            cx.update_window(self.window, |_, window, cx| window.focused(cx))
                .unwrap()
        }
    }

    fn subscribe(
        fixture: &Fixture,
        cx: &mut TestAppContext,
    ) -> (Rc<RefCell<Vec<PaletteEvent>>>, Subscription) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let subscription = cx.update(|cx| {
            cx.subscribe(&fixture.palette, move |_, event: &PaletteEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        (events, subscription)
    }

    #[gpui::test]
    fn typed_query_filters_the_list_and_enter_executes_the_selection(cx: &mut TestAppContext) {
        let fixture = setup(cx);
        let (events, _subscription) = subscribe(&fixture, cx);
        fixture.open(cx);

        assert!(
            fixture
                .palette
                .read_with(cx, |palette, _| palette.is_open()),
            "open() must show the palette"
        );
        assert!(
            !fixture
                .palette
                .read_with(cx, |palette, _| palette.matches().is_empty()),
            "open() must search the catalog"
        );

        fixture.type_str(cx, "quit");
        let hits = fixture.palette.read_with(cx, |palette, _| {
            palette
                .matches()
                .iter()
                .map(|hit| hit.meta.id.to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            hits,
            vec!["main_window::Quit".to_string()],
            "typing must filter the list, not just highlight"
        );

        fixture.press(cx, "enter");

        assert_eq!(
            *events.borrow(),
            vec![PaletteEvent::Execute("main_window::Quit".into())],
            "enter must emit the selected command"
        );
        assert!(
            !fixture
                .palette
                .read_with(cx, |palette, _| palette.is_open()),
            "confirming must close the palette"
        );
        assert_eq!(
            fixture.focused(cx),
            Some(fixture.focus.clone()),
            "closing must hand focus back so the command dispatches where the user was"
        );
    }

    #[gpui::test]
    fn escape_dismisses_without_executing(cx: &mut TestAppContext) {
        let fixture = setup(cx);
        let (events, _subscription) = subscribe(&fixture, cx);
        fixture.open(cx);

        fixture.press(cx, "escape");

        assert_eq!(*events.borrow(), vec![PaletteEvent::Dismissed]);
        assert!(
            !fixture
                .palette
                .read_with(cx, |palette, _| palette.is_open())
        );
        assert_eq!(
            fixture.focused(cx),
            Some(fixture.focus.clone()),
            "dismissing must restore focus too"
        );
    }

    /// The list is taller than the viewport at `MAX_VISIBLE_ROWS`; selection
    /// moving past the last visible row must scroll the list (PR #13 fixed the
    /// rows staying put) while movement inside the viewport must not jump.
    #[gpui::test]
    fn arrow_keys_keep_the_selected_row_inside_the_viewport(cx: &mut TestAppContext) {
        let fixture = setup(cx);
        fixture.open(cx);

        for _ in 0..3 {
            fixture.press(cx, "down");
        }
        fixture.draw(cx);
        assert_eq!(fixture.selected(cx), 3);
        assert_eq!(
            fixture.scroll_offset(cx).y,
            px(0.),
            "selection inside the first viewport must not scroll"
        );

        let target = MAX_VISIBLE_ROWS + 4;
        for _ in 3..target {
            fixture.press(cx, "down");
        }
        fixture.draw(cx);
        let selected = fixture.selected(cx);
        assert_eq!(selected, target);
        let offset = fixture.scroll_offset(cx);
        assert!(
            offset.y <= px(-ROW_HEIGHT * (selected + 1 - MAX_VISIBLE_ROWS) as f32),
            "the selected row must be scrolled into the viewport, got {offset:?}"
        );

        // Wrapping all the way back to the top scrolls the list home.
        for _ in 0..selected {
            fixture.press(cx, "up");
        }
        fixture.draw(cx);
        assert_eq!(fixture.selected(cx), 0);
        assert_eq!(
            fixture.scroll_offset(cx).y,
            px(0.),
            "selecting the first row must scroll back to the top"
        );
    }
}
