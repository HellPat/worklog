//! Read-only, snapshot-consistent access to the durable timeline change feed.
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, SecondsFormat};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
pub const KINDS: &[&str] = &[
    "transcript",
    "activity",
    "note",
    "breakpoint",
    "span",
    "comment",
];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Scope {
    pub from: i64,
    pub to: i64,
    pub kinds: Vec<String>,
    pub after: Option<(i64, String)>,
}
impl Scope {
    pub fn validate(&self) -> Result<()> {
        if self.from >= self.to {
            bail!("The read range must end after it starts");
        }
        if self.kinds.is_empty()
            || self
                .kinds
                .iter()
                .any(|kind| !KINDS.contains(&kind.as_str()))
        {
            bail!("Kinds must be selected from {}", KINDS.join(","));
        }
        Ok(())
    }
    fn contains(&self, kind: &str, time: i64, key: &str) -> bool {
        time >= self.from
            && time < self.to
            && self.kinds.iter().any(|k| k == kind)
            && self
                .after
                .as_ref()
                .is_none_or(|(t, k)| time > *t || (time == *t && key > k.as_str()))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Position {
    Snapshot {
        watermark: i64,
        after: Option<(i64, String)>,
    },
    Updates {
        after: i64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cursor {
    version: u8,
    database: String,
    scope: Scope,
    position: Position,
}
impl Cursor {
    fn encode(&self) -> Result<String> {
        Ok(format!(
            "wl1.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)
        ))
    }
    fn decode(text: &str) -> Result<Self> {
        if text.len() > 16384 {
            bail!("Cursor is too large");
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(text.strip_prefix("wl1.").context("Invalid cursor prefix")?)
            .context("Invalid cursor encoding")?;
        let cursor: Self = serde_json::from_slice(&bytes).context("Invalid cursor contents")?;
        if cursor.version != 1 {
            bail!("Unsupported cursor version");
        }
        cursor.scope.validate()?;
        Ok(cursor)
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Page {
    pub schema: String,
    pub mode: String,
    pub events: Vec<Value>,
    pub next_cursor: String,
    pub has_more: bool,
    pub through_sequence: i64,
}
struct Change {
    seq: i64,
    key: String,
    kind: String,
    time: i64,
    previous_kind: Option<String>,
    previous_time: Option<i64>,
    operation: String,
    payload: String,
}
fn change(row: &rusqlite::Row<'_>) -> rusqlite::Result<Change> {
    Ok(Change {
        seq: row.get(0)?,
        key: row.get(1)?,
        kind: row.get(2)?,
        time: row.get(3)?,
        previous_kind: row.get(4)?,
        previous_time: row.get(5)?,
        operation: row.get(6)?,
        payload: row.get(7)?,
    })
}
fn timestamp(time: i64) -> Result<String> {
    Ok(DateTime::from_timestamp_millis(time)
        .context("Stored timestamp is out of range")?
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}
fn normalized(mut value: Value) -> Result<Value> {
    for (raw, name) in [("time_ms", "time"), ("end_ms", "end")] {
        if let Some(time) = value.as_object_mut().and_then(|object| object.remove(raw)) {
            value[name] = if let Some(time) = time.as_i64() {
                json!(timestamp(time)?)
            } else {
                Value::Null
            };
        }
    }
    Ok(value)
}
pub struct Reader {
    connection: Connection,
    database: String,
}
impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("Cannot open Worklog database {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let database=connection.query_row("SELECT database_id FROM reader_state WHERE id=1 AND initialized=1",[],|r|r.get(0))
            .context("Reader feed is not initialized. Open the latest Worklog app once, or run --check-db with TEAM_UP_SPIKE_DB set")?;
        Ok(Self {
            connection,
            database,
        })
    }
    pub fn snapshot(&self, scope: Scope, limit: usize) -> Result<Page> {
        scope.validate()?;
        self.page(
            Cursor {
                version: 1,
                database: self.database.clone(),
                scope,
                position: Position::Snapshot {
                    watermark: -1,
                    after: None,
                },
            },
            limit,
        )
    }
    pub fn next(&self, cursor: &str, limit: usize) -> Result<Page> {
        self.page(Cursor::decode(cursor)?, limit)
    }
    fn page(&self, mut cursor: Cursor, limit: usize) -> Result<Page> {
        if !(1..=2000).contains(&limit) {
            bail!("Limit must be between 1 and 2000");
        }
        if cursor.database != self.database {
            bail!("This cursor belongs to a different Worklog database");
        }
        let tx = self.connection.unchecked_transaction()?;
        let current: i64 =
            tx.query_row("SELECT COALESCE(MAX(seq),0) FROM reader_changes", [], |r| {
                r.get(0)
            })?;
        let scope = &cursor.scope;
        let kinds = serde_json::to_string(&scope.kinds)?;
        let anchor = scope.after.clone().unwrap_or((i64::MIN, String::new()));
        let (mode, watermark, mut changes) = match &cursor.position {
            Position::Snapshot { watermark, after } => {
                let high = if *watermark < 0 { current } else { *watermark };
                if high > current {
                    bail!(
                        "Cursor is ahead of this database; start a new read after restoring a backup"
                    );
                }
                let lower = after.as_ref().unwrap_or(&anchor);
                let sql="WITH latest AS (SELECT entity_key,MAX(seq) AS seq FROM reader_changes WHERE seq<=?1 GROUP BY entity_key)
                    SELECT c.seq,c.entity_key,c.kind,c.time,c.previous_kind,c.previous_time,c.operation,c.payload
                    FROM reader_changes c JOIN latest l ON c.seq=l.seq
                    WHERE c.operation='upsert' AND c.time>=?2 AND c.time<?3 AND c.kind IN (SELECT value FROM json_each(?4))
                        AND (c.time,c.entity_key)>(?5,?6) ORDER BY c.time,c.entity_key LIMIT ?7";
                let rows = tx
                    .prepare(sql)?
                    .query_map(
                        params![
                            high,
                            scope.from,
                            scope.to,
                            kinds,
                            lower.0,
                            lower.1,
                            (limit + 1) as i64
                        ],
                        change,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ("snapshot", high, rows)
            }
            Position::Updates { after } => {
                if *after < 0 || *after > current {
                    bail!("Cursor is ahead of this database or invalid; start a new read");
                }
                let sql="SELECT seq,entity_key,kind,time,previous_kind,previous_time,operation,payload FROM reader_changes
                    WHERE seq>?1 AND seq<=?2 AND (
                        (time>=?3 AND time<?4 AND kind IN (SELECT value FROM json_each(?5)) AND (time,entity_key)>(?6,?7)) OR
                        (previous_time>=?3 AND previous_time<?4 AND previous_kind IN (SELECT value FROM json_each(?5)) AND (previous_time,entity_key)>(?6,?7)))
                    ORDER BY seq LIMIT ?8";
                let rows = tx
                    .prepare(sql)?
                    .query_map(
                        params![
                            after,
                            current,
                            scope.from,
                            scope.to,
                            kinds,
                            anchor.0,
                            anchor.1,
                            (limit + 1) as i64
                        ],
                        change,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ("updates", current, rows)
            }
        };
        let more = changes.len() > limit;
        changes.truncate(limit);
        let last = changes.last();
        cursor.position = if mode == "snapshot" && more {
            Position::Snapshot {
                watermark,
                after: last.map(|c| (c.time, c.key.clone())),
            }
        } else {
            Position::Updates {
                after: if more {
                    last.map(|c| c.seq).unwrap_or(watermark)
                } else {
                    watermark
                },
            }
        };
        let mut events = Vec::with_capacity(changes.len());
        for c in changes {
            let mut value = if c.operation == "delete" || !scope.contains(&c.kind, c.time, &c.key) {
                let (kind, time) = if c.operation == "delete" {
                    (c.kind, c.time)
                } else {
                    (
                        c.previous_kind.unwrap_or(c.kind),
                        c.previous_time.unwrap_or(c.time),
                    )
                };
                json!({"id":c.key,"kind":kind,"time":timestamp(time)?,"operation":"delete"})
            } else {
                let mut value = normalized(serde_json::from_str(&c.payload)?)?;
                value["operation"] = json!("upsert");
                value
            };
            value["sequence"] = json!(c.seq);
            events.push(value);
        }
        let events = if mode == "updates" {
            // Metadata and text can change in one transaction. Keep only the
            // last relevant operation for each identity within this batch.
            let mut latest = std::collections::BTreeMap::new();
            for event in events {
                latest.insert(
                    event["id"]
                        .as_str()
                        .context("Feed event has no identity")?
                        .to_owned(),
                    event,
                );
            }
            let mut events: Vec<_> = latest.into_values().collect();
            events.sort_by_key(|event| event["sequence"].as_i64().unwrap_or(0));
            events
        } else {
            events
        };
        let result = Page {
            schema: "worklog.timeline.v1".into(),
            mode: mode.into(),
            events,
            next_cursor: cursor.encode()?,
            has_more: more,
            through_sequence: watermark,
        };
        tx.commit()?;
        Ok(result)
    }
    pub fn anchor(&self, id: &str, breakpoint: bool) -> Result<(i64, String)> {
        if breakpoint {
            let id: i64 = id
                .strip_prefix("entry:")
                .unwrap_or(id)
                .parse()
                .context("Use a breakpoint ID such as entry:42")?;
            let time = self
                .connection
                .query_row(
                    "SELECT time FROM timeline_entries WHERE id=?1 AND breakpoint=1",
                    [id],
                    |r| r.get(0),
                )
                .context("Breakpoint was not found")?;
            Ok((time, format!("entry:{id}")))
        } else {
            let raw = id.strip_prefix("message:").unwrap_or(id);
            let (id,time)=if let Ok(id)=raw.parse::<i64>() {
                self.connection.query_row("SELECT id,time FROM utterances WHERE id=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))
            } else {
                self.connection.query_row("SELECT u.id,u.time FROM utterances u JOIN transcript_audio a ON a.utterance_id=u.id WHERE a.id=?1",[raw],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))
            }.context("Transcript message was not found")?;
            Ok((time, format!("message:{id}")))
        }
    }
    pub fn span(&self, id: &str) -> Result<(i64, i64)> {
        let id: i64 = id
            .strip_prefix("span:")
            .unwrap_or(id)
            .parse()
            .context("Use a span ID such as span:42")?;
        self.connection
            .query_row("SELECT start,end FROM notes WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)?.saturating_add(1)))
            })
            .context("Span was not found")
    }
    pub fn activity(&self, id: &str) -> Result<Value> {
        let id = id.strip_prefix("activity:").unwrap_or(id);
        let payload: String = self
            .connection
            .query_row(
                "SELECT payload FROM activity_events WHERE event_id=?1",
                [id],
                |r| r.get(0),
            )
            .context("Activity event was not found")?;
        Ok(
            json!({"schema":"worklog.activity.v1","id":format!("activity:{id}"),"payload":serde_json::from_str::<Value>(&payload)?}),
        )
    }
    pub fn days(&self, limit: usize) -> Result<Value> {
        if !(1..=2000).contains(&limit) {
            bail!("Limit must be between 1 and 2000");
        }
        let tx = self.connection.unchecked_transaction()?;
        let mut days =
            std::collections::BTreeMap::<String, std::collections::BTreeMap<String, usize>>::new();
        let mut query=tx.prepare("WITH latest AS (SELECT MAX(seq) seq FROM reader_changes GROUP BY entity_key) SELECT c.time,c.kind FROM reader_changes c JOIN latest l ON c.seq=l.seq WHERE c.operation='upsert'")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (time, kind) = row?;
            let day = DateTime::from_timestamp_millis(time)
                .context("Stored date is out of range")?
                .with_timezone(&chrono_tz::Europe::Berlin)
                .date_naive()
                .to_string();
            *days.entry(day).or_default().entry(kind).or_default() += 1;
        }
        let result: Vec<_> = days
            .into_iter()
            .rev()
            .take(limit)
            .map(|(day, counts)| json!({"day":day,"counts":counts}))
            .collect();
        drop(query);
        tx.commit()?;
        Ok(json!({"schema":"worklog.days.v1","timezone":"Europe/Berlin","days":result}))
    }
}
