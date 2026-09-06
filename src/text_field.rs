//! Shared single-line text input using GPUI's platform input/IME contract.
use gpui::{prelude::*, *};
use std::{ops::Range, time::Duration};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Annotation(usize),
    Watermark,
    Time(usize, bool),
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    Focus,
    Change,
    Commit,
    Cancel,
}
#[derive(Clone, Debug)]
pub(crate) struct FieldEvent {
    pub target: Target,
    pub kind: EventKind,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    text: String,
    anchor: usize,
    cursor: usize,
}
#[derive(Default)]
struct Buffer {
    state: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}
impl Buffer {
    fn range(&self) -> Range<usize> {
        self.state.anchor.min(self.state.cursor)..self.state.anchor.max(self.state.cursor)
    }
    fn select(&mut self, offset: usize, extend: bool) {
        let offset = self.boundary(offset);
        self.state.cursor = offset;
        if !extend {
            self.state.anchor = offset;
        }
    }
    fn boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.state.text.len());
        while !self.state.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
    fn replace(&mut self, range: Range<usize>, text: &str) {
        let start = self.boundary(range.start);
        let end = self.boundary(range.end).max(start);
        let text = text.replace(['\r', '\n'], " ");
        if self.state.text[start..end] == text {
            self.select(start + text.len(), false);
            return;
        }
        self.undo.push(self.state.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.state.text.replace_range(start..end, &text);
        self.select(start + text.len(), false);
    }
    fn step(&self, right: bool, word: bool) -> usize {
        let cursor = self.state.cursor;
        let text = &self.state.text;
        if word {
            if right {
                return text
                    .unicode_word_indices()
                    .map(|(i, w)| i + w.len())
                    .find(|i| *i > cursor)
                    .unwrap_or(text.len());
            }
            return text
                .unicode_word_indices()
                .map(|(i, _)| i)
                .filter(|i| *i < cursor)
                .last()
                .unwrap_or(0);
        }
        if right {
            text.grapheme_indices(true)
                .map(|(i, _)| i)
                .find(|i| *i > cursor)
                .unwrap_or(text.len())
        } else {
            text.grapheme_indices(true)
                .map(|(i, _)| i)
                .filter(|i| *i < cursor)
                .last()
                .unwrap_or(0)
        }
    }
    fn move_cursor(&mut self, right: bool, word: bool, extend: bool) {
        let range = self.range();
        let offset = if !extend && !word && !range.is_empty() {
            if right {
                range.end
            } else {
                range.start
            }
        } else {
            self.step(right, word)
        };
        self.select(offset, extend);
    }
    fn delete(&mut self, right: bool, word: bool) {
        if self.range().is_empty() {
            self.select(self.step(right, word), true);
        }
        self.replace(self.range(), "");
    }
    fn undo(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some(state) = from.pop() {
            to.push(std::mem::replace(&mut self.state, state));
        }
    }
    fn utf8(&self, utf16: usize) -> usize {
        let mut units = 0;
        for (i, c) in self.state.text.char_indices() {
            if units + c.len_utf16() > utf16 {
                return i;
            }
            units += c.len_utf16();
        }
        self.state.text.len()
    }
    fn utf16(&self, byte: usize) -> usize {
        self.state.text[..self.boundary(byte)]
            .encode_utf16()
            .count()
    }
    fn from_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.utf8(range.start)..self.utf8(range.end)
    }
    fn to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.utf16(range.start)..self.utf16(range.end)
    }
}

