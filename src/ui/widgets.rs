// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Custom Hey widgets styled to match the TorTor Industrial Dark reference.

use std::cell::Cell;

use hey_core::{Callback, Color, CursorIcon, Modifiers, MsgSink, NodeId, Point, Rect, Size};
use hey_draw::{Border, DisplayList, DrawCmd};
use hey_layout::{Dim, Style};
use hey_style::WidgetState;
use hey_text::{layout, measure, Align as TextAlign, TextEditState, TextStyle};
use hey_widgets::{Env, SharedClipboard, Ui, View, Widget};

use super::icons::ensure_icons;
use super::theme::{
    ACCENT, BAR_TRACK, CONTROL_BG, CONTROL_HOVER, CYAN, GREEN, INPUT_BG, LINE, RED, ROW_DIVIDER,
    ROW_HOVER, ROW_SELECTED, TEXT, TEXT_DIM, TEXT_MUTED,
};

fn text_style(env: &Env, size_px: f32, max_width: Option<f32>, align: TextAlign) -> TextStyle {
    let scaled_px = (size_px * env.theme.scale()).max(1.0);
    let mut st = TextStyle::new(env.chain.clone(), scaled_px);
    st.max_width = max_width;
    st.wrap = false;
    st.ellipsis = max_width.is_some();
    st.align = align;
    st
}

fn draw_text(
    env: &Env,
    text: &str,
    size_px: f32,
    pos: Point,
    max_width: Option<f32>,
    align: TextAlign,
    color: Color,
    out: &mut DisplayList,
) {
    let st = text_style(env, size_px, max_width, align);
    let l = layout(&env.fonts, text, &st);
    for run in l.to_runs(pos, color) {
        out.push(DrawCmd::Text(run));
    }
}

/// Invisible spacer whose size scales with `env.theme.scale()`.
pub struct ScaledBox;

impl ScaledBox {
    pub fn add<Msg: 'static>(ui: &mut Ui<Msg>, parent: NodeId, base_w: f32, base_h: f32) -> NodeId {
        let w = if base_w > 0.0 {
            ui.env.theme.space(base_w)
        } else {
            0.0
        };
        let h = if base_h > 0.0 {
            ui.env.theme.space(base_h)
        } else {
            0.0
        };
        let mut st = Style::leaf().pass_through();
        st.width = Dim::Px(w);
        st.height = Dim::Px(h);
        ui.add_widget(parent, st, ScaledBox, move |_| Size::new(w, h))
    }
}

impl<Msg> Widget<Msg> for ScaledBox {
    fn paint(&self, _env: &Env, _rect: Rect, _state: WidgetState, _out: &mut DisplayList) {}
}

/// Container with custom background color and optional 1px edges.
pub struct Surface {
    pub fill: Color,
    pub border_top: Option<Color>,
    pub border_right: Option<Color>,
    pub border_bottom: Option<Color>,
    pub border_left: Option<Color>,
    pub chamfer: f32,
}

impl Surface {
    pub fn container<Msg: 'static>(
        key: impl Into<hey_core::Key>,
        outer_style: Style,
        inner_style: Style,
        surface: Surface,
        children: Vec<View<Msg>>,
    ) -> View<Msg> {
        let mut stack_style = Style::stack();
        stack_style.width = outer_style.width;
        stack_style.height = outer_style.height;
        stack_style.grow = outer_style.grow;
        stack_style.shrink = outer_style.shrink;
        stack_style.z = outer_style.z;
        View::container(
            key,
            stack_style,
            vec![
                View::leaf("__bg", (), move |ui, p| {
                    ui.add_widget(p, Style::leaf().pass_through(), surface, |_| {
                        Size::new(0.0, 0.0)
                    })
                }),
                View::container("__inner", inner_style, children),
            ],
        )
    }
}

impl<Msg> Widget<Msg> for Surface {
    fn paint(&self, _env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        out.push(DrawCmd::Rect {
            rect,
            fill: self.fill,
            border: None,
            chamfer: self.chamfer,
            shadow: None,
        });
        if let Some(c) = self.border_top {
            out.push(DrawCmd::Line {
                a: Point::new(rect.x, rect.y + 0.5),
                b: Point::new(rect.right(), rect.y + 0.5),
                width: 1.0,
                color: c,
            });
        }
        if let Some(c) = self.border_bottom {
            out.push(DrawCmd::Line {
                a: Point::new(rect.x, rect.bottom() - 0.5),
                b: Point::new(rect.right(), rect.bottom() - 0.5),
                width: 1.0,
                color: c,
            });
        }
        if let Some(c) = self.border_left {
            out.push(DrawCmd::Line {
                a: Point::new(rect.x + 0.5, rect.y),
                b: Point::new(rect.x + 0.5, rect.bottom()),
                width: 1.0,
                color: c,
            });
        }
        if let Some(c) = self.border_right {
            out.push(DrawCmd::Line {
                a: Point::new(rect.right() - 0.5, rect.y),
                b: Point::new(rect.right() - 0.5, rect.bottom()),
                width: 1.0,
                color: c,
            });
        }
    }
}

