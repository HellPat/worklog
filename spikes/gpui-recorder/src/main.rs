#![deny(clippy::unwrap_used, clippy::expect_used)]
extern crate gpui_kit as gpui;
use chrono::Datelike as _;
use gpui::component::{
    button::*,
    input::{Input, InputState, Textarea, TextareaState},
    plot::{IntoPlot, Plot},
    select::{Select, SelectEvent, SelectState},
    tooltip::Tooltip,
    *,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_kit_assets::IconName as AppIcon;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::PathBuf,
    rc::Rc,
};
use team_up_gpui_spike::*;

// Muted blue, warm yellow, and lime palette shared by bars and labels.
const ME_COLOR: u32 = 0x8dbdeb;
const THEM_COLOR: u32 = 0xf2df73;
const BOTH_COLOR: u32 = 0xb5d46b;

fn day_for(time: i64) -> chrono::NaiveDate {
    chrono::DateTime::from_timestamp_millis(time)
        .unwrap_or_default()
        .with_timezone(&chrono_tz::Europe::Berlin)
        .date_naive()
}
fn appearance_color(light: u32, cx: &App) -> Hsla {
    let value = if cx.theme().is_dark() {
        match light {
            0xffffff => 0x181a1e,
            0x27272a => 0xe8ebef,
            0x71717a => 0xa3aab5,
            0xf0f2f4 => 0x282c33,
            0xf8fafb | 0xf8f9fa | 0xf7f8f8 => 0x21242a,
            0xe7e9ec | 0xe7e7eb | 0xeeeef0 | 0xd4d4d8 | 0xdde1e5 => 0x363c46,
            0xedf4fb | 0xedf2f6 => 0x24364a,
            0xadc5dd | 0xb9cfe2 => 0x527397,
            0x586c82 => 0xa8c4e2,
            0x6b8faf => 0x96bfe6,
            0x548ed0 => 0x78ade9,
            _ => light,
        }
    } else {
        light
    };
    rgb(value).into()
}