pub(crate) struct TextField {
    pub focus: FocusHandle,
    parent_focus: FocusHandle,
    pub target: Target,
    buffer: Buffer,
    initial: String,
    placeholder: SharedString,
    marked: Option<Range<usize>>,
    layout: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    selecting: bool,
    focused: bool,
    blink: bool,
    _subscriptions: Vec<Subscription>,
    _blink_task: Task<()>,
}
impl EventEmitter<FieldEvent> for TextField {}
impl TextField {
    pub fn new(parent_focus: FocusHandle, placeholder: &str, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let blink = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(530)).await;
            if weak
                .update(cx, |this, cx| {
                    if this.focused {
                        this.blink = !this.blink;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        });
        Self {
            focus,
            parent_focus,
            target: Target::None,
            buffer: Buffer::default(),
            initial: String::new(),
            placeholder: placeholder.to_owned().into(),
            marked: None,
            layout: None,
            bounds: None,
            scroll: px(0.),
            selecting: false,
            focused: false,
            blink: true,
            _subscriptions: Vec::new(),
            _blink_task: blink,
        }
    }
    fn update_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        self.blink = true;
        if focused {
            self.initial = self.buffer.state.text.clone();
            self.emit(EventKind::Focus, cx);
        } else {
            self.marked = None;
            self.selecting = false;
            self.emit(EventKind::Commit, cx);
        }
        cx.notify();
    }
    pub fn sync(
        &mut self,
        target: Target,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_focus(self.focus.is_focused(window), cx);
        let changed_target = self.target != target;
        if changed_target || (!self.focus.is_focused(window) && self.buffer.state.text != value) {
            if changed_target && self.focus.is_focused(window) {
                self.update_focus(false, cx);
                self.parent_focus.focus(window);
            }
            self.target = target;
            self.buffer = Buffer::default();
            self.buffer.state.text = value.into();
            self.buffer.select(value.len(), false);
            self.initial = value.into();
            self.marked = None;
            self.scroll = px(0.);
            cx.notify();
        }
    }
    fn emit(&self, kind: EventKind, cx: &mut Context<Self>) {
        cx.emit(FieldEvent {
            target: self.target,
            kind,
            text: self.buffer.state.text.clone(),
        });
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.blink = true;
        self.emit(EventKind::Change, cx);
        cx.notify();
    }
    fn index(&self, pos: Point<Pixels>) -> usize {
        match (&self.layout, self.bounds) {
            (Some(line), Some(bounds)) => self
                .buffer
                .boundary(line.closest_index_for_x(pos.x - bounds.left() + self.scroll)),
            _ => 0,
        }
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        self.selecting = true;
        self.marked = None;
        let index = self.index(event.position);
        if event.click_count >= 3 {
            self.buffer.select(0, false);
            self.buffer.select(self.buffer.state.text.len(), true);
        } else if event.click_count == 2 {
            let word = self
                .buffer
                .state
                .text
                .split_word_bound_indices()
                .find(|(i, w)| index >= *i && index < i + w.len())
                .map(|(i, w)| i..i + w.len())
                .unwrap_or(index..index);
            self.buffer.select(word.start, false);
            self.buffer.select(word.end, true);
        } else {
            self.buffer.select(index, event.modifiers.shift);
        }
        self.blink = true;
        cx.stop_propagation();
        cx.notify();
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        let command = mods.control || mods.platform;
        let mut changed = false;
        match key {
            "left" | "right" => {
                self.buffer.move_cursor(key == "right", command, mods.shift);
                self.marked = None;
            }
            "home" | "end" => {
                self.buffer.select(
                    if key == "home" {
                        0
                    } else {
                        self.buffer.state.text.len()
                    },
                    mods.shift,
                );
                self.marked = None;
            }
            "backspace" | "delete" => {
                self.buffer.delete(key == "delete", command);
                self.marked = None;
                changed = true;
            }
            "a" if command => {
                self.buffer.select(0, false);
                self.buffer.select(self.buffer.state.text.len(), true);
            }
            "c" | "x" if command => {
                let range = self.buffer.range();
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.state.text[range.clone()].into(),
                    ));
                    if key == "x" {
                        self.buffer.replace(range, "");
                        changed = true;
                    }
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.buffer.replace(self.buffer.range(), &text);
                    self.marked = None;
                    changed = true;
                }
            }
            "z" | "y" if command => {
                self.buffer.undo(key == "y" || mods.shift);
                self.marked = None;
                changed = true;
            }
            "enter" => {
                self.marked = None;
                self.parent_focus.focus(window);
            }
            "escape" => {
                self.buffer.state.text = self.initial.clone();
                self.buffer.select(self.initial.len(), false);
                self.marked = None;
                self.emit(EventKind::Cancel, cx);
                self.parent_focus.focus(window);
            }
            "tab" => {
                if mods.shift {
                    window.focus_prev();
                } else {
                    window.focus_next();
                }
            }
            _ => return, // Printable text and IME commits go through EntityInputHandler.
        }
        self.blink = true;
        if changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
        cx.stop_propagation();
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.from_utf16(range);
        *actual = Some(self.buffer.to_utf16(range.clone()));
        Some(self.buffer.state.text[range].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.to_utf16(self.buffer.range()),
            reversed: self.buffer.state.cursor < self.buffer.state.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|r| self.buffer.to_utf16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.buffer.from_utf16(r))
            .or(self.marked.take())
            .unwrap_or_else(|| self.buffer.range());
        self.buffer.replace(range, text);
        self.marked = None;
        self.changed(cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.buffer.from_utf16(r))
            .or(self.marked.take())
            .unwrap_or_else(|| self.buffer.range());
        let start = range.start;
        let start_utf16 = self.buffer.utf16(start);
        self.buffer.replace(range, text);
        self.marked = (!text.is_empty()).then_some(start..self.buffer.state.cursor);
        if let Some(selection) = selection {
            let range = self
                .buffer
                .from_utf16(start_utf16 + selection.start..start_utf16 + selection.end);
            self.buffer.select(range.start, false);
            self.buffer.select(range.end, true);
        }
        self.changed(cx);
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        let line = self.layout.as_ref()?;
        let range = self.buffer.from_utf16(range);
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(range.start) - self.scroll,
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(range.end) - self.scroll,
                bounds.bottom(),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.utf16(self.index(point)))
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._subscriptions.is_empty() {
            let focus = self.focus.clone();
            self._subscriptions
                .push(cx.on_focus(&focus, window, |this, _, cx| this.update_focus(true, cx)));
            self._subscriptions
                .push(cx.on_blur(&focus, window, |this, _, cx| this.update_focus(false, cx)));
        }
        self.update_focus(self.focus.is_focused(window), cx);
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let focused = self.focus.is_focused(window);
        div()
            .id("text-field")
            .w_full()
            .min_w_0()
            .h(px(32.))
            .px_2()
            .py_1()
            .border_1()
            .rounded_md()
            .border_color(rgb(if focused { 0x2997ff } else { 0xd9d9dc }))
            .bg(white())
            .text_color(rgb(0x202124))
            .text_size(px(14.))
            .line_height(px(22.))
            .track_focus(&self.focus)
            .tab_index(0)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting && event.dragging() {
                    this.buffer.select(this.index(event.position), true);
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, window, _| {
                if this.focus.is_focused(window) {
                    this.parent_focus.focus(window);
                }
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let input = entity.read(cx);
                        let style = window.text_style();
                        let empty = input.buffer.state.text.is_empty();
                        let text: SharedString = if empty {
                            input.placeholder.clone()
                        } else {
                            input.buffer.state.text.clone().into()
                        };
                        let color = if empty {
                            rgb(0x85858c).into()
                        } else {
                            style.color
                        };
                        let run = TextRun {
                            len: text.len(),
                            font: style.font(),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = window.text_system().shape_line(text, px(14.), &[run], None);
                        let cursor = line.x_for_index(input.buffer.state.cursor);
                        let mut scroll = input.scroll;
                        if focused {
                            if cursor < scroll {
                                scroll = cursor;
                            }
                            if cursor > scroll + bounds.size.width - px(2.) {
                                scroll = (cursor - bounds.size.width + px(2.)).max(px(0.));
                            }
                        }
                        scroll = scroll
                            .min((line.width - bounds.size.width + px(2.)).max(px(0.)))
                            .max(px(0.));
                        (
                            line,
                            scroll,
                            input.buffer.range(),
                            input.marked.clone(),
                            input.blink,
                        )
                    },
                    move |bounds, (line, scroll, selection, marked, blink), window, cx| {
                        let focus = paint_entity.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_entity.clone()),
                            cx,
                        );
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            let origin = point(bounds.left() - scroll, bounds.top());
                            if !selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::from_corners(
                                        point(
                                            origin.x + line.x_for_index(selection.start),
                                            bounds.top(),
                                        ),
                                        point(
                                            origin.x + line.x_for_index(selection.end),
                                            bounds.bottom(),
                                        ),
                                    ),
                                    rgba(if focused { 0x2997ff55 } else { 0x99999933 }),
                                ));
                            }
                            let _ = line.paint(origin, px(22.), window, cx);
                            if let Some(marked) = marked {
                                window.paint_quad(fill(
                                    Bounds::from_corners(
                                        point(
                                            origin.x + line.x_for_index(marked.start),
                                            bounds.bottom() - px(1.),
                                        ),
                                        point(
                                            origin.x + line.x_for_index(marked.end),
                                            bounds.bottom(),
                                        ),
                                    ),
                                    rgb(0x2997ff),
                                ));
                            }
                            if focused && blink && selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            origin.x + line.x_for_index(selection.start),
                                            bounds.top() + px(2.),
                                        ),
                                        size(px(1.), bounds.size.height - px(4.)),
                                    ),
                                    rgb(0x202124),
                                ));
                            }
                        });
                        paint_entity.update(cx, |input, _| {
                            input.layout = Some(line);
                            input.bounds = Some(bounds);
                            input.scroll = scroll;
                        });
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;
    #[test]
    fn editing_selection_clipboard_replacement_and_history() {
        let mut b = Buffer::default();
        b.replace(0..0, "hello world");
        b.select(6, false);
        b.select(11, true);
        b.replace(b.range(), "Lahza");
        assert_eq!(b.state.text, "hello Lahza");
        b.undo(false);
        assert_eq!(b.state.text, "hello world");
        assert_eq!(b.range(), 6..11);
        b.undo(true);
        assert_eq!(b.state.text, "hello Lahza");
        b.select(0, false);
        b.move_cursor(true, true, true);
        b.replace(b.range(), "Hi");
        assert_eq!(b.state.text, "Hi Lahza");
    }
    #[test]
    fn unicode_deletion_uses_graphemes_and_ime_uses_utf16() {
        let mut b = Buffer::default();
        b.replace(0..0, "A👩‍💻e\u{301}لحظہ");
        b.select("A👩‍💻e\u{301}".len(), false);
        b.delete(false, false);
        assert_eq!(b.state.text, "A👩‍💻لحظہ");
        b.delete(false, false);
        assert_eq!(b.state.text, "Aلحظہ");
        b.replace(0..b.state.text.len(), "A😀B");
        assert_eq!(b.from_utf16(1..3), 1..5);
        assert_eq!(b.to_utf16(1..5), 1..3);
        assert_eq!(b.utf8(2), 1); // Never split a surrogate pair.
    }
    #[test]
    fn selection_reverses_and_delete_respects_boundaries() {
        let mut b = Buffer::default();
        b.replace(0..0, "one two");
        b.select(4, false);
        b.select(0, true);
        b.move_cursor(true, false, false);
        assert_eq!(b.range(), 4..4);
        b.delete(true, true);
        assert_eq!(b.state.text, "one ");
        b.select(0, false);
        b.delete(false, false);
        assert_eq!(b.state.text, "one ");
    }
}