/// Dimmed full-screen modal backdrop that absorbs clicks outside the dialog.
pub struct ModalBackdrop<Msg> {
    pub on_dismiss: Callback<Msg, ()>,
}

impl<Msg: 'static> ModalBackdrop<Msg> {
    pub fn container(
        key: impl Into<hey_core::Key>,
        on_dismiss: Callback<Msg, ()>,
        dialog_view: View<Msg>,
    ) -> View<Msg> {
        View::container(
            key,
            Style::stack().z(20),
            vec![
                View::leaf("__dim", (), move |ui, p| {
                    ui.add_widget(p, Style::leaf(), ModalBackdrop { on_dismiss }, |_| {
                        Size::new(0.0, 0.0)
                    })
                }),
                View::container(
                    "__center",
                    Style::column()
                        .justify(hey_layout::Justify::Center)
                        .align_items(hey_layout::Align::Center),
                    vec![dialog_view],
                ),
            ],
        )
    }
}

impl<Msg: 'static> Widget<Msg> for ModalBackdrop<Msg> {
    fn paint(&self, _env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        out.push(DrawCmd::fill(rect, Color::rgba(0, 0, 0, 165)));
    }
    fn click(&mut self, _env: &Env, _rect: Rect, _pos: Point, sink: &MsgSink<Msg>) {
        self.on_dismiss.emit(sink, ());
    }
}

/// Standalone TorTor logo image for the About modal.
pub struct LogoBadge;

impl LogoBadge {
    pub fn add<Msg: 'static>(ui: &mut Ui<Msg>, parent: NodeId, base_px: f32) -> NodeId {
        let px = ui.env.theme.space(base_px).max(24.0);
        let mut st = Style::leaf().pass_through();
        st.width = Dim::Px(px);
        st.height = Dim::Px(px);
        ui.add_widget(parent, st, LogoBadge, move |_| Size::new(px, px))
    }
}

impl<Msg> Widget<Msg> for LogoBadge {
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        ensure_icons(env);
        let px = rect.w.min(rect.h).round().max(16.0) as u32;
        if let Some(image) = env.icons.borrow_mut().get("tortor_logo", px, None) {
            out.push(DrawCmd::Image {
                rect: Rect::new(rect.x, rect.y, px as f32, px as f32),
                image,
            });
        }
    }
}

/// Sidebar header: Tortor logo icon + bold title.
pub struct SidebarHeader;

impl SidebarHeader {
    pub fn add<Msg: 'static>(ui: &mut Ui<Msg>, parent: NodeId) -> NodeId {
        let h = ui.env.theme.space(56.0);
        let w = ui.env.theme.space(196.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(parent, st, SidebarHeader, move |_| Size::new(w, h))
    }
}

impl<Msg> Widget<Msg> for SidebarHeader {
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        ensure_icons(env);
        let icon_px = env.theme.space(28.0).max(16.0) as u32;
        let pad_x = env.theme.space(16.0);
        let text_x = env.theme.space(54.0);
        if let Some(image) = env.icons.borrow_mut().get("tortor_logo", icon_px, None) {
            let y = rect.y + (rect.h - icon_px as f32) / 2.0;
            out.push(DrawCmd::Image {
                rect: Rect::new(rect.x + pad_x, y, icon_px as f32, icon_px as f32),
                image,
            });
        }
        let st = text_style(env, 17.0, None, TextAlign::Left);
        let l = layout(&env.fonts, "Tortor", &st);
        let ty = rect.y + (rect.h - l.height) / 2.0;
        for run in l.to_runs(Point::new(rect.x + text_x, ty), TEXT) {
            out.push(DrawCmd::Text(run));
        }
    }
}

/// Uppercase muted section caption (`МЕТКИ`, `ЛИМИТ`, etc.).
pub struct SectionLabel {
    pub text: String,
    pub pad_x: f32,
}

impl SectionLabel {
    pub fn add<Msg: 'static>(ui: &mut Ui<Msg>, parent: NodeId, text: &str, pad_x: f32) -> NodeId {
        let h = ui.env.theme.space(22.0);
        let w = ui.env.theme.space(196.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            SectionLabel {
                text: text.to_string(),
                pad_x,
            },
            move |_| Size::new(w, h),
        )
    }
}

impl<Msg> Widget<Msg> for SectionLabel {
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        let pad_x = env.theme.space(self.pad_x);
        let st = text_style(
            env,
            10.5,
            Some((rect.w - pad_x * 2.0).max(20.0)),
            TextAlign::Left,
        );
        let l = layout(&env.fonts, &self.text, &st);
        let ty = rect.y + (rect.h - l.height) / 2.0;
        for run in l.to_runs(Point::new(rect.x + pad_x, ty), TEXT_DIM) {
            out.push(DrawCmd::Text(run));
        }
    }
}

/// Clickable sidebar category / tag / limit item with optional right-aligned count/badge.
pub struct SidebarItem<Msg> {
    pub label: String,
    pub right_text: String,
    pub active: bool,
    pub on_click: Callback<Msg, ()>,
}