#[derive(IntoPlot)]
struct MiniMap {
    samples: Vec<Sample>,
    recorded: Vec<(Span, u8)>,
    span: Span,
    notes: Vec<Note>,
    entries: Vec<TimelineEntry>,
    utterances: Vec<Utterance>,
    selection: Option<Span>,
    viewport: Option<Span>,
    activity_times: Vec<i64>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
impl Plot for MiniMap {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.bounds.set(bounds);
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32();
        let activity = if self.recorded.is_empty() {
            activity_runs(&self.samples)
        } else {
            self.recorded.clone()
        };
        // A sparse rail, with one short mark per screen row. Empty time stays
        // neutral; distinct turns in the same row do not invent an overlap.
        let rows = (height / 8.).floor().max(1.) as usize;
        let duration = (self.span.to - self.span.from).max(1);
        for index in 0..rows {
            let from = self.span.from + (duration as f64 * index as f64 / rows as f64) as i64;
            let to = self.span.from + (duration as f64 * (index + 1) as f64 / rows as f64) as i64;
            let contains = |time: i64| time >= from && time < to;
            let intersects = |span: Span| span.from < to && span.to > from;
            let mut coverage = [0_i64; 4];
            for (span, state) in &activity {
                if intersects(*span) {
                    coverage[usize::from((*state).min(3))] += span.to.min(to) - span.from.max(from);
                }
            }
            let speaker = if coverage[3] > 0 {
                3
            } else if coverage[1] > 0 || coverage[2] > 0 {
                if coverage[1] >= coverage[2] { 1 } else { 2 }
            } else {
                self.utterances
                    .iter()
                    .find(|u| contains(u.time))
                    .map(|u| {
                        if u.speaker.eq_ignore_ascii_case("Me") {
                            1
                        } else {
                            2
                        }
                    })
                    .unwrap_or(0)
            };
            let breakpoint = self
                .entries
                .iter()
                .any(|entry| entry.breakpoint && contains(entry.time));
            let note = self
                .entries
                .iter()
                .any(|entry| !entry.breakpoint && contains(entry.time))
                || self
                    .notes
                    .iter()
                    .any(|note| contains(note.span.from) || contains(note.span.to));
            let color = if breakpoint {
                0xe66b72
            } else if note {
                0x9b79c6
            } else {
                match speaker {
                    1 => ME_COLOR,
                    2 => THEM_COLOR,
                    3 => BOTH_COLOR,
                    _ if self.activity_times.iter().any(|time| contains(*time)) => 0x62b3a1,
                    _ => 0xd4d4d8,
                }
            };
            let selected = self.selection.is_some_and(intersects);
            let visible = self.viewport.is_some_and(intersects);
            let tick_width: f32 = if selected {
                22.
            } else if visible || breakpoint {
                18.
            } else {
                12.
            };
            let tick_width = tick_width.min(width);
            let color = if visible && color == 0xd4d4d8 {
                0x71717a
            } else {
                color
            };
            window.paint_quad(fill(
                Bounds::new(
                    bounds.origin
                        + point(
                            px((width - tick_width) / 2.),
                            px((index as f32 + 0.5) * height / rows as f32),
                        ),
                    size(px(tick_width), px(2.)),
                ),
                appearance_color(color, cx),
            ));
        }
    }
}
struct Recorder {
    store: Store,
    samples: Vec<Sample>,
    span: Span,
    utterances: Vec<Utterance>,
    notes: Vec<Note>,
    sidebar_collapsed: bool,
    editing_note: Option<Note>,
    recording: recording::Recording,
    meter_levels: [f32; 5],
    modal_background: Option<std::sync::Arc<RenderImage>>,
    tray: Option<tray_icon::TrayIcon>,
    navigation: navigation::Navigation,
    pending_start: i64,
    active_span: Option<Note>,
    start_label: Entity<InputState>,
    start_description: Entity<InputState>,
    all_samples: Vec<Sample>,
    selected_day: chrono::NaiveDate,
    calendar_today: chrono::NaiveDate,
    calendar_month: chrono::NaiveDate,
    recorded_activity: Vec<(Span, u8)>,
    scroll_to_live: bool,
    at_live_edge: bool,
    entries: Vec<TimelineEntry>,
    composer: Entity<TextareaState>,
    editing_breakpoint: Option<i64>,
    hovered_breakpoint: Option<i64>,
    annotation_anchor: Option<(i64, Pixels)>,
    breakpoint_label: Entity<InputState>,
    drafts: Vec<LiveUtterance>,
    engine: usize,
    input_device: Entity<SelectState<Vec<SharedString>>>,
    output_device: Entity<SelectState<Vec<SharedString>>>,
    audio_settings_path: PathBuf,
    selection: Option<Span>,
    drag: Option<(RangeGesture, bool)>,
    context_range: (usize, usize),
    status: String,
    capture_problem: Option<recording::CaptureProblem>,
    activity_capture: activity::Capture,
    activities: Vec<activity::ActivityEntry>,
    expanded_activity: Option<i64>,
    selected_activity_event: Option<i64>,
    activity_raw: Option<SharedString>,
    activity_authorized: Option<bool>,
    activity_error: Option<String>,
    show_capture_details: bool,
    label: Entity<InputState>,
    body: Entity<TextareaState>,
    chart_bounds: Rc<Cell<Bounds<Pixels>>>,
    track_bounds: Rc<Cell<Bounds<Pixels>>>,
    scroll: ScrollHandle,
    row_anchors: Rc<RefCell<BTreeMap<i64, Bounds<Pixels>>>>,
    last_scroll_offset: Point<Pixels>,
    hover_insert: Option<(i64, f32)>,
    time_menu: Option<(i64, Point<Pixels>)>,
    composer_time: Option<i64>,
    range_start: Option<i64>,
    pending_connection: Option<(i64, i64)>,
    minimap_scrolling: bool,
    hover_minimap: Option<(i64, f32)>,
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
        Theme::sync_system_appearance(Some(window), cx);
        cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            cx.notify();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let today = chrono::Utc::now()
                            .with_timezone(&chrono_tz::Europe::Berlin)
                            .date_naive();
                        if this.calendar_today != today {
                            this.calendar_today = today;
                            cx.notify();
                        }
                        let at_bottom =
                            (this.scroll.max_offset().y + this.scroll.offset().y).as_f32() <= 2.;
                        if this.at_live_edge != at_bottom {
                            this.at_live_edge = at_bottom;
                            cx.notify();
                        }
                        this.poll_recording(cx);
                        this.poll_activity(cx);
                        if this.last_scroll_offset != this.scroll.offset() {
                            this.last_scroll_offset = this.scroll.offset();
                            cx.notify();
                        }
                        let live_span = day_time_span(
                            this.selected_day,
                            chrono::Utc::now().timestamp_millis().div_euclid(1000) * 1000,
                        );
                        if live_span != this.span {
                            this.span = live_span;
                            cx.notify();
                        }
                        let levels = this.recording.levels().map(|v| (v * 40.).round() / 40.);
                        if levels != this.meter_levels {
                            this.meter_levels = levels;
                            this.update_tray();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let audio_settings_path = store.path().with_extension("audio.json");
        let preferences = audio_settings::AudioSettings::load(&audio_settings_path);
        let make_select = |output, selected: &str, window: &mut Window, cx: &mut Context<Self>| {
            let mut names = audio_settings::devices(output)
                .unwrap_or_else(|_| vec![audio_settings::SYSTEM_DEFAULT.into()]);
            // Preserve disconnected choices without silently choosing another device.
            if !names.iter().any(|name| name == selected) {
                names.push(selected.to_owned());
            }
            let index = names.iter().position(|name| name == selected).unwrap_or(0);
            cx.new(|cx| {
                SelectState::new(
                    names
                        .into_iter()
                        .map(SharedString::from)
                        .collect::<Vec<_>>(),
                    Some(IndexPath::new(index)),
                    window,
                    cx,
                )
            })
        };
        let input_device = make_select(false, &preferences.input, window, cx);
        let output_device = make_select(true, &preferences.output, window, cx);
        let label = cx.new(|cx| InputState::new(window, cx).placeholder("Name this span"));
        cx.subscribe(
            &label,
            |_, _, _: &gpui::component::input::InputEvent, cx| cx.notify(),
        )
        .detach();
        for device in [&input_device, &output_device] {
            cx.subscribe(device, |this, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                let preferences = audio_settings::AudioSettings {
                    model: local_audio::ModelChoice::ALL[this.engine],
                    input: this
                        .input_device
                        .read(cx)
                        .selected_value()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| audio_settings::SYSTEM_DEFAULT.into()),
                    output: this
                        .output_device
                        .read(cx)
                        .selected_value()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| audio_settings::SYSTEM_DEFAULT.into()),
                };
                this.status = preferences
                    .save(&this.audio_settings_path)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                cx.notify();
            })
            .detach();
        }
        let recorded_activity = store.recorded_activity().unwrap_or_default();
        let today = chrono::Utc::now()
            .with_timezone(&chrono_tz::Europe::Berlin)
            .date_naive();
        let initial_day = today;
        let mut recorder = Self {
            store,
            samples: samples.clone(),
            span,
            utterances,
            notes,
            scroll: ScrollHandle::new(),
            row_anchors: Rc::new(RefCell::new(BTreeMap::new())),
            last_scroll_offset: Point::default(),
            hover_insert: None,
            time_menu: None,
            composer_time: None,
            range_start: None,
            pending_connection: None,
            minimap_scrolling: false,
            hover_minimap: None,
            sidebar_collapsed: false,
            editing_note: None,
            recording: Default::default(),
            meter_levels: [0.; 5],
            modal_background: None,
            tray: Self::create_tray(),
            navigation: Default::default(),
            pending_start: 0,
            active_span: None,
            start_label: cx.new(|cx| InputState::new(window, cx)),
            start_description: cx.new(|cx| InputState::new(window, cx)),
            all_samples: samples.clone(),
            selected_day: day_for(span.from),
            calendar_today: chrono::Utc::now()
                .with_timezone(&chrono_tz::Europe::Berlin)
                .date_naive(),
            engine: local_audio::ModelChoice::ALL
                .iter()
                .position(|model| *model == preferences.model)
                .unwrap_or(0),
            calendar_month: day_for(span.from).with_day(1).unwrap_or(day_for(span.from)),
            recorded_activity,
            scroll_to_live: true,
            at_live_edge: true,
            entries: Vec::new(),
            drafts: Vec::new(),
            editing_breakpoint: None,
            hovered_breakpoint: None,
            annotation_anchor: None,
            breakpoint_label: cx
                .new(|cx| InputState::new(window, cx).placeholder("Breakpoint label (optional)")),
            composer: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Comment or take a note…")
                    .auto_grow(1, 6)
            }),
            input_device,
            output_device,
            audio_settings_path,
            selection: None,
            drag: None,
            context_range: (0, 0),
            status: String::new(),
            capture_problem: None,
            activity_capture: activity::Capture::default(),
            activities: Vec::new(),
            expanded_activity: None,
            selected_activity_event: None,
            activity_raw: None,
            activity_authorized: None,
            activity_error: None,
            show_capture_details: false,
            label,
            body: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Add a note…")
                    .auto_grow(3, 7)
            }),
            chart_bounds: Rc::new(Cell::new(Bounds::default())),
            track_bounds: Rc::new(Cell::new(Bounds::default())),
        };
        recorder.switch_day(initial_day, cx);
        recorder
    }
    fn switch_day(&mut self, day: chrono::NaiveDate, cx: &mut Context<Self>) {
        use chrono::TimeZone;
        if day
            > chrono::Utc::now()
                .with_timezone(&chrono_tz::Europe::Berlin)
                .date_naive()
        {
            return;
        }
        self.selected_day = day;
        self.hovered_breakpoint = None;
        self.annotation_anchor = None;
        self.calendar_month = day.with_day(1).unwrap_or(day);
        self.utterances = self
            .store
            .utterances()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| day_for(u.time) == day)
            .collect();
        self.samples = self
            .all_samples
            .iter()
            .filter(|s| day_for(s.time) == day)
            .cloned()
            .collect();
        self.notes = self
            .store
            .notes()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| day_for(n.span.from) == day)
            .collect();
        if let Ok(span) = day_extent(&self.samples) {
            self.span = span;
        } else if let Some(date) = day
            .and_hms_opt(8, 0, 0)
            .and_then(|d| chrono_tz::Europe::Berlin.from_local_datetime(&d).single())
        {
            self.span = Span {
                from: date.timestamp_millis(),
                to: date.timestamp_millis() + 8 * 3_600_000,
            };
        }
        self.entries = self
            .store
            .timeline_entries()
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| day_for(entry.time) == day)
            .collect();
        self.scroll_to_live = true;
        self.at_live_edge = true;
        self.include_recorded_extent();
        for entry in &self.entries {
            self.span.from = self
                .span
                .from
                .min(entry.time.div_euclid(3_600_000) * 3_600_000);
            self.span.to = self
                .span
                .to
                .max((entry.time.div_euclid(3_600_000) + 1) * 3_600_000);
        }
        self.span = day_time_span(
            self.selected_day,
            chrono::Utc::now().timestamp_millis().div_euclid(1000) * 1000,
        );
        self.activities = self.store.activity_for_day(day).unwrap_or_default();
        self.expanded_activity = None;
        self.selected_activity_event = None;
        self.activity_raw = None;
        self.row_anchors.borrow_mut().clear();
        self.composer_time = None;
        self.time_menu = None;
        self.hover_insert = None;
        self.pending_connection = None;
        self.range_start = None;
        self.selection = None;
        self.editing_breakpoint = None;
        self.editing_note = None;
        self.navigation.show_day();
        cx.notify();
    }
    fn include_recorded_extent(&mut self) {
        use chrono::TimeZone;
        let midnight = |day: chrono::NaiveDate| {
            day.and_hms_opt(0, 0, 0)
                .and_then(|time| {
                    chrono_tz::Europe::Berlin
                        .from_local_datetime(&time)
                        .single()
                })
                .map(|time| time.timestamp_millis())
        };
        let Some((start, end)) = midnight(self.selected_day)
            .zip(midnight(self.selected_day + chrono::Duration::days(1)))
        else {
            return;
        };
        for (span, _) in &self.recorded_activity {
            if span.to > start && span.from < end {
                let left = span.from.max(start).div_euclid(3_600_000) * 3_600_000;
                let right = (span.to.min(end) + 3_600_000 - 1).div_euclid(3_600_000) * 3_600_000;
                self.span.from = self.span.from.min(left.max(start));
                self.span.to = self.span.to.max(right.min(end));
            }
        }
    }
    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        use chrono::Datelike;
        if self.sidebar_collapsed {
            return div()
                .w(px(48.))
                .h_full()
                .p_2()
                .bg(appearance_color(0xf8fafb, cx))
                .child(
                    Button::new("expand-sidebar")
                        .ghost()
                        .icon(
                            Icon::new(AppIcon::PanelLeftOpen)
                                .text_color(appearance_color(0x71717a, cx)),
                        )
                        .accessibility_label("Show sidebar")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar_collapsed = false;
                            cx.notify();
                        })),
                );
        }
        let month_start = self.calendar_month;
        let next_month = (month_start + chrono::Duration::days(32))
            .with_day(1)
            .unwrap_or(month_start);
        let end = next_month - chrono::Duration::days(1);
        let start = month_start
            - chrono::Duration::days(month_start.weekday().num_days_from_monday() as i64);
        let mut counts = std::collections::BTreeMap::<chrono::NaiveDate, usize>::new();
        for u in self.store.utterances().unwrap_or_default() {
            *counts.entry(day_for(u.time)).or_default() += u.text.split_whitespace().count();
        }
        let peak = counts.values().copied().max().unwrap_or(1).max(1) as f64;
        let mut grid = div().v_flex().gap(px(3.));
        let mut headings = div().h_flex().gap(px(3.));
        for name in ["M", "T", "W", "T", "F", "S", "S"] {
            headings = headings.child(
                div()
                    .w(px(24.))
                    .text_center()
                    .text_xs()
                    .text_color(appearance_color(0x71717a, cx))
                    .child(name),
            );
        }
        grid = grid.child(headings);
        let weeks = (end - start).num_days() / 7 + 1;
        for week in 0..weeks {
            let mut column = div().h_flex().gap(px(3.));
            for weekday in 0..7 {
                let day = start + chrono::Duration::days(week * 7 + weekday);
                if day < month_start || day > end {
                    column = column.child(div().size(px(24.)));
                    continue;
                }
                let words = counts.get(&day).copied().unwrap_or(0);
                let value = words as f64 / peak;
                let color = if words == 0 {
                    if cx.theme().is_dark() {
                        if weekday >= 5 { 0x3b3b4a } else { 0x292e35 }
                    } else {
                        if weekday >= 5 { 0xdcdde8 } else { 0xe8edf1 }
                    }
                } else if value < 0.34 {
                    0x365c4a
                } else if value < 0.67 {
                    0x659d67
                } else {
                    0xb5d46b
                };
                column = column.child(
                    Button::new(("activity-day", (week * 7 + weekday) as usize))
                        .size(px(24.))
                        .p_0()
                        .min_w(px(24.))
                        .max_w(px(24.))
                        .rounded_sm()
                        .shadow_none()
                        .bg(rgb(color))
                        .border_1()
                        .border_color(if day == self.selected_day {
                            appearance_color(0x27272a, cx)
                        } else {
                            rgb(color).into()
                        })
                        .accessibility_label(format!("{} · {} spoken words", day, words))
                        .child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if words > 0 {
                                    rgb(0x243323).into()
                                } else {
                                    appearance_color(0x71717a, cx)
                                })
                                .child(format!("{}.", day.day())),
                        )
                        .disabled(day > self.calendar_today)
                        .opacity(if day > self.calendar_today { 0.28 } else { 1. })
                        .on_click(cx.listener(move |this, _, _, cx| this.switch_day(day, cx))),
                );
            }
            grid = grid.child(column);
        }
        let mut sidebar = div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Activity"),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("previous-month")
                            .ghost()
                            .size(px(24.))
                            .icon(IconName::ChevronLeft)
                            .accessibility_label("Previous month")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let previous = month_start - chrono::Duration::days(1);
                                this.calendar_month = previous.with_day(1).unwrap_or(previous);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .child(month_start.format("%B %Y").to_string()),
                    )
                    .child(
                        Button::new("next-month")
                            .ghost()
                            .size(px(24.))
                            .icon(IconName::ChevronRight)
                            .accessibility_label("Next month")
                            .disabled(
                                month_start
                                    >= self
                                        .calendar_today
                                        .with_day(1)
                                        .unwrap_or(self.calendar_today),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = (month_start + chrono::Duration::days(32))
                                    .with_day(1)
                                    .unwrap_or(month_start);
                                if next <= this.calendar_today {
                                    this.calendar_month = next;
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(grid)
            .child(
                div()
                    .mt_3()
                    .text_sm()
                    .child(self.selected_day.format("%a, %-d %B").to_string()),
            );
        let mut notes = self
            .store
            .notes()
            .unwrap_or_default()
            .into_iter()
            .filter(|note| day_for(note.span.from) == self.selected_day)
            .collect::<Vec<_>>();
        notes.sort_by_key(|note| std::cmp::Reverse(note.span.from));
        let mut tree = div().id("saved-span-tree").min_h_0().v_flex().gap_1();
        if notes.is_empty() {
            tree = tree.child(
                Button::new("whole-day-span")
                    .ghost()
                    .w_full()
                    .justify_start()
                    .label("Whole day")
                    .accessibility_label("Open whole day span")
                    .on_click(cx.listener(|this, _, window, cx| {
                        use chrono::TimeZone;
                        let midnight = |day: chrono::NaiveDate| {
                            day.and_hms_opt(0, 0, 0).and_then(|t| {
                                chrono_tz::Europe::Berlin.from_local_datetime(&t).single()
                            })
                        };
                        if let Some((from, to)) = midnight(this.selected_day)
                            .zip(midnight(this.selected_day + chrono::Duration::days(1)))
                        {
                            this.selection = Some(Span {
                                from: from.timestamp_millis(),
                                to: to.timestamp_millis(),
                            });
                            this.editing_note = None;
                            this.prepare_span(0., window, cx);
                        }
                    })),
            );
        }
        for (index, note) in notes.into_iter().enumerate() {
            let label = if validate_span_label(&note.label).is_ok() {
                note.label.clone()
            } else {
                String::new()
            };
            tree = tree.child(
                Button::new(("saved-span", index))
                    .ghost()
                    .w_full()
                    .justify_start()
                    .small()
                    .label(format!("{}  {}", local(note.span.from, false), label))
                    .accessibility_label(format!("Open span {}", note.label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selection = Some(note.span);
                        this.editing_note = Some(note.clone());
                        this.prepare_span(0., window, cx);
                        this.label.update(cx, |v, cx| {
                            v.set_value(
                                if validate_span_label(&note.label).is_ok() {
                                    note.label.clone()
                                } else {
                                    String::new()
                                },
                                window,
                                cx,
                            )
                        });
                        this.body
                            .update(cx, |v, cx| v.set_value(note.body.clone(), window, cx));
                    })),
            );
        }
        sidebar = sidebar.child(tree);
        let rail = div()
            .w(px(224.))
            .flex_shrink_0()
            .h_full()
            .p_4()
            .border_r_1()
            .border_color(appearance_color(0xe7e9ec, cx))
            .bg(appearance_color(0xf8fafb, cx))
            .v_flex()
            .gap_3()
            .child(
                Button::new("collapse-sidebar")
                    .ghost()
                    .small()
                    .icon(
                        Icon::new(AppIcon::PanelLeftClose)
                            .text_color(appearance_color(0x71717a, cx)),
                    )
                    .accessibility_label("Hide sidebar")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_collapsed = true;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("left-navigation")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(sidebar),
            );
        rail.when(!self.status.is_empty(), |rail| {
            rail.child(
                div()
                    .id("status")
                    .role(Role::Status)
                    .aria_label(self.status.clone())
                    .text_xs()
                    .text_color(appearance_color(0xe66b72, cx))
                    .child(self.status.clone()),
            )
        })
        .child(self.footer(cx))
    }
    fn empty_transcript(&self, title: &str, cx: &App) -> Stateful<Div> {
        div()
            .id("empty-transcript")
            .role(Role::Status)
            .aria_label(title.to_owned())
            .w_full()
            .min_h(px(170.))
            .p_6()
            .v_flex()
            .items_center()
            .justify_center()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(appearance_color(0xe7e9ec, cx))
            .bg(appearance_color(0xf8fafb, cx))
            .child(
                Icon::new(AppIcon::MessageCircle)
                    .size(px(28.))
                    .text_color(appearance_color(0x71717a, cx)),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_owned()),
            )
    }
    fn frame(&self, body: impl IntoElement, cx: &mut Context<Self>) -> Div {
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .size_full()
                    .h_flex()
                    .child(self.sidebar(cx))
                    .child(div().flex_1().min_w_0().h_full().child(body)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.time_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .when_some(
                self.hover_minimap
                    .filter(|_| self.time_menu.is_none() && !self.navigation.is_settings()),
                |frame, (time, y)| {
                    frame.child(
                        Button::new("minimap-plus")
                            .absolute()
                            .left(self.chart_bounds.get().left() - px(26.))
                            .top(px(y - 12.))
                            .size(px(24.))
                            .rounded_full()
                            .border_1()
                            .border_color(appearance_color(0xc7cbd1, cx))
                            .bg(appearance_color(0xffffff, cx))
                            .shadow_sm()
                            .icon(Icon::new(IconName::Plus).size(px(14.)))
                            .accessibility_label(format!("Actions at {}", local(time, false)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.time_menu = Some((
                                    time,
                                    point(this.chart_bounds.get().left() - px(220.), px(y)),
                                ));
                                cx.notify();
                            })),
                    )
                },
            )
            .when_some(
                self.time_menu.filter(|_| !self.navigation.is_settings()),
                |frame, (time, position)| frame.child(self.annotation_menu(time, position, cx)),
            )
            .when_some(self.modal_background.clone(), |frame, background| {
                frame.child(
                    img(background)
                        .object_fit(ObjectFit::Cover)
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                )
            })
    }
    fn open_span_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        self.modal_background = window.render_to_image().ok().map(|image| {
            let small = image::imageops::resize(
                &image,
                (image.width() / 3).max(1),
                (image.height() / 3).max(1),
                image::imageops::FilterType::Triangle,
            );
            let mut blurred = image::imageops::blur(&small, 4.);
            // GPUI's cached images use BGRA; the renderer snapshot is RGBA.
            for pixel in blurred.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            std::sync::Arc::new(RenderImage::new(vec![image::Frame::new(blurred)]))
        });
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let closed = owner.clone();
            let confirmed = owner.clone();
            let form = owner
                .update(cx, |this, cx| this.span_form(cx))
                .unwrap_or_else(|_| div().id("span-form-unavailable"));
            let heading = owner
                .update(cx, |this, _| {
                    if this.navigation.is_starting() {
                        "Start span"
                    } else if this.editing_note.is_some() {
                        "Edit range"
                    } else {
                        "Label range"
                    }
                })
                .unwrap_or("Label range");
            dialog
                .title(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(AppIcon::Tag)
                                .size(px(18.))
                                .text_color(appearance_color(0xe6a63b, cx)),
                        )
                        .child(heading),
                )
                .rounded_2xl()
                .centered()
                .entrance(gpui::component::dialog::DialogEntrance::None)
                .w(px(384.))
                .overlay(true)
                .overlay_closable(true)
                .on_ok(move |event, window, cx| {
                    let _ = confirmed.update(cx, |this, cx| {
                        if this.navigation.is_starting() {
                            this.begin_span(false, window, cx);
                        } else if this.selection.is_some()
                            && validate_span_label(&this.label.read(cx).value()).is_ok()
                        {
                            this.save(event, window, cx);
                        }
                    });
                    // Our transition closes the dialog only after a successful action.
                    false
                })
                .on_close(move |_, _, cx| {
                    let _ = closed.update(cx, |this, cx| {
                        this.modal_background = None;
                        this.pending_connection = None;
                        this.navigation.back();
                        cx.notify();
                    });
                })
                .child(form)
        });
        cx.notify();
    }
    fn time_at(&self, p: Point<Pixels>, track: bool) -> i64 {
        if track {
            let bounds = self.track_bounds.get();
            let anchors = self
                .row_anchors
                .borrow()
                .iter()
                .map(|(time, b)| (*time, b.origin.y.as_f32() + 16.))
                .collect::<Vec<_>>();
            time_at_timeline_y(
                &anchors,
                p.y.as_f32(),
                bounds.top().as_f32(),
                bounds.bottom().as_f32(),
                self.span,
            )
        } else {
            let bounds = self.chart_bounds.get();
            self.span
                .at(((p.y - bounds.origin.y) / bounds.size.height.max(px(1.))) as f64)
        }
    }
    fn row_anchor(&self, time: i64) -> AnyElement {
        let anchors = self.row_anchors.clone();
        canvas(
            move |bounds, _, _| {
                anchors.borrow_mut().insert(time, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }
    fn y_at_time(&self, time: i64) -> Option<f32> {
        let anchors = self.row_anchors.borrow();
        let first = anchors
            .range(..=time)
            .next_back()
            .or_else(|| anchors.first_key_value())?;
        let second = anchors
            .range(time..)
            .next()
            .or_else(|| anchors.last_key_value())?;
        let fraction = if first.0 == second.0 {
            0.
        } else {
            (time - *first.0) as f32 / (*second.0 - *first.0) as f32
        };
        Some(
            first.1.origin.y.as_f32()
                + 16.
                + (second.1.origin.y - first.1.origin.y).as_f32() * fraction.clamp(0., 1.),
        )
    }
    fn scroll_to_time(&mut self, time: i64, cx: &mut Context<Self>) {
        if let Some(y) = self.y_at_time(time) {
            let target = self.scroll.offset().y + self.scroll.bounds().top() + px(24.) - px(y);
            self.scroll.set_offset(point(
                px(0.),
                target.clamp(-self.scroll.max_offset().y, px(0.)),
            ));
            self.scroll_to_live = false;
            cx.notify();
        }
    }
    fn reload_annotations(&mut self) -> anyhow::Result<()> {
        self.row_anchors.borrow_mut().clear();
        self.entries = self
            .store
            .timeline_entries()?
            .into_iter()
            .filter(|entry| day_for(entry.time) == self.selected_day)
            .collect();
        self.notes = self
            .store
            .notes()?
            .into_iter()
            .filter(|note| day_for(note.span.from) == self.selected_day)
            .collect();
        Ok(())
    }
    fn breakpoint_at(&mut self, time: i64, cx: &mut Context<Self>) -> Option<i64> {
        match self
            .store
            .add_timeline_entry(time, "", true)
            .and_then(|id| {
                self.reload_annotations()?;
                Ok(id)
            }) {
            Ok(id) => {
                self.status.clear();
                cx.notify();
                Some(id)
            }
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
                None
            }
        }
    }
    fn annotation_menu(
        &self,
        time: i64,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut menu = div()
            .id("timeline-actions")
            .role(Role::Group)
            .aria_label(format!("Actions at {}", local(time, false)))
            .absolute()
            .left(px((position.x.as_f32() + 14.)
                .min(self.chart_bounds.get().origin.x.as_f32() - 222.)
                .max(8.)))
            .top(px(position
                .y
                .as_f32()
                .min(self.scroll.bounds().bottom().as_f32() - 192.)
                .max(8.)))
            .w(px(216.))
            .p_2()
            .rounded_xl()
            .shadow_lg()
            .border_1()
            .border_color(appearance_color(0xe7e9ec, cx))
            .bg(appearance_color(0xf8fafb, cx))
            .v_flex()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(appearance_color(0x71717a, cx))
                    .child(local(time, true)),
            )
            .child(
                Button::new("comment-at-time")
                    .ghost()
                    .h(px(32.))
                    .w_full()
                    .justify_start()
                    .accessibility_label("Comment")
                    .child(div().w_full().text_left().child("Comment"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.composer_time = Some(time);
                        this.time_menu = None;
                        this.composer.read(cx).focus_handle(cx).focus(window, cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("breakpoint-at-time")
                    .ghost()
                    .h(px(32.))
                    .w_full()
                    .justify_start()
                    .accessibility_label("Breakpoint")
                    .child(div().w_full().text_left().child("Breakpoint"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.time_menu = None;
                        this.breakpoint_at(time, cx);
                    })),
            )
            .child(
                Button::new("range-at-time")
                    .ghost()
                    .h(px(32.))
                    .w_full()
                    .justify_start()
                    .accessibility_label(if self.range_start.is_some() {
                        "End range here"
                    } else {
                        "Start range here"
                    })
                    .child(
                        div()
                            .w_full()
                            .text_left()
                            .child(if self.range_start.is_some() {
                                "End range here"
                            } else {
                                "Start range here"
                            }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.time_menu = None;
                        if let Some(start) = this.range_start {
                            let from = this
                                .entries
                                .iter()
                                .find(|entry| entry.id == start)
                                .map(|entry| entry.time);
                            if let Some(span) = from.and_then(|from| Span::new(from, time).ok()) {
                                if let Some(end) = this.breakpoint_at(time, cx) {
                                    this.range_start = None;
                                    this.pending_connection = Some((start, end));
                                    this.selection = Some(span);
                                    this.editing_note = None;
                                    this.prepare_span(0., window, cx);
                                    this.navigation.refine();
                                    this.open_span_form(window, cx);
                                }
                            } else {
                                this.status = "Choose a different end time".into();
                                cx.notify();
                            }
                        } else {
                            this.range_start = this.breakpoint_at(time, cx);
                        }
                    })),
            );
        if self.range_start.is_some() {
            menu = menu.child(
                Button::new("cancel-range")
                    .ghost()
                    .h(px(32.))
                    .w_full()
                    .justify_start()
                    .accessibility_label("Cancel range")
                    .child(div().w_full().text_left().child("Cancel range"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.range_start = None;
                        this.time_menu = None;
                        cx.notify();
                    })),
            );
        }
        menu
    }
    fn prepare_span(&mut self, _y: f32, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.selection {
            let first = self.utterances.partition_point(|u| u.time < s.from);
            let last = self.utterances.partition_point(|u| u.time <= s.to);
            self.context_range = (
                first.saturating_sub(1),
                (last + 1).min(self.utterances.len()),
            );
            cx.notify();
        }
    }
    fn timeline(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let bounds = self.scroll.bounds();
        let viewport = if bounds.size.height > px(0.) {
            Span::new(
                self.time_at(point(bounds.left(), bounds.top()), true),
                self.time_at(point(bounds.left(), bounds.bottom()), true),
            )
            .ok()
        } else {
            None
        };
        div()
            .id("day-minimap")
            .role(Role::Group)
            .aria_label("Day minimap")
            .w(px(28.))
            .h_full()
            .min_h_0()
            .cursor_crosshair()
            .tooltip(|window, cx| {
                Tooltip::new("Click to scroll · Drag to select · Option-drag to scroll")
                    .build(window, cx)
            })
            .child(MiniMap {
                samples: self.samples.clone(),
                recorded: self
                    .recorded_activity
                    .iter()
                    .filter(|(span, _)| span.to > self.span.from && span.from < self.span.to)
                    .cloned()
                    .collect(),
                span: self.span,
                notes: self.notes.clone(),
                entries: self.entries.clone(),
                utterances: self.utterances.clone(),
                selection: self.selection,
                viewport,
                activity_times: self
                    .activities
                    .iter()
                    .map(|entry| entry.event.time)
                    .collect(),
                bounds: self.chart_bounds.clone(),
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let time = this.time_at(e.position, false);
                    this.time_menu = None;
                    if e.modifiers.alt {
                        this.minimap_scrolling = true;
                        this.scroll_to_time(time, cx);
                    } else {
                        this.drag = Some((RangeGesture::new(time, e.position.y.as_f32()), false));
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.time_menu = Some((this.time_at(e.position, false), e.position));
                    cx.notify();
                }),
            )
    }
    fn page_navigation(&self, cx: &mut Context<Self>) -> Div {
        let nav = div().h_flex().justify_between().child(
            Button::new("page-back")
                .ghost()
                .icon(IconName::ArrowLeft)
                .label("Back")
                .accessibility_label("Back")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.navigation.back();
                    cx.notify();
                })),
        );
        if self.navigation.is_settings() {
            nav
        } else {
            nav.child(
                Button::new("page-settings")
                    .ghost()
                    .label("Settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.navigation.open_settings();
                        cx.notify();
                    })),
            )
        }
    }
    fn begin_span(&mut self, blank: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.navigation.back();
        self.modal_background = None;
        window.close_dialog(cx);
        self.status.clear();
        cx.notify();
    }
    fn toggle_recording(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let starting = !self.recording.is_running() && !self.recording.is_finishing();
        let devices = audio_settings::AudioSettings {
            model: local_audio::ModelChoice::ALL[self.engine],
            input: self
                .input_device
                .read(cx)
                .selected_value()
                .map(ToString::to_string)
                .unwrap_or_else(|| audio_settings::SYSTEM_DEFAULT.into()),
            output: self
                .output_device
                .read(cx)
                .selected_value()
                .map(ToString::to_string)
                .unwrap_or_else(|| audio_settings::SYSTEM_DEFAULT.into()),
        };
        let model = local_audio::ModelChoice::ALL[self.engine];
        let directory = self.store.path().with_file_name("audio");
        match self.recording.toggle(&devices, model, &directory) {
            Ok(()) => {
                if starting {
                    self.capture_problem = None;
                    self.show_capture_details = false;
                    self.activity_error = None;
                    if std::env::var_os("WORKLOG_AUDIO_FIXTURE").is_none() {
                        if let Err(error) = self.activity_capture.start() {
                            self.activity_error = Some(error.to_string());
                        }
                    }
                    self.switch_day(self.calendar_today, cx);
                    self.scroll_to_live = true;
                    self.status = "Starting local transcription…".into();
                } else {
                    self.activity_capture.stop();
                    self.status = "Finishing transcript…".into();
                }
            }
            Err(error) => {
                self.capture_problem =
                    Some(recording::CaptureProblem::from_error(&format!("{error:#}")));
                self.status.clear();
            }
        }
        self.meter_levels = [0.; 5];
        self.update_tray();
        cx.notify();
    }
    fn can_follow_live(&self) -> bool {
        self.selection.is_none()
            && self.at_live_edge
            && self.editing_breakpoint.is_none()
            && self.hovered_breakpoint.is_none()
            && self.time_menu.is_none()
            && self.drag.is_none()
    }
    fn current_annotation_anchor(&self) -> Option<(i64, Pixels)> {
        let id = self.hovered_breakpoint.or(self.editing_breakpoint)?;
        let time = self.entries.iter().find(|entry| entry.id == id)?.time;
        self.row_anchors
            .borrow()
            .get(&time)
            .map(|bounds| (time, bounds.origin.y))
    }
    fn poll_recording(&mut self, cx: &mut Context<Self>) {
        let anchor = self.current_annotation_anchor();
        let before = (
            self.recording.is_running(),
            self.recording.is_finishing(),
            self.recording.is_starting(),
        );
        let mut changed = false;
        for event in self.recording.poll() {
            match event {
                local_audio::AudioEvent::Partial {
                    source,
                    start,
                    delta,
                } => {
                    if let Some(draft) = self.drafts.iter_mut().find(|draft| draft.source == source)
                    {
                        draft.text.push_str(&delta);
                    } else {
                        self.drafts.push(LiveUtterance {
                            source,
                            time: start.timestamp_millis(),
                            text: delta,
                        });
                    }
                    if self.selected_day == self.calendar_today {
                        self.scroll_to_live = self.can_follow_live();
                    }
                    changed = true;
                }
                local_audio::AudioEvent::Transcript(segment) => {
                    self.drafts.retain(|draft| draft.source != segment.source);
                    changed = true;
                    match self.store.ingest_segment(&segment) {
                        Ok(Some(message)) => {
                            if day_for(message.time) == self.selected_day {
                                self.span.from = self
                                    .span
                                    .from
                                    .min(message.time.div_euclid(3_600_000) * 3_600_000);
                                self.span.to = self.span.to.max(
                                    (segment.end.timestamp_millis().div_euclid(3_600_000) + 1)
                                        * 3_600_000,
                                );
                                self.utterances.push(message);
                                self.utterances.sort_by_key(|u| u.time);
                                self.scroll_to_live = self.can_follow_live();
                            }
                            self.recorded_activity =
                                self.store.recorded_activity().unwrap_or_default();
                            self.include_recorded_extent();
                            changed = true;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            self.status = error.to_string();
                            changed = true;
                        }
                    }
                }
                local_audio::AudioEvent::Error(error) => {
                    self.drafts
                        .retain(|draft| !error.starts_with(&format!("{}:", draft.source)));
                    self.capture_problem = Some(recording::CaptureProblem::from_error(&error));
                    self.show_capture_details = false;
                    self.status.clear();
                    changed = true;
                }
                local_audio::AudioEvent::Ready | local_audio::AudioEvent::SourceReady { .. } => {}
                local_audio::AudioEvent::Flushed { .. } => {}
            }
        }
        if self.recording.take_on_air_cue() {
            if self.status == "Starting local transcription…" {
                self.status.clear();
            }
            recording::play_on_air();
            changed = true;
        }
        let after = (
            self.recording.is_running(),
            self.recording.is_finishing(),
            self.recording.is_starting(),
        );
        if before != after {
            if !after.0 {
                self.activity_capture.stop();
            }
            changed = true;
            if !after.0
                && !after.1
                && matches!(
                    self.status.as_str(),
                    "Finishing transcript…" | "Starting local transcription…"
                )
            {
                self.status.clear();
            }
            self.update_tray();
        }
        if !after.0 && !after.1 && !self.drafts.is_empty() {
            self.drafts.clear();
            changed = true;
        }
        if changed {
            self.annotation_anchor = self.annotation_anchor.or(anchor);
            cx.notify();
        }
    }
    fn poll_activity(&mut self, cx: &mut Context<Self>) {
        let anchor = self.current_annotation_anchor();
        let mut changed = false;
        for message in self.activity_capture.poll() {
            match message {
                activity::CaptureMessage::Status(authorized) => {
                    if self.activity_authorized != Some(authorized) {
                        self.activity_authorized = Some(authorized);
                        changed = true;
                    }
                }
                activity::CaptureMessage::Activity(event) => {
                    match self.store.ingest_activity(event) {
                        Ok(Some(entry)) if day_for(entry.event.time) == self.selected_day => {
                            self.activities.push(entry);
                            self.activities
                                .sort_by_key(|entry| (entry.event.time, entry.id));
                            self.scroll_to_live = self.can_follow_live();
                            changed = true;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.activity_error = Some(error.to_string());
                            changed = true;
                        }
                    }
                }
                activity::CaptureMessage::Error(error) => {
                    self.activity_error = Some(error);
                    changed = true;
                }
            }
        }
        if changed {
            self.annotation_anchor = self.annotation_anchor.or(anchor);
            cx.notify();
        }
    }
    fn create_tray() -> Option<tray_icon::TrayIcon> {
        let icon =
            tray_icon::Icon::from_rgba(recording::icon_pixels([0.; 5], false), 24, 24).ok()?;
        tray_icon::TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Worklog · Not recording")
            .build()
            .map_err(|e| eprintln!("Tray unavailable: {e}"))
            .ok()
    }
    fn update_tray(&self) {
        if let Some(tray) = &self.tray {
            if let Ok(icon) = tray_icon::Icon::from_rgba(
                recording::icon_pixels(
                    self.meter_levels,
                    self.recording.button_state() == recording::ButtonState::OnAir,
                ),
                24,
                24,
            ) {
                let _ = tray.set_icon(Some(icon));
            }
            let _ = tray.set_tooltip(Some(if self.recording.is_starting() {
                "Worklog · Starting…"
            } else if self.recording.is_running() {
                "Worklog · Recording"
            } else {
                "Worklog · Not recording"
            }));
        }
    }
    fn recording_meter(&self, _cx: &App) -> Div {
        let mut meter = div()
            .id("recording-meter")
            .role(Role::Meter)
            .aria_label("Microphone level")
            .h(px(24.))
            .h_flex()
            .items_center()
            .gap(px(3.));
        for level in self.meter_levels {
            meter = meter.child(
                div()
                    .w(px(3.))
                    .h(px(3. + level * 19.))
                    .rounded_full()
                    .bg(rgb(0xffffff)),
            );
        }
        div().child(meter)
    }
    fn settings_footer(&self, _screen: &navigation::SettingsScreen, cx: &mut Context<Self>) -> Div {
        div().h_flex().pt_3().child(
            Button::new("settings-done")
                .large()
                .flex_1()
                .h(px(48.))
                .primary()
                .label("Done")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.navigation.back();
                    cx.notify();
                })),
        )
    }
    fn add_conversation_entry(
        &mut self,
        breakpoint: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let time = self
            .composer_time
            .or(self.selection.map(|span| span.to.saturating_sub(1)))
            .unwrap_or_else(|| {
                if self.selected_day == self.calendar_today {
                    chrono::Utc::now().timestamp_millis()
                } else {
                    self.utterances
                        .last()
                        .map(|u| u.time + 1)
                        .unwrap_or(self.span.from)
                }
            });
        match self
            .store
            .add_timeline_entry(time, &self.composer.read(cx).value(), breakpoint)
        {
            Ok(_) => {
                self.entries = self
                    .store
                    .timeline_entries()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|entry| day_for(entry.time) == self.selected_day)
                    .collect();
                self.span.from = self.span.from.min(time.div_euclid(3_600_000) * 3_600_000);
                self.span.to = self
                    .span
                    .to
                    .max((time.div_euclid(3_600_000) + 1) * 3_600_000);
                self.span = day_time_span(
                    self.selected_day,
                    chrono::Utc::now().timestamp_millis().div_euclid(1000) * 1000,
                );
                self.composer
                    .update(cx, |v, cx| v.set_value("", window, cx));
                self.scroll_to_live = self.composer_time.is_none();
                self.composer_time = None;
                self.status.clear();
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }
    fn conversation_composer(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut actions = div()
            .h_flex()
            .justify_center()
            .items_center()
            .gap_2()
            .child(
                Button::new("breakpoint")
                    .h(px(40.))
                    .icon(
                        Icon::new(AppIcon::ScanLine)
                            .size(px(18.))
                            .text_color(rgb(0xe66b72)),
                    )
                    .label("Breakpoint")
                    .accessibility_label("Breakpoint")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_conversation_entry(true, window, cx)
                    })),
            );
        if self.selected_day == self.calendar_today {
            actions = actions.child(self.record_button(cx));
        }
        div()
            .id("conversation-composer")
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .role(Role::Group)
            .aria_label("Conversation notes")
            .v_flex()
            .gap_2()
            .pt_2()
            .flex_shrink_0()
            .when_some(self.composer_time, |composer, time| {
                composer.child(
                    div()
                        .h_flex()
                        .justify_center()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .child(format!("At {}", local(time, false)))
                        .child(
                            Button::new("clear-comment-time")
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .accessibility_label("Clear annotation time")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.composer_time = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(
                !self.status.is_empty()
                    && !matches!(
                        self.status.as_str(),
                        "Starting local transcription…" | "Finishing transcript…"
                    ),
                |composer| {
                    composer.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xe66b72))
                            .child(self.status.clone()),
                    )
                },
            )
            .when_some(self.capture_problem.as_ref(), |composer, problem| {
                composer.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .p_3()
                        .rounded_lg()
                        .bg(appearance_color(0xf8fafb, cx))
                        .v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0xe66b72))
                                .child(problem.summary.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_sm()
                                .line_height(relative(1.4))
                                .child(problem.help.clone()),
                        )
                        .when(self.show_capture_details, |card| {
                            card.child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_xs()
                                    .line_height(relative(1.4))
                                    .child(problem.details.clone()),
                            )
                        })
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .child(
                                    Button::new("capture-error-settings")
                                        .small()
                                        .label("Settings")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.navigation.open_settings();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("capture-error-details")
                                        .small()
                                        .ghost()
                                        .label(if self.show_capture_details {
                                            "Hide details"
                                        } else {
                                            "Show details"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.show_capture_details = !this.show_capture_details;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .when(self.recording.is_running() && self.activity_authorized==Some(false),|composer|composer.child(
                div().w_full().text_xs().child("App switches are being logged. Enable Accessibility in Settings for control events and raw attributes.")))
            .when_some(self.activity_error.clone(),|composer,error|composer.child(div().w_full().text_xs().text_color(rgb(0xe66b72)).child(format!("Computer activity: {error}"))))
            .child(actions)
            .child(
                div()
                    .h_flex()
                    .items_end()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().overflow_hidden().child(
                            Textarea::new(&self.composer).w_full().aria_label("Comment or take a note"),
                        ),
                    )
                    .child(
                        Button::new("send-note")
                            .flex_shrink_0()
                            .ghost()
                            .size(px(40.))
                            .icon(
                                Icon::new(AppIcon::Send)
                                    .size(px(18.))
                                    .text_color(appearance_color(0x9b79c6, cx)),
                            )
                            .accessibility_label("Add comment or note")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_conversation_entry(false, window, cx)
                            })),
                    ),
            )
    }
    fn minute_marker(time: i64, cx: &App) -> Stateful<Div> {
        let label = local(time, false);
        div()
            .id(("minute", time as usize))
            .role(Role::Note)
            .aria_label(format!("Minute {label}"))
            .absolute()
            .left(relative(0.5))
            .ml(px(-10.))
            .top(px(10.))
            .w(px(20.))
            .h(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(div().w(px(8.)).h(px(1.)).bg(appearance_color(0x71717a, cx)))
    }
    fn speech_lane(mine: bool, bubble: impl IntoElement) -> Div {
        div()
            .w(relative(0.5))
            .px_4()
            .h_flex()
            .when(!mine, |lane| lane.justify_end())
            .child(bubble)
    }
    fn bubble_metadata(speaker: &str, time: i64, mine: bool, cx: &App) -> Div {
        div()
            .h_flex()
            .justify_end()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(rgb(if cx.theme().is_dark() {
                if mine { 0xc3d2e3 } else { 0xabb2bc }
            } else {
                0x526273
            }))
            .child(speaker.to_owned())
            .child(local(time, false))
    }
    fn show_activity_event(
        &mut self,
        group: i64,
        entry: &activity::ActivityEntry,
        cx: &mut Context<Self>,
    ) {
        self.expanded_activity = Some(group);
        self.selected_activity_event = Some(entry.id);
        self.activity_raw = Some(
            serde_json::to_string_pretty(entry.event.as_ref())
                .unwrap_or_else(|error| format!("Could not format event: {error}"))
                .into(),
        );
        cx.notify();
    }
    fn activity_row(
        &self,
        group: &activity::ActivityGroup,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = group.id();
        let time = group.time();
        let application = group
            .entries
            .first()
            .map(|entry| entry.event.application.clone())
            .unwrap_or_default();
        let events = group.entries.clone();
        let summary = if events.len() == 1 {
            group.summary()
        } else {
            format!("{} recorded events", events.len())
        };
        let mut bubble = div()
            .max_w_full()
            .px_3()
            .py_2()
            .rounded_lg()
            .v_flex()
            .gap_2()
            .border_1()
            .border_color(if self.selection.is_some_and(|span| span.contains(time)) {
                appearance_color(ME_COLOR, cx)
            } else {
                transparent_black()
            })
            .bg(rgb(if cx.theme().is_dark() {
                0x26343b
            } else {
                0xeaf0f3
            }))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child(Icon::new(AppIcon::MousePointer).size(px(14.)))
                    .child(application.clone()),
            )
            .child(div().text_sm().line_height(relative(1.35)).child(summary))
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .child(
                        Button::new(("activity-open", id as usize))
                            .small()
                            .ghost()
                            .h(px(24.))
                            .label(if self.expanded_activity == Some(id) {
                                "Hide details"
                            } else {
                                "Details"
                            })
                            .accessibility_label(format!("Activity details {id}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.expanded_activity == Some(id) {
                                    this.expanded_activity = None;
                                    this.selected_activity_event = None;
                                    this.activity_raw = None;
                                    cx.notify();
                                } else if let Some(event) = events.last() {
                                    this.show_activity_event(id, event, cx);
                                }
                            })),
                    )
                    .child(Self::bubble_metadata("Me", time, true, cx)),
            );
        if self.expanded_activity == Some(id) {
            let selected = group
                .entries
                .iter()
                .position(|entry| Some(entry.id) == self.selected_activity_event)
                .unwrap_or(0);
            let all = self
                .activities
                .iter()
                .filter(|entry| entry.event.application == application)
                .count();
            let clicks = self
                .activities
                .iter()
                .filter(|entry| {
                    entry.event.application == application
                        && matches!(entry.event.action.as_str(), "click" | "right_click")
                })
                .count();
            let previous = group.entries.get(selected.saturating_sub(1)).cloned();
            let next = group.entries.get(selected + 1).cloned();
            bubble = bubble
                .child(div().text_xs().child(format!(
                    "{} · {all} events · {clicks} clicks",
                    self.selected_day
                )))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(("activity-prev", id as usize))
                                .ghost()
                                .small()
                                .icon(AppIcon::ChevronLeft)
                                .disabled(selected == 0)
                                .accessibility_label("Previous activity event")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(entry) = &previous {
                                        this.show_activity_event(id, entry, cx);
                                    }
                                })),
                        )
                        .child(div().text_xs().child(format!(
                            "Event {} of {}",
                            selected + 1,
                            group.entries.len()
                        )))
                        .child(
                            Button::new(("activity-next", id as usize))
                                .ghost()
                                .small()
                                .icon(AppIcon::ChevronRight)
                                .disabled(next.is_none())
                                .accessibility_label("Next activity event")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(entry) = &next {
                                        this.show_activity_event(id, entry, cx);
                                    }
                                })),
                        ),
                )
                .child(
                    div()
                        .id(("activity-raw", id as usize))
                        .max_h(px(280.))
                        .w_full()
                        .min_w_0()
                        .overflow_y_scroll()
                        .p_2()
                        .rounded_md()
                        .bg(appearance_color(0xffffff, cx))
                        .text_xs()
                        .font_family("Menlo")
                        .child(self.activity_raw.clone().unwrap_or_default()),
                );
        }
        div()
            .id(("activity-row", id as usize))
            .role(Role::Group)
            .aria_label(format!("Computer activity at {}", local(time, false)))
            .relative()
            .w_full()
            .h_flex()
            .justify_end()
            .py_2()
            .child(self.row_anchor(time))
            .child(Self::minute_marker(time, cx))
            .child(Self::speech_lane(true, bubble))
    }
    fn annotations_before(
        &self,
        mut rows: Div,
        entries: &mut std::iter::Peekable<std::vec::IntoIter<TimelineEntry>>,
        activities: &mut std::iter::Peekable<std::vec::IntoIter<activity::ActivityGroup>>,
        until: i64,
        cx: &mut Context<Self>,
    ) -> Div {
        loop {
            let note_time = entries
                .peek()
                .map(|entry| entry.time)
                .filter(|time| *time <= until);
            let activity_time = activities
                .peek()
                .map(|group| group.time())
                .filter(|time| *time <= until);
            match (note_time, activity_time) {
                (None, None) => break,
                (Some(note), Some(activity)) if activity < note => {
                    if let Some(group) = activities.next() {
                        rows = rows.child(self.activity_row(&group, cx));
                    }
                }
                (Some(_), _) => {
                    if let Some(entry) = entries.next() {
                        rows = rows.child(self.timeline_entry(&entry, cx));
                    }
                }
                (None, Some(_)) => {
                    if let Some(group) = activities.next() {
                        rows = rows.child(self.activity_row(&group, cx));
                    }
                }
            }
        }
        rows
    }
    fn timeline_entry(&self, entry: &TimelineEntry, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut item = div()
            .id(("timeline-entry", entry.id as usize))
            .role(Role::Note)
            .aria_label(if entry.breakpoint {
                format!("Breakpoint at {}: {}", local(entry.time, false), entry.body)
            } else {
                format!("Note at {}: {}", local(entry.time, false), entry.body)
            })
            .relative()
            .child(self.row_anchor(entry.time))
            .child(Self::minute_marker(entry.time, cx))
            .w_full()
            .py_2()
            .v_flex()
            .gap_2();
        if entry.breakpoint {
            let id = entry.id;
            let label = entry.label.clone();
            let display_label = if !entry.label.is_empty() {
                entry.label.clone()
            } else if self.range_start == Some(entry.id) {
                "Range start".into()
            } else {
                self.notes
                    .iter()
                    .filter_map(|note| {
                        if note.span.from == entry.time {
                            Some(format!("{} · start", note.label))
                        } else if note.span.to == entry.time {
                            Some(format!("{} · end", note.label))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let right_line = div().h(px(1.)).flex_1().bg(rgb(0xe66b72));
            item = item.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().h(px(1.)).flex_1().bg(rgb(0xe66b72)))
                    .child(
                        Button::new(("edit-breakpoint", id as usize))
                            .h(px(26.))
                            .rounded_full()
                            .px_2()
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(rgb(0xb93e47).into())
                                    .foreground(rgb(0xffffff).into())
                                    .hover(rgb(0xaa303a).into())
                                    .active(rgb(0x94232d).into()),
                            )
                            .bg(rgb(0xb93e47))
                            .text_color(rgb(0xffffff))
                            .max_w(relative(0.8))
                            .child(div().flex_shrink_0().child(local(entry.time, false)))
                            .child(
                                Icon::new(AppIcon::Pencil)
                                    .size(px(12.))
                                    .text_color(rgb(0xffffff)),
                            )
                            .when(!display_label.is_empty(), |button| {
                                button.child(div().min_w_0().truncate().child(display_label))
                            })
                            .accessibility_label(format!(
                                "Edit breakpoint at {}",
                                local(entry.time, true)
                            ))
                            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                                if *over {
                                    this.hovered_breakpoint = Some(id);
                                    this.scroll_to_live = false;
                                } else if this.hovered_breakpoint == Some(id) {
                                    this.hovered_breakpoint = None;
                                }
                                cx.notify();
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.editing_breakpoint = Some(id);
                                this.breakpoint_label
                                    .update(cx, |v, cx| v.set_value(label.clone(), window, cx));
                                cx.notify();
                            })),
                    )
                    .child(right_line),
            );
            if self.editing_breakpoint == Some(id) {
                item = item.child(
                    div().h_flex().justify_center().child(
                        div()
                            .w_full()
                            .max_w(px(460.))
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(
                                Input::new(&self.breakpoint_label).aria_label("Breakpoint label"),
                            ))
                            .child(
                                Button::new(("save-breakpoint", id as usize))
                                    .h(px(32.))
                                    .label("Save")
                                    .accessibility_label("Save breakpoint label")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        match this
                                            .store
                                            .update_breakpoint_label(
                                                id,
                                                &this.breakpoint_label.read(cx).value(),
                                            )
                                            .and_then(|_| this.store.timeline_entries())
                                        {
                                            Ok(entries) => {
                                                this.entries = entries
                                                    .into_iter()
                                                    .filter(|e| {
                                                        day_for(e.time) == this.selected_day
                                                    })
                                                    .collect();
                                                this.editing_breakpoint = None;
                                                this.status.clear();
                                            }
                                            Err(error) => this.status = error.to_string(),
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(("cancel-breakpoint", id as usize))
                                    .ghost()
                                    .h(px(32.))
                                    .label("Cancel")
                                    .accessibility_label("Cancel breakpoint edit")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editing_breakpoint = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
                );
            }
        }
        if !entry.body.is_empty() {
            item = item.child(
                div().h_flex().justify_center().child(
                    div()
                        .max_w(relative(0.6))
                        .px_4()
                        .py_3()
                        .rounded_2xl()
                        .bg(rgb(if cx.theme().is_dark() {
                            0x3b2b51
                        } else {
                            0xeee4f8
                        }))
                        .text_color(rgb(if cx.theme().is_dark() {
                            0xe6d6f5
                        } else {
                            0x50396e
                        }))
                        .v_flex()
                        .gap_1()
                        .text_sm()
                        .line_height(relative(1.45))
                        .child(entry.body.clone())
                        .child(
                            div()
                                .text_xs()
                                .text_right()
                                .opacity(0.75)
                                .child(local(entry.time, false)),
                        ),
                ),
            );
        }
        item
    }
    fn record_button(&self, cx: &mut Context<Self>) -> Button {
        use recording::{ButtonState, RECORD_BUTTON_WIDTH};
        let state = self.recording.button_state();
        Button::new("record-control")
            .h(px(40.))
            .w(px(RECORD_BUTTON_WIDTH))
            .min_w(px(RECORD_BUTTON_WIDTH))
            .max_w(px(RECORD_BUTTON_WIDTH))
            .flex_shrink_0()
            .icon(Icon::new(AppIcon::Mic).text_color(appearance_color(0xe66b72, cx)))
            .loading(state == ButtonState::Starting)
            .when(state == ButtonState::OnAir, |button| {
                button
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(rgb(0xb93e47).into())
                            .foreground(rgb(0xffffff).into())
                            .hover(rgb(0xaa303a).into())
                            .active(rgb(0x94232d).into()),
                    )
                    .bg(rgb(0xb93e47))
                    .text_color(rgb(0xffffff))
                    .icon(Icon::new(AppIcon::Square).text_color(rgb(0xffffff)))
                    .child(self.recording_meter(cx))
            })
            .label(state.label())
            .accessibility_label(match state {
                ButtonState::Idle => "Start recording",
                ButtonState::Starting => "Preparing recording",
                ButtonState::OnAir => "Stop recording",
                ButtonState::Finishing => "Finishing recording",
            })
            .disabled(state == ButtonState::Finishing)
            .on_click(cx.listener(Self::toggle_recording))
    }
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        if let navigation::Navigation::Settings(screen) = &self.navigation {
            return self.settings_footer(screen, cx);
        }
        div()
    }
    fn utility_page(&self, cx: &mut Context<Self>) -> Div {
        let mut page = div().v_flex().gap_3();
        if self.navigation.is_settings() {
            page = page
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Settings"),
                )
                .child(div().mt_3().font_weight(FontWeight::SEMIBOLD).child("Audio devices"))
                .child("Microphone input")
                .child(Select::new(&self.input_device).accessibility_label("Microphone input device"))
                .child("System audio output")
                .child(Select::new(&self.output_device).accessibility_label("System audio output device"))
                .child(
                    Button::new("engine")
                        .label(format!(
                            "Engine: {}",
                            local_audio::ModelChoice::ALL[self.engine].short_name()
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.engine = (this.engine + 1) % local_audio::ModelChoice::ALL.len();
                            let mut settings = audio_settings::AudioSettings::load(&this.audio_settings_path);
                            settings.model = local_audio::ModelChoice::ALL[this.engine];
                            this.status = settings.save(&this.audio_settings_path).err().map(|e|e.to_string()).unwrap_or_default();
                            cx.notify();
                        })),
                )
                .child(div().text_xs().text_color(appearance_color(0x71717a,cx)).child("Nemotron shows live drafts. Whisper and Parakeet finalize short audio chunks."))
                .child(div().mt_3().font_weight(FontWeight::SEMIBOLD).child("Computer activity"))
                .child(match self.activity_authorized {Some(true)=>"Accessibility logging is available",Some(false)=>"Enable Worklog Activity in macOS Accessibility. Status updates automatically.",None=>"Allow Worklog Activity in macOS Accessibility to capture window and control details."})
                .child(Button::new("allow-accessibility").label("Allow Accessibility logging")
                    .on_click(cx.listener(|this,_,_,cx|{this.activity_error=this.activity_capture.request_accessibility().err().map(|error|error.to_string());cx.notify();})))
                .when_some(self.activity_error.clone(), |page,error| page.child(div().text_xs().text_color(rgb(0xe66b72)).child(error)))
                .child(div().text_xs().text_color(appearance_color(0x71717a,cx)).child("Raw OS attributes and values are retained locally. Timeline summaries can be expanded to inspect them."))
                .child(div().mt_3().text_color(appearance_color(0x71717a,cx)).child("Appearance"))
                .child("Follows macOS automatically")
                .child(div().mt_3().text_color(appearance_color(0x71717a,cx)).child("Storage"))
                .child(self.store.path().display().to_string())
                .child(div().mt_3().text_xs().text_color(appearance_color(0x71717a,cx)).child("Audio and transcription stay on this Mac. Transcript and spans are stored separately in SQLite."));
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
        div()
            .size_full()
            .bg(appearance_color(0xffffff, cx))
            .text_color(appearance_color(0x27272a, cx))
            .font_family(".AppleSystemUIFont")
            .text_sm()
            .p(px(24.))
            .v_flex()
            .gap_3()
            .child(self.page_navigation(cx))
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(
                        div()
                            .id("utility-content")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(page),
                    ),
            )
    }

    fn span_form(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let form = div()
            .id("span-form")
            .role(Role::Group)
            .aria_label("Span form")
            .w_full()
            .v_flex()
            .gap_3();
        if self.navigation.is_starting() {
            return form
                .child("Label (optional)")
                .child(Input::new(&self.start_label).aria_label("New span label"))
                .child("Description (optional)")
                .child(Input::new(&self.start_description).aria_label("New span description"))
                .child(
                    Button::new("confirm-start")
                        .primary()
                        .w_full()
                        .label("Start span")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.begin_span(false, window, cx)),
                        ),
                )
                .child(
                    Button::new("start-blank")
                        .ghost()
                        .w_full()
                        .label("Start without details")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.begin_span(true, window, cx)),
                        ),
                );
        }
        let can_save =
            self.selection.is_some() && validate_span_label(&self.label.read(cx).value()).is_ok();
        form.child(self.annotation_editor(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(appearance_color(0xe66b72, cx))
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        Button::new("cancel-span")
                            .ghost()
                            .h(px(40.))
                            .flex_1()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigation.show_day();
                                this.pending_connection = None;
                                this.modal_background = None;
                                window.close_dialog(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save")
                            .primary()
                            .h(px(40.))
                            .icon(AppIcon::Tag)
                            .when(can_save, |button| {
                                button
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(rgb(0x315a82).into())
                                            .foreground(rgb(0xffffff).into())
                                            .hover(rgb(0x284d73).into())
                                            .active(rgb(0x21415f).into()),
                                    )
                                    .bg(rgb(0x315a82))
                                    .text_color(rgb(0xffffff))
                            })
                            .flex_1()
                            .label("Save range")
                            .accessibility_label("Save annotation")
                            .disabled(!can_save)
                            .on_click(cx.listener(Self::save)),
                    ),
            )
            .when(self.editing_note.is_some(), |form| {
                form.child(
                    Button::new("remove-range")
                        .ghost()
                        .label("Remove range connection")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let result = this
                                .editing_note
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("Select a range"))
                                .and_then(|note| this.store.remove_range(note));
                            match result.and_then(|_| this.reload_annotations()) {
                                Ok(()) => {
                                    this.editing_note = None;
                                    this.selection = None;
                                    this.pending_connection = None;
                                    this.modal_background = None;
                                    this.navigation.show_day();
                                    window.close_dialog(cx);
                                    this.status.clear();
                                }
                                Err(error) => this.status = error.to_string(),
                            }
                            cx.notify();
                        })),
                )
            })
    }
    fn span_popover(&self, cx: &mut Context<Self>) -> Button {
        Button::new("footer-label")
            .h(px(36.))
            .icon(Icon::new(AppIcon::Tag).text_color(appearance_color(0xe6a63b, cx)))
            .label(if self.editing_note.is_some() {
                "Edit range"
            } else {
                "Label range"
            })
            .accessibility_label(if self.editing_note.is_some() {
                "Edit range"
            } else {
                "Label range"
            })
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
                this.prepare_span(0., window, cx);
                this.navigation.refine();
                this.open_span_form(window, cx);
            }))
    }
    fn annotation_editor(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w_full()
            .v_flex()
            .gap_4()
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Label"),
                    )
                    .child(Input::new(&self.label).aria_label("Annotation label")),
            )
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(appearance_color(0x71717a, cx))
                            .child("Description (optional)"),
                    )
                    .child(Textarea::new(&self.body).aria_label("Annotation note")),
            )
    }
    fn save(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let span = self
                .selection
                .ok_or_else(|| anyhow::anyhow!("Select a span first"))?;
            let note = Note {
                span,
                label: self.label.read(cx).value().trim().to_owned(),
                body: self.body.read(cx).value().to_string(),
            };
            if let Some(original) = &self.editing_note {
                self.store.update_note(original, &note)?;
            } else if let Some((start, end)) = self.pending_connection {
                self.store.save_connected_span(&note, start, end)?;
            } else {
                self.store.save(&note)?;
            }
            self.pending_connection = None;
            self.reload_annotations()?;
            self.editing_note = None;
            self.notes = self
                .store
                .notes()?
                .into_iter()
                .filter(|n| day_for(n.span.from) == self.selected_day)
                .collect();
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.navigation.show_day();
                self.modal_background = None;
                window.close_dialog(cx);
                self.status = String::new();
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
        if let Some((time, previous_y)) = self.annotation_anchor.take() {
            let recorder = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = recorder.update(cx, |this, cx| {
                    let new_y = this
                        .row_anchors
                        .borrow()
                        .get(&time)
                        .map(|bounds| bounds.origin.y);
                    if let Some(new_y) = new_y {
                        let delta = previous_y - new_y;
                        if delta.as_f32().abs() > 0.5 {
                            let offset = this.scroll.offset();
                            this.scroll.set_offset(point(
                                offset.x,
                                (offset.y + delta).clamp(-this.scroll.max_offset().y, px(0.)),
                            ));
                            this.last_scroll_offset = this.scroll.offset();
                            cx.notify();
                        }
                    }
                });
            });
        }
        if self.scroll_to_live {
            self.scroll_to_live = false;
            let scroll = self.scroll.clone();
            window.on_next_frame(move |window, _| {
                scroll.scroll_to_bottom();
                window.refresh();
            });
        }
        if self.navigation.is_settings() {
            let body = self.utility_page(cx);
            return self.frame(body, cx).into_any_element();
        }
        let chart = self.timeline(cx);
        let track_cell = self.track_bounds.clone();
        let connections = self.notes.clone();
        let anchors = self.row_anchors.clone();
        let mut track = div()
            .id("transcript-track")
            .role(Role::Group)
            .aria_label("Transcript range selector")
            .absolute()
            .left(relative(0.5))
            .ml(px(-10.))
            .top_0()
            .bottom_0()
            .w(px(20.))
            .cursor_crosshair()
            .child(
                canvas(
                    move |bounds, _, _| {
                        track_cell.set(bounds);
                    },
                    move |bounds, _, window, cx| {
                        window.paint_quad(fill(
                            Bounds::new(
                                bounds.origin + point(px(9.), px(0.)),
                                size(px(1.), bounds.size.height),
                            ),
                            appearance_color(0xd4d4d8, cx),
                        ));
                        let points = anchors
                            .borrow()
                            .iter()
                            .map(|(time, b)| (*time, b.origin.y.as_f32() + 16.))
                            .collect::<Vec<_>>();
                        for note in &connections {
                            if let Some((from, to)) = timeline_y_at(&points, note.span.from)
                                .zip(timeline_y_at(&points, note.span.to))
                            {
                                let top = from.max(bounds.top().as_f32());
                                let end = to.min(bounds.bottom().as_f32());
                                if end > top {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(bounds.left() + px(5.), px(top)),
                                            size(px(3.), px(end - top)),
                                        ),
                                        appearance_color(0x9b79c6, cx),
                                    ));
                                }
                            }
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.drag.is_none() && this.time_menu.is_none() {
                    this.hover_insert = Some((
                        this.time_at(event.position, true),
                        event.position.y.as_f32(),
                    ));
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if !*over {
                    this.hover_insert = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let time = this.time_at(event.position, true);
                    this.time_menu = None;
                    this.drag = Some((RangeGesture::new(time, event.position.y.as_f32()), true));
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        if let Some((time, y)) = self
            .hover_insert
            .filter(|_| self.time_menu.is_none() && self.drag.is_none())
        {
            track = track.child(
                Button::new("timeline-plus")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .absolute()
                    .left(px(-2.))
                    .top(px(y - self.track_bounds.get().top().as_f32() - 12.))
                    .size(px(24.))
                    .rounded_full()
                    .border_1()
                    .border_color(appearance_color(0xc7cbd1, cx))
                    .bg(appearance_color(0xffffff, cx))
                    .shadow_sm()
                    .icon(Icon::new(IconName::Plus).size(px(14.)))
                    .accessibility_label(format!("Actions at {}", local(time, false)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.time_menu = Some((time, point(this.track_bounds.get().left(), px(y))));
                        cx.notify();
                    })),
            );
        }
        self.row_anchors.borrow_mut().clear();
        let mut messages = div().v_flex().w_full().gap_2();
        let mut previous_speaker = String::new();
        let mut previous_minute = None;
        let mut entries = self.entries.clone().into_iter().peekable();
        let mut activity_groups = activity::groups(&self.activities).into_iter().peekable();
        let empty_range = self.drag.is_none()
            && self.selection.is_some_and(|span| {
                !self.utterances.iter().any(|u| span.contains(u.time))
                    && !self.entries.iter().any(|entry| span.contains(entry.time))
                    && !self.drafts.iter().any(|draft| span.contains(draft.time))
                    && !self
                        .activities
                        .iter()
                        .any(|entry| span.contains(entry.event.time))
            });
        if (self.utterances.is_empty()
            && self.entries.is_empty()
            && self.activities.is_empty()
            && self.drafts.is_empty())
            || empty_range
        {
            let mut empty = self.empty_transcript(
                if self.recording.is_starting() {
                    "Preparing local transcription…"
                } else if self.recording.is_running() {
                    "Listening…"
                } else if self.recording.is_finishing() {
                    "Finishing transcript…"
                } else if empty_range {
                    "No messages in this span"
                } else {
                    "No transcript for this day"
                },
                cx,
            );
            if !empty_range
                && self.selected_day == self.calendar_today
                && !self.recording.is_running()
                && !self.recording.is_finishing()
            {
                empty = empty.child(
                    Button::new("empty-start-recording")
                        .primary()
                        .mt_3()
                        .label("Start recording")
                        .accessibility_label("Start recording in transcript")
                        .icon(Icon::new(AppIcon::Mic))
                        .on_click(cx.listener(Self::toggle_recording)),
                );
            }
            messages = messages.child(empty);
        }
        for u in ordered_transcript_bubbles(&self.utterances, &self.drafts)
            .into_iter()
            .filter(|u| day_for(u.time) == self.selected_day)
            .take(if empty_range { 0 } else { usize::MAX })
        {
            messages =
                self.annotations_before(messages, &mut entries, &mut activity_groups, u.time, cx);
            let marked = self.selection.is_some_and(|s| s.contains(u.time));
            let id = u.utterance_id;
            let streaming = id.is_none();
            let mine = u.speaker.eq_ignore_ascii_case("Me");
            let same_speaker = previous_speaker == u.speaker;
            previous_speaker = u.speaker.clone();
            let background = if cx.theme().is_dark() {
                if mine { 0x284769 } else { 0x2a2d33 }
            } else if mine {
                0xdceaf7
            } else {
                0xf0f2f4
            };
            let mut bubble = div()
                .id(match id {
                    Some(id) => ElementId::from(("utterance", id as usize)),
                    None => {
                        ElementId::from(SharedString::from(format!("live-utterance-{}", u.source)))
                    }
                })
                .role(Role::Group)
                .aria_label(if streaming {
                    format!("Live transcript {}: {}", u.speaker, u.text)
                } else {
                    format!("{}, {}: {}", u.speaker, local(u.time, false), u.text)
                })
                .max_w_full()
                .px_4()
                .py_3()
                .rounded_2xl()
                .when(mine, |bubble| bubble.rounded_bl_sm())
                .when(!mine, |bubble| bubble.rounded_br_sm())
                .border_1()
                .border_color(if marked {
                    rgb(0x8dbdeb).into()
                } else {
                    transparent_black()
                })
                .bg(rgb(background))
                .when(streaming, |bubble| {
                    bubble
                        .text_color(appearance_color(0x71717a, cx))
                        .opacity(0.65)
                })
                .text_size(px(15.))
                .line_height(relative(1.45))
                .v_flex()
                .gap_1()
                .child(u.text)
                .child(Self::bubble_metadata(&u.speaker, u.time, mine, cx));
            {
                if let Some(id) = id
                    && let Ok(replies) = self.store.replies(id)
                {
                    for (reply_index, reply) in replies.into_iter().enumerate() {
                        bubble = bubble.child(
                            div()
                                .id(("reply-body", reply_index))
                                .role(Role::Note)
                                .aria_label(reply.clone())
                                .mt_2()
                                .ml_4()
                                .pl_3()
                                .py_2()
                                .border_l_2()
                                .border_color(appearance_color(0x9b79c6, cx))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(appearance_color(0x71717a, cx))
                                        .child("Comment"),
                                )
                                .child(reply),
                        );
                    }
                }
            }
            let minute = u.time.div_euclid(60_000) * 60_000;
            let show_minute = previous_minute != Some(minute);
            previous_minute = Some(minute);
            messages = messages.child(
                div()
                    .relative()
                    .w_full()
                    .h_flex()
                    .child(self.row_anchor(u.time))
                    .when(show_minute, |row| {
                        row.child(Self::minute_marker(minute, cx))
                    })
                    .when(mine, |row| row.justify_end())
                    .when(!same_speaker, |row| row.mt_2())
                    .child(Self::speech_lane(mine, bubble)),
            );
        }
        if !empty_range {
            messages =
                self.annotations_before(messages, &mut entries, &mut activity_groups, i64::MAX, cx);
        }
        let content = div()
            .id("recorder")
            .relative()
            .size_full()
            .bg(appearance_color(0xffffff, cx))
            .text_color(appearance_color(0x27272a, cx))
            .font_family(".AppleSystemUIFont")
            .text_sm()
            .p(px(24.))
            .v_flex()
            .gap_3()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let Some((mut gesture, track)) = this.drag {
                    let end = this.time_at(e.position, track);
                    let selection = gesture.update(end, e.position.y.as_f32());
                    if gesture.is_marking() {
                        this.selection = selection;
                        this.pending_connection = None;
                        this.range_start = None;
                        this.editing_note = None;
                    }
                    this.drag = Some((gesture, track));
                    cx.notify();
                } else if this.minimap_scrolling {
                    let time = this.time_at(e.position, false);
                    this.scroll_to_time(time, cx);
                } else {
                    let b = this.chart_bounds.get();
                    let hover = if e.position.x >= b.left() - px(28.)
                        && e.position.x <= b.right()
                        && e.position.y >= b.top()
                        && e.position.y <= b.bottom()
                    {
                        Some((this.time_at(e.position, false), e.position.y.as_f32()))
                    } else {
                        None
                    };
                    if hover != this.hover_minimap {
                        this.hover_minimap = hover;
                        cx.notify();
                    }
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, e: &MouseUpEvent, window, cx| {
                this.minimap_scrolling = false;
                if let Some((mut gesture, track)) = this.drag.take() {
                    let time = this.time_at(e.position, track);
                    let selection = gesture.update(time, e.position.y.as_f32());
                    if gesture.is_marking() {
                        this.selection = selection;
                        this.prepare_span(e.position.y.as_f32(), window, cx);
                    } else if track {
                        this.time_menu = Some((time, e.position));
                    } else {
                        this.scroll_to_time(time, cx);
                    }
                    cx.notify();
                }
            }))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                        "{} · Europe/Berlin",
                        self.selected_day.format("%-d %B %Y")
                    )))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .when(self.selected_day == self.calendar_today, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(appearance_color(0x71717a, cx))
                                        .child(
                                            chrono::DateTime::from_timestamp_millis(self.span.to)
                                                .unwrap_or_default()
                                                .with_timezone(&chrono_tz::Europe::Berlin)
                                                .format("%H:%M:%S")
                                                .to_string(),
                                        ),
                                )
                            })
                            .when(self.selection.is_some(), |row| {
                                row.child(self.span_popover(cx))
                            })
                            .child(
                                Button::new("settings")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Settings)
                                    .accessibility_label("Settings")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.time_menu = None;
                                        this.navigation.open_settings();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .justify_center()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .max_w(px(1280.))
                            .min_h_0()
                            .v_flex()
                            .gap_3()
                            .child(
                                div()
                                    .id("transcript")
                                    .role(Role::Group)
                                    .aria_label("Conversation transcript")
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll)
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div()
                                            .relative()
                                            .w_full()
                                            .min_h_full()
                                            .v_flex()
                                            .justify_end()
                                            .child(track)
                                            .child(messages.flex_shrink_0()),
                                    ),
                            )
                            .child(div().relative().h_flex().justify_center().when(
                                (!self.utterances.is_empty()
                                    || !self.entries.is_empty()
                                    || !self.activities.is_empty())
                                    && !empty_range
                                    && (self.selection.is_some() || !self.at_live_edge),
                                |row| {
                                    row.child(
                                        Button::new("live")
                                            .primary()
                                            .small()
                                            .label("↓ Back to live")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.selection = None;
                                                this.editing_note = None;
                                                this.navigation.show_day();
                                                this.scroll_to_live = true;
                                                this.at_live_edge = true;
                                                this.status.clear();
                                                cx.notify();
                                            })),
                                    )
                                },
                            ))
                            .child(self.conversation_composer(cx)),
                    )
                    .child(
                        div()
                            .w(px(28.))
                            .h_full()
                            .flex_shrink_0()
                            .v_flex()
                            .gap_2()
                            .child(div().flex_1().min_h_0().child(chart))
                            .child(
                                div().h_flex().justify_center().gap(px(2.)).children(
                                    [
                                        ("Me", ME_COLOR),
                                        ("Them", THEM_COLOR),
                                        ("Both", BOTH_COLOR),
                                        ("Computer activity", 0x62b3a1),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(
                                        |(index, (label, color))| {
                                            div()
                                                .id(("minimap-color", index))
                                                .size(px(5.))
                                                .rounded_full()
                                                .bg(appearance_color(color, cx))
                                                .tooltip(move |window, cx| {
                                                    Tooltip::new(label).build(window, cx)
                                                })
                                        },
                                    ),
                                ),
                            ),
                    ),
            );
        let _ = window;
        self.frame(content, cx).into_any_element()
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
    if cli::run(std::env::args_os())? {
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
            Theme::sync_system_appearance(None, cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(960.), px(830.)),
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
