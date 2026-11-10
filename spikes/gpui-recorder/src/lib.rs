#![deny(clippy::unwrap_used, clippy::expect_used)]
pub mod activity;
pub mod audio_settings;
pub mod cli;
#[path = "../../../src/echo.rs"]
pub mod echo;
pub mod navigation;
pub mod reader;
pub mod recording;
// Reuse the local capture/transcription core, without depending on the legacy UI.
#[path = "../../../src/audio.rs"]
pub mod local_audio;
#[path = "../../../src/model.rs"]
pub mod model;
use anyhow::{Result, bail};
use chrono::{DateTime, TimeZone};
use chrono_tz::Europe::Berlin;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// Ask the packaged sqlite3def to plan against the embedded desired schema,
/// then execute the plan atomically on our own SQLite connection.
pub fn validate_span_label(label: &str) -> Result<()> {
    let normalized = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "untitled span" | "untitles span") {
        bail!("Enter a label");
    }
    Ok(())
}

pub fn reconcile_schema(path: &Path, schema: &str) -> Result<()> {
    use anyhow::Context;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    // Older spike databases used bare identifiers that sqldef treats as keywords.
    // SQLite can quote the existing names without renaming or copying any data.
    let normalize = connection.unchecked_transaction()?;
    for (table, column) in [
        ("utterances", "time"),
        ("utterances", "text"),
        ("notes", "start"),
        ("notes", "end"),
    ] {
        let sql: Option<String> = normalize
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .optional()?;
        if sql.is_some_and(|sql| !sql.contains(&format!("\"{column}\""))) {
            normalize.execute_batch(&format!(
                "ALTER TABLE \"{table}\" RENAME COLUMN \"{column}\" TO \"{column}\";"
            ))?;
        }
    }
    normalize.commit()?;
    let configured = std::env::var_os("WORKLOG_SQLITE3DEF")
        .map(std::path::PathBuf::from)
        .or_else(|| option_env!("WORKLOG_SQLITE3DEF").map(std::path::PathBuf::from));
    let executable = configured.unwrap_or_else(|| {
        [
            "/opt/homebrew/opt/sqlite3def/bin/sqlite3def",
            "/usr/local/opt/sqlite3def/bin/sqlite3def",
        ]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.is_file())
        .unwrap_or_else(|| "sqlite3def".into())
    });
    let mut child = Command::new(executable)
        .arg(path)
        .args(["--dry-run", "--file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("sqlite3def is required: brew install sqldef/sqldef/sqlite3def")?;
    child
        .stdin
        .take()
        .context("Schema input unavailable")?
        .write_all(schema.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "Schema planning failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let plan = String::from_utf8(output.stdout)?;
    let plan = plan.trim();
    let plan = plan.strip_prefix("-- dry run --").unwrap_or(plan).trim();
    let plan = if let Some(body) = plan.strip_prefix("BEGIN;") {
        body.trim()
            .strip_suffix("COMMIT;")
            .ok_or_else(|| anyhow::anyhow!("Unexpected sqlite3def transaction plan"))?
            .trim()
    } else {
        plan
    };
    let tx = connection.unchecked_transaction()?;
    tx.execute_batch(plan)
        .context("Applying the sqlite3def schema plan")?;
    let violations: i64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if violations != 0 {
        bail!("Schema change violates foreign keys");
    }
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: i64,
    pub to: i64,
}
#[derive(Clone, Copy, Debug)]
pub struct RangeGesture {
    start: i64,
    origin: f32,
    marked: bool,
}
impl RangeGesture {
    pub fn new(start: i64, origin: f32) -> Self {
        Self {
            start,
            origin,
            marked: false,
        }
    }
    pub fn update(&mut self, time: i64, position: f32) -> Option<Span> {
        self.marked |= (position - self.origin).abs() >= 4.;
        if self.marked {
            Span::new(self.start, time).ok()
        } else {
            None
        }
    }
    pub fn is_marking(&self) -> bool {
        self.marked
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    pub id: i64,
    pub time: i64,
    pub utterance_id: Option<i64>,
    pub body: String,
    pub breakpoint: bool,
    pub label: String,
}
pub fn live_chart_span(span: Span, day: chrono::NaiveDate, now: i64) -> Span {
    let Some(time) = DateTime::from_timestamp_millis(now) else {
        return span;
    };
    if day != time.with_timezone(&Berlin).date_naive() {
        return span;
    }
    let midnight = day
        .and_hms_opt(0, 0, 0)
        .and_then(|date| Berlin.from_local_datetime(&date).single())
        .map(|date| date.timestamp_millis())
        .unwrap_or(now - 1);
    Span {
        from: if span.from >= now {
            midnight.min(now.saturating_sub(1))
        } else {
            span.from
        },
        to: now,
    }
}
pub fn day_time_span(day: chrono::NaiveDate, now: i64) -> Span {
    let midnight = |date: chrono::NaiveDate| {
        date.and_hms_opt(0, 0, 0)
            .and_then(|date| Berlin.from_local_datetime(&date).single())
            .map(|time| time.timestamp_millis())
    };
    let from = midnight(day).unwrap_or(now);
    let next = day
        .succ_opt()
        .and_then(midnight)
        .unwrap_or(from + 86_400_000);
    let today =
        DateTime::from_timestamp_millis(now).map(|time| time.with_timezone(&Berlin).date_naive());
    Span {
        from,
        to: if today == Some(day) {
            now.max(from + 1)
        } else {
            next
        },
    }
}
pub fn time_at_timeline_y(
    anchors: &[(i64, f32)],
    y: f32,
    top: f32,
    bottom: f32,
    span: Span,
) -> i64 {
    let mut points = anchors
        .iter()
        .copied()
        .filter(|(_, p)| p.is_finite() && *p >= top && *p <= bottom)
        .collect::<Vec<_>>();
    points.push((span.from, top));
    points.push((span.to, bottom));
    points.sort_by(|a, b| a.1.total_cmp(&b.1));
    for pair in points.windows(2) {
        let ((a, ay), (b, by)) = (pair[0], pair[1]);
        if by > ay && y >= ay && y <= by {
            return (a + ((b - a) as f64 * ((y - ay) / (by - ay)) as f64).round() as i64)
                .clamp(span.from, span.to);
        }
    }
    span.at(((y - top) / (bottom - top).max(1.)) as f64)
}
pub fn timeline_y_at(anchors: &[(i64, f32)], time: i64) -> Option<f32> {
    let mut points = anchors.to_vec();
    points.sort_by_key(|point| point.0);
    if time <= points.first()?.0 {
        return Some(points[0].1);
    }
    for pair in points.windows(2) {
        let ((a, ay), (b, by)) = (pair[0], pair[1]);
        if time >= a && time <= b && b > a {
            return Some(ay + (by - ay) * (time - a) as f32 / (b - a) as f32);
        }
    }
    points.last().map(|point| point.1)
}
impl Span {
    pub fn new(a: i64, b: i64) -> Result<Self> {
        if a == b {
            bail!("Select a non-empty time range");
        }
        Ok(Self {
            from: a.min(b),
            to: a.max(b),
        })
    }
    pub fn contains(&self, time: i64) -> bool {
        time >= self.from && time <= self.to
    }
    pub fn at(&self, fraction: f64) -> i64 {
        self.from + ((self.to - self.from) as f64 * fraction.clamp(0.0, 1.0)).round() as i64
    }
}
#[derive(Clone, Debug)]
pub struct Sample {
    pub time: i64,
    pub me: f64,
    pub them: f64,
}
pub fn extent(samples: &[Sample]) -> Result<Span> {
    let active: Vec<_> = samples
        .iter()
        .filter(|s| s.me > 0.0 || s.them > 0.0)
        .map(|s| s.time)
        .collect();
    match (active.iter().min(), active.iter().max()) {
        (Some(a), Some(b)) => Span::new(*a, *b),
        _ => bail!("No voice activity"),
    }
}
pub fn local(time: i64, precise: bool) -> String {
    DateTime::from_timestamp_millis(time)
        .map(|d| {
            d.with_timezone(&Berlin)
                .format(if precise { "%H:%M:%S%.3f" } else { "%H:%M" })
                .to_string()
        })
        .unwrap_or_default()
}
pub fn parse_local(text: &str) -> Result<i64> {
    let parsed = chrono::NaiveDateTime::parse_from_str(
        &format!("2026-11-09 {text}"),
        "%Y-%m-%d %H:%M:%S%.f",
    )?;
    Ok(Berlin
        .from_local_datetime(&parsed)
        .single()
        .ok_or_else(|| anyhow::anyhow!("Ambiguous local time"))?
        .timestamp_millis())
}
pub fn hours(span: Span) -> Vec<i64> {
    let first = (span.from.div_euclid(3_600_000) + 1) * 3_600_000;
    (first..=span.to).step_by(3_600_000).collect()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub span: Span,
    pub label: String,
    pub body: String,
}
#[derive(Clone, Debug)]
pub struct Utterance {
    pub id: i64,
    pub time: i64,
    pub speaker: String,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct LiveUtterance {
    pub source: String,
    pub time: i64,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct TranscriptBubble {
    pub utterance_id: Option<i64>,
    pub source: String,
    pub time: i64,
    pub speaker: String,
    pub text: String,
}
/// Streaming text occupies its chronological position from its first delta.
/// Finalization changes its contents/style, rather than moving it past notes.
pub fn ordered_transcript_bubbles(
    utterances: &[Utterance],
    drafts: &[LiveUtterance],
) -> Vec<TranscriptBubble> {
    let mut rows: Vec<_> = utterances
        .iter()
        .map(|u| TranscriptBubble {
            utterance_id: Some(u.id),
            source: if u.speaker.eq_ignore_ascii_case("Me") {
                "microphone"
            } else {
                "system audio"
            }
            .into(),
            time: u.time,
            speaker: u.speaker.clone(),
            text: u.text.clone(),
        })
        .chain(drafts.iter().map(|draft| {
            TranscriptBubble {
                utterance_id: None,
                source: draft.source.clone(),
                time: draft.time,
                speaker: if draft.source == "microphone" {
                    "Me"
                } else {
                    "Them"
                }
                .into(),
                text: draft.text.clone(),
            }
        }))
        .collect();
    rows.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.source.cmp(&b.source)));
    rows
}
pub struct Store {
    connection: Connection,
    path: std::path::PathBuf,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        reconcile_schema(path, include_str!("../schema.sql"))?;
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        let store = Self {
            connection,
            path: path.to_path_buf(),
        };
        store.link_legacy_spans()?;
        store.initialize_reader()?;
        Ok(store)
    }
    fn initialize_reader(&self) -> Result<()> {
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO reader_state(id,database_id) VALUES(1,?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        let initialized: bool = tx.query_row(
            "SELECT initialized FROM reader_state WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        if !initialized {
            tx.execute_batch(include_str!("../reader_backfill.sql"))?;
            tx.execute("UPDATE reader_state SET initialized=1 WHERE id=1", [])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn import_transcript(&mut self, path: &Path) -> Result<()> {
        let values: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let messages = values
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Expected transcript array"))?;
        if messages.is_empty() {
            bail!("Audio fixture produced no messages");
        }
        let tx = self.connection.transaction()?;
        for (id, message) in messages.iter().enumerate() {
            let time = message["time"]
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Missing timestamp"))?;
            let text = message["text"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing text"))?;
            tx.execute(
                "INSERT OR IGNORE INTO utterances VALUES(?1,?2,'Me',?3)",
                params![id as i64, time, text],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn seed(&mut self) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for (id, (time, speaker, text)) in [
            (
                "14:30:04.120",
                "Me",
                "Let's look at the day as one continuous timeline.",
            ),
            (
                "14:31:12.580",
                "Them",
                "Keep the gaps visible, so the time still makes sense.",
            ),
            (
                "14:32:18.240",
                "Me",
                "We can mark this passage without hiding the surrounding conversation.",
            ),
            (
                "14:33:22.910",
                "Them",
                "A short note belongs right here beside the passage.",
            ),
            (
                "15:26:03.100",
                "Me",
                "And Back to live brings us to the latest text.",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            transaction.execute(
                "INSERT OR IGNORE INTO utterances VALUES(?1,?2,?3,?4)",
                params![id as i64, parse_local(time)?, speaker, text],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn utterances(&self) -> Result<Vec<Utterance>> {
        Ok(self
            .connection
            .prepare("SELECT id,time,speaker,text FROM utterances ORDER BY time")?
            .query_map([], |r| {
                Ok(Utterance {
                    id: r.get(0)?,
                    time: r.get(1)?,
                    speaker: r.get(2)?,
                    text: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn ingest_segment(&self, segment: &model::Segment) -> Result<Option<Utterance>> {
        let from = segment.start.timestamp_millis();
        let to = segment.end.timestamp_millis();
        if segment.text.trim().is_empty() {
            return Ok(None);
        }
        if to <= from {
            bail!("Transcript end must be after its start");
        }
        let tx = self.connection.unchecked_transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM transcript_audio WHERE id=?1)",
            [segment.id.to_string()],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(None);
        }
        let speaker = if segment.source == "microphone" {
            "Me"
        } else {
            "Them"
        };
        tx.execute(
            "INSERT INTO utterances(time,speaker,text) VALUES(?1,?2,?3)",
            params![from, speaker, segment.text.trim()],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            r#"INSERT INTO transcript_audio(id,utterance_id,"end",source) VALUES(?1,?2,?3,?4)"#,
            params![segment.id.to_string(), id, to, segment.source],
        )?;
        tx.execute("INSERT OR IGNORE INTO span_messages SELECT id,?1 FROM notes WHERE ?2 BETWEEN start AND end", params![id,from])?;
        tx.commit()?;
        Ok(Some(Utterance {
            id,
            time: from,
            speaker: speaker.into(),
            text: segment.text.trim().into(),
        }))
    }
    pub fn recorded_activity(&self) -> Result<Vec<(Span, u8)>> {
        let mut statement = self.connection.prepare(r#"SELECT u.time,a."end",u.speaker FROM transcript_audio a JOIN utterances u ON u.id=a.utterance_id ORDER BY u.time"#)?;
        let spans = statement
            .query_map([], |r| {
                Ok((
                    Span {
                        from: r.get(0)?,
                        to: r.get(1)?,
                    },
                    if r.get::<_, String>(2)? == "Me" { 1 } else { 2 },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(combine_activity(&spans))
    }
    fn link_legacy_spans(&self) -> Result<()> {
        let tx = self.connection.unchecked_transaction()?;
        let missing=tx.prepare("SELECT id,start,end FROM notes WHERE id NOT IN (SELECT span_id FROM span_breakpoints)")?
            .query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, from, to) in missing {
            tx.execute(
                "INSERT INTO timeline_entries(time,body,breakpoint) VALUES(?1,'',1)",
                [from],
            )?;
            let start = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO timeline_entries(time,body,breakpoint) VALUES(?1,'',1)",
                [to],
            )?;
            tx.execute(
                "INSERT INTO span_breakpoints VALUES(?1,?2,?3)",
                params![id, start, tx.last_insert_rowid()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn save(&self, note: &Note) -> Result<()> {
        self.save_range(note, None)
    }
    pub fn save_connected_span(&self, note: &Note, start: i64, end: i64) -> Result<()> {
        self.save_range(note, Some((start, end)))
    }
    fn save_range(&self, note: &Note, endpoints: Option<(i64, i64)>) -> Result<()> {
        validate_span_label(&note.label)?;
        if note.span.to <= note.span.from {
            bail!("To must be after From");
        }
        let tx = self.connection.unchecked_transaction()?;
        let (start, end) = if let Some((a, b)) = endpoints {
            let from: i64 = tx.query_row(
                "SELECT time FROM timeline_entries WHERE id=?1 AND breakpoint=1",
                [a],
                |r| r.get(0),
            )?;
            let to: i64 = tx.query_row(
                "SELECT time FROM timeline_entries WHERE id=?1 AND breakpoint=1",
                [b],
                |r| r.get(0),
            )?;
            let original = Span::new(from, to)?;
            let (start, end) = if from < to { (a, b) } else { (b, a) };
            if original != note.span {
                let linked:i64=tx.query_row("SELECT count(*) FROM span_breakpoints WHERE start_id IN (?1,?2) OR end_id IN (?1,?2)",params![start,end],|r|r.get(0))?;
                if linked > 0 {
                    bail!("Edit the existing range to move its breakpoints");
                }
                tx.execute(
                    "UPDATE timeline_entries SET time=?1 WHERE id=?2",
                    params![note.span.from, start],
                )?;
                tx.execute(
                    "UPDATE timeline_entries SET time=?1 WHERE id=?2",
                    params![note.span.to, end],
                )?;
            }
            (start, end)
        } else {
            tx.execute(
                "INSERT INTO timeline_entries(time,body,breakpoint) VALUES(?1,'',1)",
                [note.span.from],
            )?;
            let a = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO timeline_entries(time,body,breakpoint) VALUES(?1,'',1)",
                [note.span.to],
            )?;
            (a, tx.last_insert_rowid())
        };
        tx.execute(
            "INSERT INTO notes(start,end,label,body) VALUES(?1,?2,?3,?4)",
            params![note.span.from, note.span.to, note.label.trim(), note.body],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO span_breakpoints VALUES(?1,?2,?3)",
            params![id, start, end],
        )?;
        tx.execute(
            "INSERT INTO span_messages SELECT ?1,id FROM utterances WHERE time BETWEEN ?2 AND ?3",
            params![id, note.span.from, note.span.to],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn remove_range(&self, note: &Note) -> Result<()> {
        let changed=self.connection.execute("DELETE FROM notes WHERE id=(SELECT id FROM notes WHERE start=?1 AND end=?2 AND label=?3 AND body=?4 ORDER BY id LIMIT 1)",params![note.span.from,note.span.to,note.label,note.body])?;
        if changed != 1 {
            bail!("Range not found");
        }
        Ok(())
    }
    pub fn update_note(&self, original: &Note, note: &Note) -> Result<()> {
        validate_span_label(&note.label)?;
        if note.span.to <= note.span.from {
            bail!("End must be after start");
        }
        let tx = self.connection.unchecked_transaction()?;
        let id:i64=tx.query_row("SELECT id FROM notes WHERE start=?1 AND end=?2 AND label=?3 AND body=?4 ORDER BY id LIMIT 1",params![original.span.from,original.span.to,original.label,original.body],|r|r.get(0))?;
        tx.execute(
            "UPDATE notes SET start=?1,end=?2,label=?3,body=?4 WHERE id=?5",
            params![
                note.span.from,
                note.span.to,
                note.label.trim(),
                note.body,
                id
            ],
        )?;
        tx.execute("UPDATE timeline_entries SET time=?1 WHERE id=(SELECT start_id FROM span_breakpoints WHERE span_id=?2)",params![note.span.from,id])?;
        tx.execute("UPDATE timeline_entries SET time=?1 WHERE id=(SELECT end_id FROM span_breakpoints WHERE span_id=?2)",params![note.span.to,id])?;
        tx.execute("DELETE FROM span_messages WHERE span_id=?1", [id])?;
        tx.execute(
            "INSERT INTO span_messages SELECT ?1,id FROM utterances WHERE time BETWEEN ?2 AND ?3",
            params![id, note.span.from, note.span.to],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn notes(&self) -> Result<Vec<Note>> {
        Ok(self
            .connection
            .prepare("SELECT start,end,label,body FROM notes ORDER BY id")?
            .query_map([], |r| {
                Ok(Note {
                    span: Span {
                        from: r.get(0)?,
                        to: r.get(1)?,
                    },
                    label: r.get(2)?,
                    body: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn add_timeline_entry(&self, time: i64, body: &str, breakpoint: bool) -> Result<i64> {
        let body = body.trim();
        if body.is_empty() && !breakpoint {
            bail!("Write a comment or note");
        }
        // Couple to the preceding message on the same local day, while retaining
        // an independent timestamp so notes also work before recording starts.
        let utterance_id = self
            .utterances()?
            .into_iter()
            .rev()
            .find(|u| {
                u.time <= time
                    && DateTime::from_timestamp_millis(u.time)
                        .map(|d| d.with_timezone(&Berlin).date_naive())
                        == DateTime::from_timestamp_millis(time)
                            .map(|d| d.with_timezone(&Berlin).date_naive())
            })
            .map(|u| u.id);
        self.connection.execute(
            "INSERT INTO timeline_entries(time,utterance_id,body,breakpoint) VALUES(?1,?2,?3,?4)",
            params![time, utterance_id, body, breakpoint],
        )?;
        Ok(self.connection.last_insert_rowid())
    }
    pub fn timeline_entries(&self) -> Result<Vec<TimelineEntry>> {
        Ok(self.connection.prepare("SELECT id,time,utterance_id,body,breakpoint,label FROM timeline_entries ORDER BY time,id")?
            .query_map([], |r| Ok(TimelineEntry {id:r.get(0)?, time:r.get(1)?, utterance_id:r.get(2)?, body:r.get(3)?, breakpoint:r.get(4)?, label:r.get(5)?}))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn update_breakpoint_label(&self, id: i64, label: &str) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE timeline_entries SET label=?1 WHERE id=?2 AND breakpoint=1",
            params![label.trim(), id],
        )?;
        if changed != 1 {
            bail!("Breakpoint not found");
        }
        Ok(())
    }
    pub fn ingest_activity(
        &self,
        event: activity::ActivityPayload,
    ) -> Result<Option<activity::ActivityEntry>> {
        let payload = serde_json::to_string(&event)?;
        let changed=self.connection.execute("INSERT OR IGNORE INTO activity_events(event_id,time,application,action,payload) VALUES(?1,?2,?3,?4,?5)",params![event.event_id,event.time,event.application,event.action,payload])?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(Some(activity::ActivityEntry {
            id: self.connection.last_insert_rowid(),
            event: std::sync::Arc::new(event),
        }))
    }
    pub fn activity_for_day(&self, day: chrono::NaiveDate) -> Result<Vec<activity::ActivityEntry>> {
        let now = chrono::Utc::now().timestamp_millis();
        let start = day_time_span(day, now).from;
        let end = day
            .succ_opt()
            .map(|next| day_time_span(next, now).from)
            .unwrap_or(start + 86_400_000);
        let rows=self.connection.prepare("SELECT id,payload FROM activity_events WHERE time>=?1 AND time<?2 ORDER BY time,id")?
            .query_map(params![start,end],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, payload)| {
                Ok(activity::ActivityEntry {
                    id,
                    event: std::sync::Arc::new(serde_json::from_str(&payload)?),
                })
            })
            .collect()
    }
    pub fn reply(&self, utterance: i64, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            bail!("Enter a reply");
        }
        self.connection.execute(
            "INSERT INTO replies(utterance,body) VALUES(?1,?2)",
            params![utterance, body],
        )?;
        Ok(())
    }
    pub fn replies(&self, utterance: i64) -> Result<Vec<String>> {
        Ok(self
            .connection
            .prepare("SELECT body FROM replies WHERE utterance=?1 ORDER BY id")?
            .query_map([utterance], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
}
pub fn fixture(solo: bool) -> Result<Vec<Sample>> {
    let start = parse_local("08:42:00.000")?;
    Ok((0..=404)
        .map(|i| Sample {
            time: start + i * 60_000,
            me: if (i < 37 || (348..=404).contains(&i)) && i % 4 != 0 {
                ((i * 17 % 61) + 20) as f64 / 100.0
            } else {
                0.0
            },
            them: if !solo && (348..=404).contains(&i) && i % 3 != 0 {
                ((i * 13 % 57) + 18) as f64 / 100.0
            } else {
                0.0
            },
        })
        .collect())
}

/// Fixture samples describe one minute each. Never merge alternating speakers
/// into an overlap: only activity present in the same sample is "both".
pub fn activity_runs(samples: &[Sample]) -> Vec<(Span, u8)> {
    let mut runs: Vec<(Span, u8)> = Vec::new();
    for s in samples {
        let state = u8::from(s.me > 0.) | (u8::from(s.them > 0.) << 1);
        if state == 0 {
            continue;
        }
        if let Some((span, previous)) = runs.last_mut() {
            if *previous == state && span.to == s.time {
                span.to += 60_000;
                continue;
            }
        }
        runs.push((
            Span {
                from: s.time,
                to: s.time + 60_000,
            },
            state,
        ));
    }
    runs
}

pub fn day_extent(samples: &[Sample]) -> Result<Span> {
    let active = extent(samples)?;
    Ok(Span {
        from: active.from.div_euclid(3_600_000) * 3_600_000,
        to: (active.to.div_euclid(3_600_000) + 1) * 3_600_000,
    })
}

/// Inclusive message selection uses message start timestamps, not inferred audio
/// durations. One extra millisecond makes a single-message span non-empty.
pub fn message_span(messages: &[Utterance], first: usize, last: usize) -> Option<Span> {
    if first > last {
        return None;
    }
    let from = messages.get(first)?.time;
    let to = messages.get(last)?.time.checked_add(1)?;
    (to > from).then_some(Span { from, to })
}

/// Combine synchronized source intervals without turning sequential speakers into overlaps.
pub fn combine_activity(spans: &[(Span, u8)]) -> Vec<(Span, u8)> {
    let mut events = std::collections::BTreeMap::<i64, [i32; 2]>::new();
    for (span, source) in spans {
        let channel = usize::from(*source == 2);
        events.entry(span.from).or_default()[channel] += 1;
        events.entry(span.to).or_default()[channel] -= 1;
    }
    let mut counts = [0, 0];
    let mut cursor = None;
    let mut runs: Vec<(Span, u8)> = Vec::new();
    for (time, delta) in events {
        let state = u8::from(counts[0] > 0) | (u8::from(counts[1] > 0) << 1);
        if let Some(from) = cursor.filter(|from| *from < time) {
            if state != 0 {
                if let Some((last, previous)) = runs
                    .last_mut()
                    .filter(|(last, previous)| *previous == state && last.to == from)
                {
                    last.to = time;
                    let _ = previous;
                } else {
                    runs.push((Span { from, to: time }, state));
                }
            }
        }
        counts[0] += delta[0];
        counts[1] += delta[1];
        cursor = Some(time);
    }
    runs
}