impl<Msg: 'static> SidebarItem<Msg> {
    pub fn add(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        label: &str,
        right_text: &str,
        active: bool,
        on_click: Callback<Msg, ()>,
    ) -> NodeId {
        let h = ui.env.theme.space(30.0);
        let w = ui.env.theme.space(196.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            SidebarItem {
                label: label.to_string(),
                right_text: right_text.to_string(),
                active,
                on_click,
            },
            move |_| Size::new(w, h),
        )
    }
}

impl<Msg: 'static> Widget<Msg> for SidebarItem<Msg> {
    fn cursor(&self) -> CursorIcon {
        CursorIcon::Pointer
    }
    fn focusable(&self) -> bool {
        true
    }
    fn paint(&self, env: &Env, rect: Rect, state: WidgetState, out: &mut DisplayList) {
        if self.active {
            out.push(DrawCmd::fill(rect, ROW_SELECTED));
            out.push(DrawCmd::fill(
                Rect::new(rect.x, rect.y + 3.0, 2.5, rect.h - 6.0),
                ACCENT,
            ));
        } else if state.hovered() {
            out.push(DrawCmd::fill(rect, ROW_HOVER));
        }

        let pad_x = env.theme.space(16.0);
        let right_reserve = env.theme.space(58.0);
        let fg = if self.active { TEXT } else { TEXT_MUTED };
        let st = text_style(
            env,
            13.0,
            Some((rect.w - right_reserve).max(20.0)),
            TextAlign::Left,
        );
        let l = layout(&env.fonts, &self.label, &st);
        let ty = rect.y + (rect.h - l.height) / 2.0;
        for run in l.to_runs(Point::new(rect.x + pad_x, ty), fg) {
            out.push(DrawCmd::Text(run));
        }

        if !self.right_text.is_empty() {
            let rst = text_style(env, 12.0, None, TextAlign::Right);
            let rl = layout(&env.fonts, &self.right_text, &rst);
            let ry = rect.y + (rect.h - rl.height) / 2.0;
            for run in rl.to_runs(Point::new(rect.right() - pad_x - rl.width, ry), TEXT_DIM) {
                out.push(DrawCmd::Text(run));
            }
        }
    }
    fn activate(&mut self, sink: &MsgSink<Msg>) {
        self.on_click.emit(sink, ());
    }
}

/// Toolbar action button (`+ Добавить`, or icon-only `pause`/`play`/`trash`).
pub struct ActionButton<Msg> {
    pub icon: &'static str,
    pub label: Option<String>,
    pub icon_color: Color,
    pub filled: bool,
    pub on_click: Callback<Msg, ()>,
}

impl<Msg: 'static> ActionButton<Msg> {
    pub fn add(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        icon: &'static str,
        label: Option<&str>,
        icon_color: Color,
        filled: bool,
        on_click: Callback<Msg, ()>,
    ) -> NodeId {
        let env = ui.env.clone();
        let h = env.theme.space(32.0);
        let w = if let Some(text) = label {
            let l = layout(
                &env.fonts,
                text,
                &text_style(&env, 13.0, None, TextAlign::Left),
            );
            env.theme.space(34.0) + l.width.ceil() + env.theme.space(14.0)
        } else {
            h
        };
        let mut st = Style::leaf();
        st.width = Dim::Px(w);
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            ActionButton {
                icon,
                label: label.map(str::to_string),
                icon_color,
                filled,
                on_click,
            },
            move |_| Size::new(w, h),
        )
    }
}

impl<Msg: 'static> Widget<Msg> for ActionButton<Msg> {
    fn cursor(&self) -> CursorIcon {
        CursorIcon::Pointer
    }
    fn focusable(&self) -> bool {
        true
    }
    fn paint(&self, env: &Env, rect: Rect, state: WidgetState, out: &mut DisplayList) {
        ensure_icons(env);
        let fill = if state.disabled() {
            Color::TRANSPARENT
        } else if state.pressed() {
            ACCENT.with_alpha(40)
        } else if state.hovered() {
            CONTROL_HOVER
        } else if self.filled {
            CONTROL_BG
        } else {
            Color::TRANSPARENT
        };
        let border = if self.filled || state.hovered() {
            Some(Border {
                width: 1.0,
                color: if state.hovered() {
                    LINE
                } else {
                    Color::hex(0x252932)
                },
            })
        } else {
            None
        };
        out.push(DrawCmd::Rect {
            rect,
            fill,
            border,
            chamfer: 4.0,
            shadow: None,
        });

        let icon_sz = env.theme.space(15.0).max(12.0) as u32;
        let tint = if state.disabled() {
            TEXT_DIM
        } else {
            self.icon_color
        };
        if let Some(img) = env.icons.borrow_mut().get(self.icon, icon_sz, Some(tint)) {
            let ix = if self.label.is_some() {
                rect.x + env.theme.space(11.0)
            } else {
                rect.x + (rect.w - icon_sz as f32) / 2.0
            };
            let iy = rect.y + (rect.h - icon_sz as f32) / 2.0;
            out.push(DrawCmd::Image {
                rect: Rect::new(ix, iy, icon_sz as f32, icon_sz as f32),
                image: img,
            });
        }
        if let Some(text) = &self.label {
            let l = layout(
                &env.fonts,
                text,
                &text_style(env, 13.0, None, TextAlign::Left),
            );
            let ty = rect.y + (rect.h - l.height) / 2.0;
            let fg = if state.disabled() { TEXT_DIM } else { TEXT };
            for run in l.to_runs(Point::new(rect.x + env.theme.space(32.0), ty), fg) {
                out.push(DrawCmd::Text(run));
            }
        }
    }
    fn activate(&mut self, sink: &MsgSink<Msg>) {
        self.on_click.emit(sink, ());
    }
}
/// Recessed search field with magnifying-glass icon, placeholder, and cyan bottom line.
pub struct FilterInput<Msg> {
    edit: TextEditState,
    placeholder: String,
    clipboard: SharedClipboard,
    on_change: Callback<Msg, String>,
    scroll: Cell<f32>,
}

