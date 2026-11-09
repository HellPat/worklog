use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Destination {
    pub provider: String,
    pub id: String,
    pub title: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub id: Uuid,
    pub source: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub id: Uuid,
    pub destination: Destination,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub segments: Vec<Segment>,
}
impl Batch {
    pub fn prompt(&self) -> String {
        let mut text = format!("Conversation transcript, {} → {}\n\n", self.start, self.end);
        for segment in &self.segments {
            text.push_str(&format!(
                "[{}] {}: {}\n",
                segment.start, segment.source, segment.text
            ));
        }
        text
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Transcript {
        segment: Segment,
    },
    Conversation {
        at: DateTime<Utc>,
        destination: Option<Destination>,
    },
    Range {
        at: DateTime<Utc>,
    },
    SendClicked {
        at: DateTime<Utc>,
    },
    SendRequested {
        batch: Batch,
    },
    SendAccepted {
        id: Uuid,
        at: DateTime<Utc>,
    },
    SendFailed {
        id: Uuid,
        at: DateTime<Utc>,
        message: String,
    },
    SendUncertain {
        id: Uuid,
        at: DateTime<Utc>,
        message: String,
    },
    Recording {
        at: DateTime<Utc>,
        active: bool,
    },
    MicrophoneMuted {
        at: DateTime<Utc>,
        muted: bool,
    },
}
#[derive(Clone, Debug)]
pub struct Model {
    pub segments: Vec<Segment>,
    pub destination: Option<Destination>,
    pub topic_start: DateTime<Utc>,
    pub range_start: DateTime<Utc>,
    pub range_floor: DateTime<Utc>,
    pub last_sent: Option<DateTime<Utc>>,
    pub delivered: BTreeSet<Uuid>,
    pub pending: Option<Batch>,
    pub uncertain: bool,
    pub auto: bool,
    pub interval_minutes: u32,
    pub next_send: Option<DateTime<Utc>>,
}
impl Model {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            segments: vec![],
            destination: None,
            topic_start: now,
            range_start: now,
            range_floor: now,
            last_sent: None,
            delivered: BTreeSet::new(),
            pending: None,
            uncertain: false,
            auto: false,
            interval_minutes: 7,
            next_send: None,
        }
    }
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Transcript { segment } => {
                if segment.start >= self.range_floor && !self.delivered.contains(&segment.id) {
                    self.range_start = self.range_start.min(segment.start);
                }
                self.segments.push(segment.clone());
            }
            Event::Conversation { at, destination } => {
                self.topic_start = *at;
                self.range_start = *at;
                self.range_floor = *at;
                self.destination = destination.clone();
                self.auto = false;
                self.next_send = None;
            }
            Event::Range { at } => {
                self.range_start = (*at).max(self.topic_start);
                self.range_floor = self.range_start;
            }
            Event::SendRequested { batch } => {
                self.pending = Some(batch.clone());
                self.uncertain = true;
            }
            Event::SendAccepted { id, at } => {
                if let Some(batch) = self.pending.take().filter(|batch| batch.id == *id) {
                    self.delivered.extend(batch.segments.iter().map(|s| s.id));
                    self.range_start = self.range_start.max(batch.end);
                    self.last_sent = Some(*at);
                    self.uncertain = false;
                    self.rearm(*at);
                }
            }
            Event::SendFailed { id, .. } => {
                if self.pending.as_ref().is_some_and(|b| b.id == *id) {
                    self.pending = None;
                    self.uncertain = false;
                }
                self.auto = false;
                self.next_send = None;
            }
            Event::SendUncertain { .. } => {
                self.uncertain = true;
                self.auto = false;
                self.next_send = None;
            }
            Event::Recording { .. } | Event::MicrophoneMuted { .. } | Event::SendClicked { .. } => {
            }
        }
    }
    pub fn rearm(&mut self, now: DateTime<Utc>) {
        self.next_send = if self.auto && self.destination.is_some() {
            Some(now + Duration::minutes(i64::from(self.interval_minutes.max(1))))
        } else {
            None
        };
    }
    pub fn batch(&self, now: DateTime<Utc>) -> Option<Batch> {
        if self.pending.is_some() || self.uncertain {
            return None;
        }
        let destination = self.destination.clone()?;
        // A late finalization remains eligible after the previous send. Topic boundaries
        // use capture timestamps, never arrival time. User edits can only narrow a topic.
        let segments: Vec<_> = self
            .segments
            .iter()
            .filter(|s| {
                s.start >= self.topic_start
                    && s.end <= now
                    && !self.delivered.contains(&s.id)
                    && s.start >= self.range_floor
            })
            .cloned()
            .collect();
        if segments.is_empty() {
            return None;
        }
        let start = segments
            .iter()
            .map(|s| s.start)
            .min()
            .unwrap_or(self.range_start);
        Some(Batch {
            id: Uuid::new_v4(),
            destination,
            start,
            end: now,
            segments,
        })
    }
}
