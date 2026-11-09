#![deny(clippy::unwrap_used, clippy::expect_used)]
extern crate gpui_kit as gpui;
use gpui::component::{
    button::*,
    input::{Input, InputState},
    plot::{
        AxisText, IntoPlot, Plot, PlotAxis,
        scale::{Scale, ScaleLinear},
    },
    *,
};
use gpui::*;
use gpui_kit_assets::IconName as AppIcon;
use std::{cell::Cell, path::PathBuf, rc::Rc};
use team_up_gpui_spike::*;

// Muted blue, warm yellow, and lime palette shared by bars and labels.
const ME_COLOR: u32 = 0x8dbdeb;
const THEM_COLOR: u32 = 0xf2df73;
const BOTH_COLOR: u32 = 0xb5d46b;

#[derive(IntoPlot)]
struct Ribbon {
    samples: Vec<Sample>,
    span: Span,
    notes: Vec<Note>,
    selection: Option<Span>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
impl Plot for Ribbon {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.bounds.set(bounds);
        let w = bounds.size.width.as_f32();
        let x = ScaleLinear::new([self.span.from as f64, self.span.to as f64], [0., w]);
        let mut rect = |left: f32, top: f32, width: f32, height: f32, color: Hsla, radius: f32| {
            if width <= 0. {
                return;
            }
            let mut quad = fill(
                Bounds::new(
                    bounds.origin + point(px(left), px(top)),
                    size(px(width), px(height)),
                ),
                color,
            );
            quad.corner_radii = Corners::all(px(radius));
            window.paint_quad(quad);
        };
        rect(0., 12., w, 36., rgb(0xf0f2f4).into(), 6.);
        for (span, state) in activity_runs(&self.samples) {
            let left = x.tick(&(span.from as f64)).unwrap_or(0.).clamp(0., w);
            let right = x.tick(&(span.to as f64)).unwrap_or(w).clamp(0., w);
            let color = match state {
                1 => ME_COLOR,
                2 => THEM_COLOR,
                _ => BOTH_COLOR,
            };
            rect(
                left,
                12.,
                right - left,
                36.,
                rgb(color).into(),
                ((right - left) / 2.).min(5.),
            );
        }
        // Notes live beneath the activity track, so their shading cannot imply speech.
        for n in &self.notes {
            let left = x.tick(&(n.span.from as f64)).unwrap_or(0.).clamp(0., w);
            let right = x.tick(&(n.span.to as f64)).unwrap_or(w).clamp(0., w);
            rect(left, 79., right - left, 6., rgb(0x9aa9b8).into(), 2.);
        }
        if let Some(s) = self.selection {
            let left = x.tick(&(s.from as f64)).unwrap_or(0.).clamp(0., w);
            let right = x.tick(&(s.to as f64)).unwrap_or(w).clamp(0., w);
            rect(left, 5., right - left, 50., rgba(0x586c8215).into(), 4.);
            rect(left, 5., right - left, 1., rgb(0x586c82).into(), 0.);
            rect(left, 54., right - left, 1., rgb(0x586c82).into(), 0.);
            for edge in [left, right] {
                rect(
                    (edge - 2.).clamp(0., w - 4.),
                    17.,
                    4.,
                    26.,
                    rgb(0x586c82).into(),
                    2.,
                );
            }
        }
        for t in (self.span.from..=self.span.to).step_by(900_000) {
            let p = x.tick(&(t as f64)).unwrap_or(0.).clamp(0., w - 1.);
            let hour = t.rem_euclid(3_600_000) == 0;
            rect(
                p,
                58.,
                1.,
                if hour { 6. } else { 3. },
                rgb(if hour { 0x9ca3af } else { 0xdde1e5 }).into(),
                0.,
            );
        }
        let ticks = std::iter::once(self.span.from).chain(hours(self.span));
        PlotAxis::new()
            .x(px(64.))
            .x_axis(false)
            .x_label(ticks.filter_map(|t| {
                x.tick(&(t as f64)).map(|p| {
                    AxisText::new(
                        local(t, false),
                        px(p.clamp(14., w - 30.)),
                        rgb(0x71717a).into(),
                    )
                    .font_size(px(10.))
                })
            }))
            .paint(&bounds, window, cx);
    }
}
struct Recorder {
    store: Store,
    samples: Vec<Sample>,
    span: Span,
    utterances: Vec<Utterance>,
    notes: Vec<Note>,
    recording: bool,
    start_page: bool,
    pending_start: i64,
    active_span: Option<Note>,
    clock: std::time::Instant,
    start_label: Entity<InputState>,
    start_description: Entity<InputState>,
    settings: bool,
    solo: bool,
    engine: usize,
    selection: Option<Span>,
    drag: Option<(i64, bool)>,
    popup: Option<f32>,
    context_range: (usize, usize),
    hover: String,
    status: String,
    label: Entity<InputState>,
    body: Entity<InputState>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    reply_input: Entity<InputState>,
    reply_open: Option<i64>,
    chart_bounds: Rc<Cell<Bounds<Pixels>>>,
    track_bounds: Rc<Cell<Bounds<Pixels>>>,
    scroll: ScrollHandle,
}
impl Recorder {
    fn new(
        store: Store,
        samples: Vec<Sample>,
        span: Span,
        utterances: Vec<Utterance>,
        notes: Vec<Note>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            store,
            samples,
            span,
            utterances,
            notes,
            scroll: ScrollHandle::new(),
            recording: false,
            start_page: false,
            pending_start: 0,
            active_span: None,
            clock: std::time::Instant::now(),
            start_label: cx.new(|cx| InputState::new(window, cx)),
            start_description: cx.new(|cx| InputState::new(window, cx)),
            settings: false,
            solo: false,
            engine: 0,
            selection: None,
            drag: None,
            popup: None,
            context_range: (0, 0),
            hover: String::new(),
            status: String::new(),
            label: cx.new(|cx| InputState::new(window, cx).placeholder("Label")),
            body: cx.new(|cx| InputState::new(window, cx).placeholder("Add a note…")),
            from: cx.new(|cx| InputState::new(window, cx)),
            to: cx.new(|cx| InputState::new(window, cx)),
            reply_input: cx.new(|cx| InputState::new(window, cx).placeholder("Reply…")),
            reply_open: None,
            chart_bounds: Rc::new(Cell::new(Bounds::default())),
            track_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }
    fn time_at(&self, p: Point<Pixels>, track: bool) -> i64 {
        if track {
            let b = self.track_bounds.get();
            let index = ((p.y - b.origin.y).as_f32() / b.size.height.as_f32()
                * self.utterances.len() as f32)
                .floor()
                .max(0.) as usize;
            self.utterances
                .get(index.min(self.utterances.len().saturating_sub(1)))
                .map(|u| u.time)
                .unwrap_or(self.span.from)
        } else {
            let b = self.chart_bounds.get();
            self.span.at(((p.x - b.origin.x) / b.size.width) as f64)
        }
    }
    fn open_note(&mut self, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.selection {
            self.popup = Some(y.clamp(105., 440.));
            let first = self.utterances.partition_point(|u| u.time < s.from);
            let last = self.utterances.partition_point(|u| u.time <= s.to);
            self.context_range = (
                first.saturating_sub(1),
                (last + 1).min(self.utterances.len()),
            );
            self.from
                .update(cx, |v, cx| v.set_value(local(s.from, true), window, cx));
            self.to
                .update(cx, |v, cx| v.set_value(local(s.to, true), window, cx));
            cx.notify();
        }
    }
    fn choose_boundary(
        &mut self,
        index: usize,
        start: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(u) = self.utterances.get(index) else {
            return;
        };
        let current = self.selection.unwrap_or(Span {
            from: u.time,
            to: u.time + 1,
        });
        let selected: Vec<_> = self
            .utterances
            .iter()
            .enumerate()
            .filter(|(_, u)| current.contains(u.time))
            .map(|(i, _)| i)
            .collect();
        let first = if start {
            index
        } else {
            selected.first().copied().unwrap_or(index).min(index)
        };
        let last = if start {
            selected.last().copied().unwrap_or(index).max(index)
        } else {
            index
        };
        if let Some(span) = message_span(&self.utterances, first, last) {
            self.selection = Some(span);
            self.from
                .update(cx, |v, cx| v.set_value(local(span.from, true), window, cx));
            self.to
                .update(cx, |v, cx| v.set_value(local(span.to, true), window, cx));
            cx.notify();
        }
    }
    fn timeline(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("activity-chart")
            .role(Role::Group)
            .aria_label("Voice activity chart")
            .h(px(92.))
            .w_full()
            .cursor_crosshair()
            .child(Ribbon {
                samples: self.samples.clone(),
                span: self.span,
                notes: self.notes.clone(),
                selection: self.selection,
                bounds: self.chart_bounds.clone(),
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let time = this.time_at(e.position, false);
                    let tolerance = (this.span.to - this.span.from) as f64 * 7.
                        / this.chart_bounds.get().size.width.as_f32() as f64;
                    let anchor = this
                        .selection
                        .map(|s| {
                            if (time - s.from).abs() as f64 <= tolerance {
                                s.to
                            } else if (time - s.to).abs() as f64 <= tolerance {
                                s.from
                            } else {
                                time
                            }
                        })
                        .unwrap_or(time);
                    this.drag = Some((anchor, false));
                    cx.notify();
                }),
            )
            .capture_any_mouse_up(cx.listener(|this, e: &MouseUpEvent, window, cx| {
                if this.drag.take().is_some() {
                    this.open_note(e.position.y.as_f32(), window, cx);
                }
            }))
    }
    fn annotation_page(&self, cx: &mut Context<Self>) -> Div {
        let selected = self.selection;
        let mut passage = div().v_flex().gap_2();
        let (first, last) = self.context_range;
        if first > 0 {
            passage = passage.child(
                Button::new("earlier")
                    .ghost()
                    .label("Show earlier messages")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.context_range.0 = this.context_range.0.saturating_sub(5);
                        cx.notify();
                    })),
            );
        }
        let count = self
            .utterances
            .iter()
            .filter(|u| selected.is_some_and(|s| s.contains(u.time)))
            .count();
        if count == 0 {
            passage = passage.child(div().text_sm().text_color(rgb(0x71717a)).child(
                "No transcript messages in this time range. Nearby messages are shown below.",
            ));
        }
        for (index, u) in self.utterances.iter().enumerate().take(last).skip(first) {
            let marked = selected.is_some_and(|s| s.contains(u.time));
            passage = passage.child(
                div()
                    .id(("refine-message", index))
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(if marked { 0xadc5dd } else { 0xe7e9ec }))
                    .bg(rgb(if marked { 0xedf4fb } else { 0xf8f9fa }))
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .text_xs()
                            .child(format!("{} · {}", u.speaker, local(u.time, true)))
                            .child(if marked { "Included" } else { "Context" }),
                    )
                    .child(u.text.clone())
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new(("span-start", index))
                                    .small()
                                    .ghost()
                                    .label("Start here")
                                    .accessibility_label(format!("Start at message {}", index + 1))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_boundary(index, true, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("span-end", index))
                                    .small()
                                    .ghost()
                                    .label("End here")
                                    .accessibility_label(format!("End at message {}", index + 1))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_boundary(index, false, window, cx)
                                    })),
                            ),
                    ),
            );
        }
        if last < self.utterances.len() {
            passage = passage.child(
                Button::new("later")
                    .ghost()
                    .label("Show later messages")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.context_range.1 =
                            (this.context_range.1 + 5).min(this.utterances.len());
                        cx.notify();
                    })),
            );
        }
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x27272a))
            .font_family(".AppleSystemUIFont")
            .text_sm()
            .p_4()
            .v_flex()
            .gap_3()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let Some((start, _)) = this.drag {
                    this.selection = Span::new(start, this.time_at(e.position, false)).ok();
                    cx.notify();
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, e: &MouseUpEvent, window, cx| {
                if this.drag.take().is_some() {
                    this.open_note(e.position.y.as_f32(), window, cx);
                }
            }))
            .child(self.page_navigation(cx))
            .child(
                div()
                    .id("span-page-title")
                    .role(Role::Heading)
                    .aria_label("Refine span")
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Refine span"),
            )
            .child(self.timeline(cx))
            .child(
                div()
                    .text_xs()
                    .child("Me: blue · Them: yellow · Both: lime · 15-minute ticks"),
            )
            .child(
                div()
                    .id("selected-message-count")
                    .role(Role::Status)
                    .aria_label(format!("Included messages: {count}"))
                    .child(format!(
                        "Included messages: {count} · Choose the first and last message"
                    )),
            )
            .child(
                div()
                    .id("span-transcript")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h(px(120.))
                    .child(passage),
            )
            .child(self.annotation_editor(cx))
            .child(self.footer(cx))
            .child(
                div()
                    .id("span-page-status")
                    .role(Role::Status)
                    .text_xs()
                    .child(self.status.clone()),
            )
    }
    fn page_navigation(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .justify_between()
            .child(
                Button::new("page-back")
                    .ghost()
                    .label("Back")
                    .icon(IconName::ArrowLeft)
                    .accessibility_label("Back")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.settings {
                            this.settings = false;
                        } else if this.start_page {
                            this.start_page = false;
                        } else {
                            this.popup = None;
                        }
                        cx.notify();
                    })),
            )
            .child(
                Button::new("page-settings")
                    .ghost()
                    .label("Settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings = true;
                        cx.notify();
                    })),
            )
    }
    fn fixture_now(&self) -> i64 {
        self.samples
            .last()
            .map(|s| s.time)
            .unwrap_or(self.span.from)
            + self.clock.elapsed().as_millis() as i64
    }
    fn begin_span(&mut self, blank: bool, cx: &mut Context<Self>) {
        self.active_span = Some(Note {
            span: Span {
                from: self.pending_start,
                to: self.pending_start + 1,
            },
            label: if blank {
                String::new()
            } else {
                self.start_label.read(cx).value().to_string()
            },
            body: if blank {
                String::new()
            } else {
                self.start_description.read(cx).value().to_string()
            },
        });
        self.start_page = false;
        self.status = "Span running · recording is controlled separately".into();
        cx.notify();
    }
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let mut bar = div()
            .h_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(rgb(0xe7e9ec))
            .flex_shrink_0()
            .child(
                Button::new("footer-record")
                    .large()
                    .flex_1()
                    .h(px(48.))
                    .icon(Icon::new(AppIcon::Mic).text_color(rgb(0xe66b72)))
                    .label(if self.recording {
                        "Stop recording"
                    } else {
                        "Record"
                    })
                    .accessibility_label(if self.recording {
                        "● Simulated recording · Stop"
                    } else {
                        "Start simulation"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.recording = !this.recording;
                        this.status = if this.recording {
                            "Simulation running — no audio captured"
                        } else {
                            "Simulation stopped"
                        }
                        .into();
                        cx.notify();
                    })),
            );
        if self.settings {
            return bar.child(
                Button::new("settings-done")
                    .large()
                    .flex_1()
                    .h(px(48.))
                    .primary()
                    .label("Done")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings = false;
                        cx.notify();
                    })),
            );
        }
        bar = bar.child(
            Button::new("footer-start")
                .large()
                .flex_1()
                .h(px(48.))
                .icon(Icon::new(AppIcon::ScanLine).text_color(rgb(0x548ed0)))
                .accessibility_label(if self.start_page {
                    "Start span"
                } else if self.active_span.is_some() {
                    "■ End span"
                } else {
                    "＋ Start span"
                })
                .label(if self.start_page {
                    "Start span"
                } else if self.active_span.is_some() {
                    "End span"
                } else {
                    "Start span"
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.start_page {
                        this.begin_span(false, cx);
                        return;
                    }
                    if let Some(mut note) = this.active_span.clone() {
                        note.span.to = this.fixture_now().max(note.span.from + 1);
                        if note.label.trim().is_empty() {
                            note.label = "Untitled span".into();
                        }
                        match this.store.save(&note).and_then(|_| this.store.notes()) {
                            Ok(notes) => {
                                this.notes = notes;
                                this.active_span = None;
                                this.status = "Span ended and saved locally".into();
                            }
                            Err(e) => this.status = e.to_string(),
                        }
                    } else {
                        this.pending_start = this.fixture_now();
                        this.start_page = true;
                        this.start_label
                            .update(cx, |v, cx| v.set_value("", window, cx));
                        this.start_description
                            .update(cx, |v, cx| v.set_value("", window, cx));
                    }
                    cx.notify();
                })),
        );
        if self.start_page {
            bar.child(
                Button::new("start-blank")
                    .large()
                    .flex_1()
                    .h(px(48.))
                    .primary()
                    .label("Start without details")
                    .on_click(cx.listener(|this, _, _, cx| this.begin_span(true, cx))),
            )
        } else if self.popup.is_some() {
            bar.child(
                Button::new("save")
                    .large()
                    .flex_1()
                    .h(px(48.))
                    .icon(Icon::new(AppIcon::Tag).text_color(rgb(0xe6a63b)))
                    .primary()
                    .label("Save span")
                    .accessibility_label("Save annotation")
                    .on_click(cx.listener(Self::save)),
            )
        } else {
            bar.child(
                Button::new("footer-label")
                    .large()
                    .flex_1()
                    .h(px(48.))
                    .icon(Icon::new(AppIcon::Tag).text_color(rgb(0xe6a63b)))
                    .label("Label span")
                    .accessibility_label("◇ Label span")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.selection.is_none() {
                            this.selection = this.utterances.last().and_then(|_| {
                                message_span(
                                    &this.utterances,
                                    this.utterances.len().saturating_sub(2),
                                    this.utterances.len() - 1,
                                )
                            });
                        }
                        this.open_note(0., window, cx);
                    })),
            )
        }
    }
    fn utility_page(&self, cx: &mut Context<Self>) -> Div {
        let mut page = div()
            .size_full()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x27272a))
            .font_family(".AppleSystemUIFont")
            .text_sm()
            .p_4()
            .v_flex()
            .gap_3()
            .child(self.page_navigation(cx));
        if self.settings {
            page = page
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Settings"),
                )
                .child(
                    Button::new("engine")
                        .label(format!(
                            "Engine: {}",
                            ["Nemotron", "Parakeet", "Whisper"][self.engine]
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.engine = (this.engine + 1) % 3;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("solo")
                        .label(if self.solo {
                            "Fixture: Me only"
                        } else {
                            "Fixture: Me and Them"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.solo = !this.solo;
                            match fixture(this.solo) {
                                Ok(s) => {
                                    if let Ok(span) = day_extent(&s) {
                                        this.span = span;
                                    }
                                    this.samples = s;
                                }
                                Err(e) => this.status = e.to_string(),
                            }
                            cx.notify();
                        })),
                );
        } else {
            page = page
                .child(div().text_lg().child("Start span"))
                .child(format!(
                    "Start captured at {} · simulation time",
                    local(self.pending_start, true)
                ))
                .child("Label (optional)")
                .child(Input::new(&self.start_label).aria_label("New span label"))
                .child("Description (optional)")
                .child(Input::new(&self.start_description).aria_label("New span description"));
        }
        page.child(div().flex_1())
            .child(div().text_xs().child(self.status.clone()))
            .child(self.footer(cx))
    }
    fn annotation_editor(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .pt_3()
            .border_t_1()
            .border_color(rgb(0xe7e9ec))
            .bg(rgb(0xffffff))
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child("From")
                            .child(Input::new(&self.from).aria_label("From")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child("To")
                            .child(Input::new(&self.to).aria_label("To")),
                    ),
            )
            .child("Label (optional)")
            .child(Input::new(&self.label).aria_label("Annotation label"))
            .child("Description (optional)")
            .child(Input::new(&self.body).aria_label("Annotation note"))
    }
    fn save(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let from = parse_local(&self.from.read(cx).value())?;
            let to = parse_local(&self.to.read(cx).value())?;
            let note = Note {
                span: Span { from, to },
                label: if self.label.read(cx).value().trim().is_empty() {
                    "Untitled span".into()
                } else {
                    self.label.read(cx).value().to_string()
                },
                body: self.body.read(cx).value().to_string(),
            };
            self.store.save(&note)?;
            self.notes = self.store.notes()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.popup = None;
                self.status = "Annotation saved locally".into();
                self.label.update(cx, |v, cx| v.set_value("", window, cx));
                self.body.update(cx, |v, cx| v.set_value("", window, cx));
            }
            Err(e) => self.status = e.to_string(),
        }
        cx.notify();
    }
}
impl Render for Recorder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.settings || self.start_page {
            return self.utility_page(cx).into_any_element();
        }
        if self.popup.is_some() {
            return self.annotation_page(cx).into_any_element();
        }
        let chart = self.timeline(cx);
        let track_cell = self.track_bounds.clone();
        let track = div()
            .id("transcript-track")
            .role(Role::Group)
            .aria_label("Transcript range selector")
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(20.))
            .cursor_crosshair()
            .child(
                canvas(
                    move |b, _, _| {
                        track_cell.set(b);
                    },
                    |b, _, w, _| {
                        w.paint_quad(fill(
                            Bounds::new(
                                b.origin + point(px(9.), px(0.)),
                                size(px(1.), b.size.height),
                            ),
                            rgb(0xd4d4d8),
                        ));
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.drag = Some((this.time_at(e.position, true), true));
                    this.popup = None;
                    cx.notify();
                }),
            )
            .capture_any_mouse_up(cx.listener(|this, e: &MouseUpEvent, window, cx| {
                if this.drag.take().is_some() {
                    this.open_note(e.position.y.as_f32(), window, cx);
                }
            }));
        let mut messages = div().v_flex().flex_1().gap_2();
        for u in self.utterances.clone() {
            let marked = self.selection.is_some_and(|s| s.contains(u.time));
            let id = u.id;
            let mut bubble = div()
                .id(("utterance", id as usize))
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(if marked { rgb(0xb9cfe2) } else { rgb(0xeeeeF0) })
                .bg(if marked {
                    rgb(0xedf2f6)
                } else if u.speaker == "Me" {
                    rgb(0xf7f8f8)
                } else {
                    rgb(0xffffff)
                })
                .text_sm()
                .child(
                    div()
                        .h_flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x71717a))
                                .child(u.speaker.clone()),
                        )
                        .child(
                            Button::new(("reply", id as usize))
                                .ghost()
                                .small()
                                .icon(Icon::new(AppIcon::MessageCircle).text_color(rgb(0x9b79c6)))
                                .accessibility_label(format!("Comment on message {}", id + 1))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.reply_open = Some(id);
                                    cx.notify();
                                })),
                        ),
                )
                .child(u.text)
                .on_hover(cx.listener(move |this, hover, _, cx| {
                    this.hover = if *hover {
                        local(u.time, true)
                    } else {
                        String::new()
                    };
                    cx.notify();
                }));
            for n in self.notes.iter().filter(|n| n.span.contains(u.time)) {
                bubble = bubble.child(
                    div()
                        .id(("note", id as usize))
                        .role(Role::Note)
                        .aria_label(format!("{} · {}", n.label, n.body))
                        .mt_2()
                        .text_color(rgb(0x6b8faf))
                        .child(format!("{} · {}", n.label, n.body)),
                );
            }
            if self.reply_open == Some(id) {
                if let Ok(replies) = self.store.replies(id) {
                    for (reply_index, reply) in replies.into_iter().enumerate() {
                        bubble = bubble.child(
                            div()
                                .id(("reply-body", reply_index))
                                .role(Role::Note)
                                .aria_label(reply.clone())
                                .mt_2()
                                .child(reply),
                        );
                    }
                }
                bubble = bubble.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .mt_2()
                        .child(Input::new(&self.reply_input).aria_label("Reply"))
                        .child(
                            Button::new(("send", id as usize))
                                .label("Save reply")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    match this.store.reply(id, &this.reply_input.read(cx).value()) {
                                        Ok(()) => {
                                            this.reply_input
                                                .update(cx, |v, cx| v.set_value("", window, cx));
                                            this.status = "Reply saved locally".into();
                                        }
                                        Err(e) => this.status = e.to_string(),
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            messages = messages.child(bubble);
        }
        let content = div()
            .id("recorder")
            .relative()
            .size_full()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x27272a))
            .font_family(".AppleSystemUIFont")
            .text_sm()
            .p(px(24.))
            .v_flex()
            .gap_3()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let Some((start, track)) = this.drag {
                    let end = this.time_at(e.position, track);
                    this.selection = Span::new(start, end).ok();
                }
                if this.chart_bounds.get().contains(&e.position) {
                    let time = this.time_at(e.position, false);
                    this.hover = local(time, false);
                    for n in &this.notes {
                        if n.span.contains(time) {
                            this.hover = format!("{} · {}", this.hover, n.label);
                        }
                    }
                }
                cx.notify();
            }))
            .capture_any_mouse_up(cx.listener(|this, e: &MouseUpEvent, window, cx| {
                if this.drag.take().is_some() {
                    this.open_note(e.position.y.as_f32(), window, cx);
                }
            }))
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .v_flex()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Worklog"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x71717a))
                                    .child("Sample data · 9 November 2026 · Europe/Berlin"),
                            ),
                    )
                    .child(
                        div().h_flex().gap_2().child(
                            Button::new("settings")
                                .small()
                                .accessibility_label("Settings")
                                .ghost()
                                .icon(IconName::Settings)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings = !this.settings;
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0xe7e7eb))
                    .bg(rgb(0xf8fafb))
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                                "Today · {} – {}",
                                local(self.span.from, false),
                                local(self.span.to, false)
                            )))
                            .child(
                                Button::new("annotate")
                                    .small()
                                    .ghost()
                                    .label("Annotate span")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.status =
                                            "Drag on the chart or the line beside the transcript"
                                                .into();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(chart)
                    .child(
                        div()
                            .h_flex()
                            .gap_4()
                            .text_xs()
                            .child(
                                div()
                                    .h_flex()
                                    .gap_1()
                                    .child(div().size(px(8.)).rounded_sm().bg(rgb(ME_COLOR)))
                                    .child("Me"),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap_1()
                                    .child(div().size(px(8.)).rounded_sm().bg(rgb(THEM_COLOR)))
                                    .child("Them"),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap_1()
                                    .child(div().size(px(8.)).rounded_sm().bg(rgb(BOTH_COLOR)))
                                    .child("Both speaking"),
                            )
                            .child(div().text_color(rgb(0x71717a)).child("Empty = silence")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x71717a))
                            .child("Drag to mark a span · Pull either edge to refine"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x71717a))
                            .child(self.hover.clone()),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Transcript"))
                    .child(
                        div().text_xs().text_color(rgb(0x71717a)).child(
                            self.selection
                                .map(|s| {
                                    format!("{} – {}", local(s.from, false), local(s.to, false))
                                })
                                .unwrap_or_else(|| "Latest passage".into()),
                        ),
                    ),
            )
            .child(
                div()
                    .id("transcript")
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .child(
                        div()
                            .relative()
                            .h_flex()
                            .gap_3()
                            .child(track)
                            .child(messages.ml(px(32.))),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(
                        div()
                            .id("status")
                            .role(Role::Status)
                            .aria_label(self.status.clone())
                            .text_xs()
                            .child(self.status.clone()),
                    )
                    .child(
                        Button::new("live")
                            .primary()
                            .small()
                            .label("↓ Back to live")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selection = None;
                                this.popup = None;
                                this.scroll.scroll_to_bottom();
                                this.status = "Latest passage".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(self.footer(cx));
        let _ = window;
        content.into_any_element()
    }
}
fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--version") {
        println!("worklog {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if std::env::args().any(|a| a == "--check-db") {
        let path = std::env::var_os("TEAM_UP_SPIKE_DB")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("Set TEAM_UP_SPIKE_DB for --check-db"))?;
        Store::open(&path)?;
        println!("Schema ready");
        return Ok(());
    }
    let path = match std::env::var_os("TEAM_UP_SPIKE_DB") {
        Some(path) => PathBuf::from(path),
        None => {
            let dir = PathBuf::from(
                std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME missing"))?,
            )
            .join("Library/Application Support/worklog");
            std::fs::create_dir_all(&dir)?;
            dir.join("worklog.sqlite")
        }
    };
    let runtime = tokio::runtime::Runtime::new()?;
    let (store, utterances, notes) =
        runtime.block_on(runtime.spawn_blocking(move || -> anyhow::Result<_> {
            let mut store = Store::open(&path)?;
            if let Some(path) =
                std::env::var_os("TEAM_UP_SPIKE_TRANSCRIPT").filter(|p| !p.is_empty())
            {
                store.import_transcript(&std::path::PathBuf::from(path))?;
            } else {
                store.seed()?;
            }
            let utterances = store.utterances()?;
            let notes = store.notes()?;
            Ok((store, utterances, notes))
        }))??;
    let samples = fixture(false)?;
    let span = day_extent(&samples)?;
    gpui::application()
        .with_assets(gpui_kit_assets::AllAssets)
        .run(move |cx| {
            gpui::init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(700.), px(830.)),
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Worklog — sample data".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(e) = gpui::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Recorder::new(store, samples, span, utterances, notes, window, cx))
            }) {
                eprintln!("Window failed: {e}");
                cx.quit();
            }
            cx.activate(true);
        });
    Ok(())
}