impl<Msg: 'static> FilterInput<Msg> {
    pub fn add(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        text: &str,
        placeholder: &str,
        width: f32,
        on_change: Callback<Msg, String>,
    ) -> NodeId {
        let w = ui.env.theme.space(width);
        let h = ui.env.theme.space(32.0);
        let mut st = Style::leaf();
        st.width = Dim::Px(w);
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            FilterInput {
                edit: TextEditState::new(text),
                placeholder: placeholder.to_string(),
                clipboard: SharedClipboard,
                on_change,
                scroll: Cell::new(0.0),
            },
            move |_| Size::new(w, h),
        )
    }

    fn x_of(&self, env: &Env, byte: usize) -> f32 {
        measure(
            &env.fonts,
            &self.edit.text()[..byte],
            &env.chain,
            (13.0 * env.theme.scale()).max(1.0),
        )
    }

    fn changed(&self, before: &str, sink: &MsgSink<Msg>) {
        if self.edit.text() != before {
            self.on_change.emit(sink, self.edit.text().to_string());
        }
    }
}

impl<Msg: 'static> Widget<Msg> for FilterInput<Msg> {
    fn cursor(&self) -> CursorIcon {
        CursorIcon::Text
    }
    fn focusable(&self) -> bool {
        true
    }
    fn save_state(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new((
            self.scroll.get(),
            self.edit.anchor(),
            self.edit.caret(),
        )))
    }
    fn load_state(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(s) = state.downcast::<(f32, usize, usize)>() {
            let (scroll, anchor, caret) = *s;
            let t = self.edit.text();
            if anchor <= t.len()
                && caret <= t.len()
                && t.is_char_boundary(anchor)
                && t.is_char_boundary(caret)
            {
                self.edit.set_caret(anchor, false);
                self.edit.set_caret(caret, true);
            }
            self.scroll.set(scroll);
        }
    }
    fn paint(&self, env: &Env, rect: Rect, state: WidgetState, out: &mut DisplayList) {
        ensure_icons(env);
        out.push(DrawCmd::fill(rect, INPUT_BG));
        let line_col = if state.focused() {
            CYAN
        } else {
            Color::hex(0x00A8BD)
        };
        out.push(DrawCmd::Line {
            a: Point::new(rect.x, rect.bottom() - 1.0),
            b: Point::new(rect.right(), rect.bottom() - 1.0),
            width: 2.0,
            color: line_col,
        });

        let icon_sz = env.theme.space(14.0).max(12.0) as u32;
        if let Some(img) = env
            .icons
            .borrow_mut()
            .get("search", icon_sz, Some(TEXT_MUTED))
        {
            out.push(DrawCmd::Image {
                rect: Rect::new(
                    rect.x + env.theme.space(10.0),
                    rect.y + (rect.h - icon_sz as f32) / 2.0,
                    icon_sz as f32,
                    icon_sz as f32,
                ),
                image: img,
            });
        }

        let text_left = rect.x + env.theme.space(32.0);
        let inner_w = (rect.w - env.theme.space(42.0)).max(10.0);
        out.push(DrawCmd::ClipPush(Rect::new(
            text_left - 1.0,
            rect.y,
            inner_w + 2.0,
            rect.h,
        )));

        let shown = self.edit.text();
        let st = text_style(env, 13.0, None, TextAlign::Left);
        if shown.is_empty() {
            let l = layout(&env.fonts, &self.placeholder, &st);
            let ty = rect.y + (rect.h - l.height) / 2.0;
            for run in l.to_runs(Point::new(text_left, ty), TEXT_DIM) {
                out.push(DrawCmd::Text(run));
            }
            if state.focused() {
                out.push(DrawCmd::Line {
                    a: Point::new(text_left + 0.5, ty),
                    b: Point::new(text_left + 0.5, ty + l.height),
                    width: 1.0,
                    color: TEXT,
                });
            }
        } else {
            let caret_x = self.x_of(env, self.edit.caret());
            let mut sc = self.scroll.get();
            if caret_x - sc > inner_w {
                sc = caret_x - inner_w;
            } else if caret_x - sc < 0.0 {
                sc = caret_x;
            }
            self.scroll.set(sc);
            let l = layout(&env.fonts, shown, &st);
            let ty = rect.y + (rect.h - l.height) / 2.0;
            let ox = text_left - sc;
            if self.edit.has_selection() && state.focused() {
                let r = self.edit.selection();
                let (a, b) = (self.x_of(env, r.start), self.x_of(env, r.end));
                out.push(DrawCmd::fill(
                    Rect::new(ox + a, ty, b - a, l.height),
                    ACCENT.with_alpha(70),
                ));
            }
            for run in l.to_runs(Point::new(ox, ty), TEXT) {
                out.push(DrawCmd::Text(run));
            }
            if state.focused() {
                let cx = (ox + caret_x).floor() + 0.5;
                out.push(DrawCmd::Line {
                    a: Point::new(cx, ty),
                    b: Point::new(cx, ty + l.height),
                    width: 1.0,
                    color: TEXT,
                });
            }
        }
        out.push(DrawCmd::ClipPop);
    }
    fn press(&mut self, env: &Env, rect: Rect, pos: Point, clicks: u32, _sink: &MsgSink<Msg>) {
        if clicks >= 2 {
            self.edit.select_all();
            return;
        }
        let rel = pos.x - (rect.x + env.theme.space(32.0)) + self.scroll.get();
        let t = self.edit.text();
        let mut best = (0usize, f32::MAX);
        for i in t
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(t.len()))
        {
            let d = (self.x_of(env, i) - rel).abs();
            if d < best.1 {
                best = (i, d);
            }
        }
        self.edit.set_caret(best.0, false);
    }
    fn key(&mut self, code: &str, mods: Modifiers, sink: &MsgSink<Msg>) -> bool {
        let before = self.edit.text().to_string();
        let sh = mods.shift;
        let ctrl = if matches!(code, "a" | "c" | "x" | "v" | "z" | "y") {
            mods.command()
        } else {
            mods.word()
        };
        let e = &mut self.edit;
        match (code, ctrl) {
            ("ArrowLeft", false) => e.move_left(sh),
            ("ArrowLeft", true) => e.move_word_left(sh),
            ("ArrowRight", false) => e.move_right(sh),
            ("ArrowRight", true) => e.move_word_right(sh),
            ("Home", _) => e.home(sh),
            ("End", _) => e.end(sh),
            ("Backspace", false) => e.backspace(),
            ("Backspace", true) => e.delete_word_left(),
            ("Delete", false) => e.delete(),
            ("Delete", true) => e.delete_word_right(),
            ("Escape", _) if e.has_selection() => e.set_caret(e.caret(), false),
            ("a", true) => e.select_all(),
            ("c", true) => e.copy(&mut self.clipboard),
            ("x", true) => e.cut(&mut self.clipboard),
            ("v", true) => e.paste(&mut self.clipboard),
            ("z", true) => {
                if sh {
                    e.redo();
                } else {
                    e.undo();
                }
            }
            ("y", true) => {
                e.redo();
            }
            _ => return false,
        }
        self.changed(&before, sink);
        true
    }
    fn text(&mut self, text: &str, sink: &MsgSink<Msg>) -> bool {
        let before = self.edit.text().to_string();
        self.edit.insert(text);
        self.changed(&before, sink);
        true
    }
}

