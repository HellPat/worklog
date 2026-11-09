#![deny(clippy::unwrap_used, clippy::expect_used)]
use anyhow::{Result, bail};
use chrono::{DateTime, TimeZone};
use chrono_tz::Europe::Berlin;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// Ask the packaged sqlite3def to plan against the embedded desired schema,
/// then execute the plan atomically on our own SQLite connection.
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
pub struct Store {
    connection: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        reconcile_schema(path, include_str!("../schema.sql"))?;
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(Self { connection })
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
    pub fn save(&self, note: &Note) -> Result<()> {
        if note.label.trim().is_empty() {
            bail!("Enter a label");
        }
        if note.span.to <= note.span.from {
            bail!("To must be after From");
        }
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO notes(start,end,label,body) VALUES(?1,?2,?3,?4)",
            params![note.span.from, note.span.to, note.label, note.body],
        )?;
        let id = tx.last_insert_rowid();
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