struct TableCols {
    name_x: f32,
    name_w: f32,
    size_x: f32,
    size_w: f32,
    prog_x: f32,
    prog_w: f32,
    down_x: f32,
    down_w: f32,
    up_x: f32,
    up_w: f32,
    eta_x: f32,
    eta_w: f32,
}

fn table_cols(env: &Env, rect: Rect) -> TableCols {
    let sp = |px: f32| env.theme.space(px);
    let pad = sp(16.0);
    let size_w = sp(70.0);
    let prog_w = sp(142.0);
    let down_w = sp(78.0);
    let up_w = sp(78.0);
    let eta_w = sp(64.0);
    let right = rect.right() - pad;
    let eta_x = right - eta_w;
    let up_x = eta_x - sp(8.0) - up_w;
    let down_x = up_x - sp(8.0) - down_w;
    let prog_x = down_x - sp(14.0) - prog_w;
    let size_x = prog_x - sp(12.0) - size_w;
    let name_x = rect.x + pad;
    let name_w = (size_x - sp(12.0) - name_x).max(sp(80.0));
    TableCols {
        name_x,
        name_w,
        size_x,
        size_w,
        prog_x,
        prog_w,
        down_x,
        down_w,
        up_x,
        up_w,
        eta_x,
        eta_w,
    }
}

/// Column header strip (`NAME`, `SIZE`, `PROGRESS`, `↓`, `↑`, `LEFT`).
pub struct TableHeader {
    pub cols: [String; 4],
}

impl TableHeader {
    pub fn add<Msg: 'static>(ui: &mut Ui<Msg>, parent: NodeId, cols: [String; 4]) -> NodeId {
        let h = ui.env.theme.space(30.0);
        let w = ui.env.theme.space(500.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(parent, st, TableHeader { cols }, move |_| Size::new(w, h))
    }
}

impl<Msg> Widget<Msg> for TableHeader {
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        out.push(DrawCmd::Line {
            a: Point::new(rect.x, rect.bottom() - 0.5),
            b: Point::new(rect.right(), rect.bottom() - 0.5),
            width: 1.0,
            color: LINE,
        });
        let c = table_cols(env, rect);
        let draw_col = |label: &str, x: f32, w: f32, align: TextAlign, out: &mut DisplayList| {
            let st = text_style(env, 10.5, Some(w), align);
            let l = layout(&env.fonts, label, &st);
            let ty = rect.y + (rect.h - l.height) / 2.0;
            let tx = match align {
                TextAlign::Right => x + w - l.width,
                _ => x,
            };
            for run in l.to_runs(Point::new(tx, ty), TEXT_DIM) {
                out.push(DrawCmd::Text(run));
            }
        };
        draw_col(&self.cols[0], c.name_x, c.name_w, TextAlign::Left, out);
        draw_col(&self.cols[1], c.size_x, c.size_w, TextAlign::Right, out);
        draw_col(&self.cols[2], c.prog_x, c.prog_w, TextAlign::Left, out);
        draw_col("↓", c.down_x, c.down_w, TextAlign::Right, out);
        draw_col("↑", c.up_x, c.up_w, TextAlign::Right, out);
        draw_col(&self.cols[3], c.eta_x, c.eta_w, TextAlign::Right, out);
    }
}

#[derive(Clone, Hash)]
pub struct TorrentRowProps {
    pub name: String,
    pub subtitle: String,
    pub status_kind: u8, // 0=downloading, 1=seeding, 2=done, 3=paused, 4=error
    pub size_text: String,
    pub progress_milli: u32,
    pub pct_text: String,
    pub down_text: String,
    pub up_text: String,
    pub eta_text: String,
    pub selected: bool,
}

/// Interactive two-line torrent table row matching the reference mockup.
pub struct TorrentRow<Msg> {
    pub props: TorrentRowProps,
    pub on_select: Callback<Msg, ()>,
}

impl<Msg: 'static> TorrentRow<Msg> {
    pub fn add(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        props: TorrentRowProps,
        on_select: Callback<Msg, ()>,
    ) -> NodeId {
        let h = ui.env.theme.space(54.0);
        let w = ui.env.theme.space(500.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(parent, st, TorrentRow { props, on_select }, move |_| {
            Size::new(w, h)
        })
    }
}

impl<Msg: 'static> Widget<Msg> for TorrentRow<Msg> {
    fn cursor(&self) -> CursorIcon {
        CursorIcon::Pointer
    }
    fn focusable(&self) -> bool {
        true
    }
    fn paint(&self, env: &Env, rect: Rect, state: WidgetState, out: &mut DisplayList) {
        if self.props.selected {
            out.push(DrawCmd::fill(rect, ROW_SELECTED));
            out.push(DrawCmd::fill(
                Rect::new(rect.x, rect.y + 4.0, 2.5, rect.h - 8.0),
                ACCENT,
            ));
        } else if state.hovered() {
            out.push(DrawCmd::fill(rect, ROW_HOVER));
        }
        let pad = env.theme.space(18.0);
        out.push(DrawCmd::Line {
            a: Point::new(rect.x + pad, rect.bottom() - 0.5),
            b: Point::new(rect.right() - pad, rect.bottom() - 0.5),
            width: 1.0,
            color: ROW_DIVIDER,
        });

        let c = table_cols(env, rect);
        let status_color = match self.props.status_kind {
            1 => GREEN,
            2 => CYAN,
            3 => TEXT_MUTED,
            4 => RED,
            _ => ACCENT,
        };

        draw_text(
            env,
            &self.props.name,
            13.5,
            Point::new(c.name_x, rect.y + env.theme.space(9.0)),
            Some(c.name_w),
            TextAlign::Left,
            TEXT,
            out,
        );
        draw_text(
            env,
            &self.props.subtitle,
            11.5,
            Point::new(c.name_x, rect.y + env.theme.space(30.0)),
            Some(c.name_w),
            TextAlign::Left,
            status_color,
            out,
        );

        let right_text = |text: &str, x: f32, w: f32, color: Color, out: &mut DisplayList| {
            let st = text_style(env, 12.5, Some(w), TextAlign::Right);
            let l = layout(&env.fonts, text, &st);
            let ty = rect.y + (rect.h - l.height) / 2.0;
            for run in l.to_runs(Point::new(x + w - l.width, ty), color) {
                out.push(DrawCmd::Text(run));
            }
        };

        right_text(&self.props.size_text, c.size_x, c.size_w, TEXT_MUTED, out);

        let pct_w = env.theme.space(40.0);
        let pct_gap = env.theme.space(6.0);
        let bar_w = (c.prog_w - pct_w - pct_gap).max(env.theme.space(40.0));
        let bar_h = env.theme.space(4.0).max(4.0);
        let bar_y = rect.y + (rect.h - bar_h) / 2.0;
        out.push(DrawCmd::fill(
            Rect::new(c.prog_x, bar_y, bar_w, bar_h),
            BAR_TRACK,
        ));
        let frac = (self.props.progress_milli as f32 / 1000.0).clamp(0.0, 1.0);
        if frac > 0.0 {
            let fill_col = if self.props.status_kind == 3 {
                TEXT_DIM
            } else {
                status_color
            };
            out.push(DrawCmd::fill(
                Rect::new(c.prog_x, bar_y, (bar_w * frac).max(3.0), bar_h),
                fill_col,
            ));
        }
        right_text(
            &self.props.pct_text,
            c.prog_x + bar_w + pct_gap,
            pct_w,
            TEXT_MUTED,
            out,
        );

        let down_col = if self.props.down_text == "—" {
            TEXT_DIM
        } else {
            TEXT
        };
        let up_col = if self.props.up_text == "—" {
            TEXT_DIM
        } else {
            TEXT
        };
        right_text(&self.props.down_text, c.down_x, c.down_w, down_col, out);
        right_text(&self.props.up_text, c.up_x, c.up_w, up_col, out);
        right_text(&self.props.eta_text, c.eta_x, c.eta_w, TEXT_MUTED, out);
    }
    fn activate(&mut self, sink: &MsgSink<Msg>) {
        self.on_select.emit(sink, ());
    }
}
/// Clean underline tab strip (`Общее`, `Файлы · N`, `Пиры · N`, `Трекеры`, `Логи`).
pub struct InspectorTabs<Msg> {
    pub labels: Vec<String>,
    pub active: usize,
    pub on_select: Callback<Msg, usize>,
}

impl<Msg: 'static> InspectorTabs<Msg> {
    pub fn add(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        labels: Vec<String>,
        active: usize,
        on_select: Callback<Msg, usize>,
    ) -> NodeId {
        let h = ui.env.theme.space(34.0);
        let w = ui.env.theme.space(400.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            InspectorTabs {
                labels,
                active,
                on_select,
            },
            move |_| Size::new(w, h),
        )
    }

    fn tab_spans(&self, env: &Env, rect: Rect) -> Vec<(f32, f32)> {
        let pad = env.theme.space(18.0);
        let gap = env.theme.space(8.0);
        let mut x = rect.x + pad;
        let mut spans = Vec::with_capacity(self.labels.len());
        for label in &self.labels {
            let l = layout(
                &env.fonts,
                label,
                &text_style(env, 12.5, None, TextAlign::Left),
            );
            let w = l.width.ceil() + pad;
            spans.push((x, x + w));
            x += w + gap;
        }
        spans
    }
}

impl<Msg: 'static> Widget<Msg> for InspectorTabs<Msg> {
    fn cursor(&self) -> CursorIcon {
        CursorIcon::Pointer
    }
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        out.push(DrawCmd::Line {
            a: Point::new(rect.x, rect.bottom() - 0.5),
            b: Point::new(rect.right(), rect.bottom() - 0.5),
            width: 1.0,
            color: LINE,
        });
        for (i, ((x0, x1), label)) in self
            .tab_spans(env, rect)
            .into_iter()
            .zip(&self.labels)
            .enumerate()
        {
            let active = i == self.active;
            let st = text_style(env, 12.5, None, TextAlign::Center);
            let l = layout(&env.fonts, label, &st);
            let ty = rect.y + (rect.h - l.height) / 2.0;
            let tx = x0 + ((x1 - x0) - l.width) / 2.0;
            let col = if active { TEXT } else { TEXT_MUTED };
            for run in l.to_runs(Point::new(tx, ty), col) {
                out.push(DrawCmd::Text(run));
            }
            if active {
                out.push(DrawCmd::Line {
                    a: Point::new(x0, rect.bottom() - 1.0),
                    b: Point::new(x1, rect.bottom() - 1.0),
                    width: 2.0,
                    color: ACCENT,
                });
            }
        }
    }
    fn click(&mut self, env: &Env, rect: Rect, pos: Point, sink: &MsgSink<Msg>) {
        for (i, (x0, x1)) in self.tab_spans(env, rect).into_iter().enumerate() {
            if pos.x >= x0 && pos.x <= x1 {
                self.on_select.emit(sink, i);
                return;
            }
        }
    }
}

/// Segmented piece-map visualization matching `Карта частей · X из Y` in the mockup.
pub struct PieceMapBar {
    pub left_text: String,
    pub right_text: String,
    pub buckets: Vec<u8>,
    pub completed_all: bool,
}

impl PieceMapBar {
    pub fn add<Msg: 'static>(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        left_text: String,
        right_text: String,
        buckets: Vec<u8>,
        completed_all: bool,
    ) -> NodeId {
        let h = ui.env.theme.space(56.0);
        let w = ui.env.theme.space(400.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            PieceMapBar {
                left_text,
                right_text,
                buckets,
                completed_all,
            },
            move |_| Size::new(w, h),
        )
    }
}

impl<Msg> Widget<Msg> for PieceMapBar {
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        draw_text(
            env,
            &self.left_text,
            12.0,
            Point::new(rect.x, rect.y),
            Some(rect.w * 0.55),
            TextAlign::Left,
            TEXT_MUTED,
            out,
        );
        let rst = text_style(env, 12.0, None, TextAlign::Right);
        let rl = layout(&env.fonts, &self.right_text, &rst);
        for run in rl.to_runs(Point::new(rect.right() - rl.width, rect.y), TEXT_MUTED) {
            out.push(DrawCmd::Text(run));
        }

        let bar_y = rect.y + env.theme.space(24.0);
        let bar_h = env.theme.space(22.0);
        let gap = env.theme.space(2.0).max(1.0);
        let segs = if self.buckets.is_empty() {
            64usize
        } else {
            self.buckets.len()
        };
        let seg_w = ((rect.w - gap * (segs.saturating_sub(1) as f32)) / segs as f32).max(2.0);
        for i in 0..segs {
            let x = rect.x + i as f32 * (seg_w + gap);
            if x + seg_w > rect.right() + 0.5 {
                break;
            }
            let code = self.buckets.get(i).copied().unwrap_or(0);
            let col = match code {
                2 => {
                    if self.completed_all {
                        GREEN
                    } else {
                        ACCENT
                    }
                }
                1 => CYAN,
                _ => CONTROL_HOVER,
            };
            out.push(DrawCmd::fill(Rect::new(x, bar_y, seg_w, bar_h), col));
        }
    }
}

/// Custom-colored text leaf.
pub struct StyledLabel {
    pub text: String,
    pub size_px: f32,
    pub color: Color,
}

impl StyledLabel {
    pub fn add<Msg: 'static>(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        text: &str,
        size_px: f32,
        color: Color,
    ) -> NodeId {
        let env = ui.env.clone();
        let t = text.to_string();
        let measure_fn = move |w: Option<f32>| {
            let mut st = text_style(&env, size_px, w, TextAlign::Left);
            st.wrap = true;
            st.ellipsis = false;
            let l = layout(&env.fonts, &t, &st);
            Size::new(l.width.ceil(), l.height.ceil())
        };
        ui.add_widget(
            parent,
            Style::leaf(),
            StyledLabel {
                text: text.to_string(),
                size_px,
                color,
            },
            measure_fn,
        )
    }
}

impl<Msg> Widget<Msg> for StyledLabel {
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        let mut st = text_style(env, self.size_px, Some(rect.w), TextAlign::Left);
        st.wrap = true;
        st.ellipsis = false;
        let l = layout(&env.fonts, &self.text, &st);
        for run in l.to_runs(Point::new(rect.x, rect.y), self.color) {
            out.push(DrawCmd::Text(run));
        }
    }
}

/// Bottom status bar showing total DL/UL speed, limits, DHT nodes, and port status.
pub struct StatusBarWidget<Msg> {
    pub down_text: String,
    pub up_text: String,
    pub limit_text: String,
    pub right_text: String,
    pub on_limits_click: Callback<Msg, ()>,
}

impl<Msg: 'static> StatusBarWidget<Msg> {
    pub fn add(
        ui: &mut Ui<Msg>,
        parent: NodeId,
        down_text: String,
        up_text: String,
        limit_text: String,
        right_text: String,
        on_limits_click: Callback<Msg, ()>,
    ) -> NodeId {
        let h = ui.env.theme.space(32.0);
        let w = ui.env.theme.space(500.0);
        let mut st = Style::leaf();
        st.height = Dim::Px(h);
        ui.add_widget(
            parent,
            st,
            StatusBarWidget {
                down_text,
                up_text,
                limit_text,
                right_text,
                on_limits_click,
            },
            move |_| Size::new(w, h),
        )
    }
}

impl<Msg: 'static> Widget<Msg> for StatusBarWidget<Msg> {
    fn cursor(&self) -> CursorIcon {
        CursorIcon::Pointer
    }
    fn paint(&self, env: &Env, rect: Rect, _state: WidgetState, out: &mut DisplayList) {
        let pad = env.theme.space(18.0);
        let mut x = rect.x + pad;
        let mut push_seg = |txt: &str, col: Color, out: &mut DisplayList| {
            let st = text_style(env, 12.0, None, TextAlign::Left);
            let l = layout(&env.fonts, txt, &st);
            let ty = rect.y + (rect.h - l.height) / 2.0;
            for run in l.to_runs(Point::new(x, ty), col) {
                out.push(DrawCmd::Text(run));
            }
            x += l.width;
        };

        push_seg(&self.down_text, ACCENT, out);
        push_seg(" · ", TEXT_DIM, out);
        push_seg(&self.up_text, GREEN, out);
        push_seg(" · ", TEXT_DIM, out);
        push_seg(&self.limit_text, TEXT_MUTED, out);

        let rst = text_style(env, 12.0, None, TextAlign::Right);
        let rl = layout(&env.fonts, &self.right_text, &rst);
        let ry = rect.y + (rect.h - rl.height) / 2.0;
        for run in rl.to_runs(Point::new(rect.right() - pad - rl.width, ry), TEXT_MUTED) {
            out.push(DrawCmd::Text(run));
        }
    }
    fn click(&mut self, _env: &Env, rect: Rect, pos: Point, sink: &MsgSink<Msg>) {
        if pos.x < rect.x + rect.w * 0.5 {
            self.on_limits_click.emit(sink, ());
        }
    }
}
