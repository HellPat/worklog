#![forbid(unsafe_code)]
use anyhow::Result;
use chrono::{Duration, Utc};
use team_up::{
    journal::Journal,
    model::{Destination, Event, Segment},
};
use uuid::Uuid;
fn target(name: &str) -> Destination {
    Destination {
        provider: "test".into(),
        id: name.into(),
        title: name.into(),
    }
}
fn speech(at: chrono::DateTime<Utc>, text: &str) -> Segment {
    Segment {
        id: Uuid::new_v4(),
        source: "microphone".into(),
        start: at,
        end: at + Duration::seconds(1),
        text: text.into(),
    }
}
#[test]
fn day_log_isolation_delivery_and_recovery() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut journal = Journal::open(dir.path())?;
    let now = Utc::now();
    journal.append(Event::Transcript {
        segment: speech(now, "private day notes"),
    })?;
    journal.append(Event::Conversation {
        at: now + Duration::seconds(2),
        destination: Some(target("meeting")),
    })?;
    journal.append(Event::Transcript {
        segment: speech(now + Duration::seconds(3), "meeting only"),
    })?;
    let batch = journal
        .model
        .batch(now + Duration::seconds(5))
        .ok_or_else(|| anyhow::anyhow!("Expected batch"))?;
    assert_eq!(batch.segments.len(), 1);
    assert!(batch.prompt().contains("meeting only"));
    assert!(!batch.prompt().contains("private"));
    journal.append(Event::SendRequested {
        batch: batch.clone(),
    })?;
    drop(journal);
    let mut journal = Journal::open(dir.path())?;
    assert!(journal.model.uncertain);
    assert!(journal.model.batch(now + Duration::seconds(6)).is_none());
    journal.append(Event::SendAccepted {
        id: batch.id,
        at: now + Duration::seconds(6),
    })?;
    assert!(journal.model.batch(now + Duration::seconds(7)).is_none());
    journal.append(Event::Transcript {
        segment: speech(now + Duration::seconds(4), "late finalized speech"),
    })?;
    let late = journal
        .model
        .batch(now + Duration::seconds(8))
        .ok_or_else(|| anyhow::anyhow!("Late speech lost"))?;
    assert_eq!(late.segments.len(), 1);
    journal.append(Event::SendRequested {
        batch: late.clone(),
    })?;
    journal.append(Event::SendFailed {
        id: late.id,
        at: now + Duration::seconds(9),
        message: "offline".into(),
    })?;
    assert_eq!(journal.model.last_sent, Some(now + Duration::seconds(6)));
    assert!(journal.model.batch(now + Duration::seconds(10)).is_some());
    journal.append(Event::Conversation {
        at: now + Duration::seconds(11),
        destination: Some(target("next topic")),
    })?;
    assert!(journal.model.batch(now + Duration::seconds(12)).is_none());
    assert_eq!(journal.model.segments.len(), 3);
    Ok(())
}
#[test]
fn manual_range_exclusion_survives_success_and_late_arrivals() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut journal = Journal::open(directory.path())?;
    let now = Utc::now();
    journal.append(Event::Conversation {
        at: now,
        destination: Some(target("work")),
    })?;
    journal.append(Event::Range {
        at: now + Duration::seconds(10),
    })?;
    journal.append(Event::Transcript {
        segment: speech(now + Duration::seconds(11), "share"),
    })?;
    let batch = journal
        .model
        .batch(now + Duration::seconds(15))
        .ok_or_else(|| anyhow::anyhow!("No batch"))?;
    journal.append(Event::SendRequested {
        batch: batch.clone(),
    })?;
    journal.append(Event::SendAccepted {
        id: batch.id,
        at: now + Duration::seconds(16),
    })?;
    journal.append(Event::Transcript {
        segment: speech(now + Duration::seconds(3), "private late result"),
    })?;
    assert!(journal.model.batch(now + Duration::seconds(20)).is_none());
    Ok(())
}
#[test]
fn automatic_deadline_is_separate_from_interval_and_acknowledgement() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut journal = Journal::open(directory.path())?;
    let now = Utc::now();
    journal.model.auto = true;
    journal.model.rearm(now);
    assert!(journal.model.next_send.is_none());
    journal.append(Event::Conversation {
        at: now,
        destination: Some(target("work")),
    })?;
    journal.model.auto = true;
    journal.model.rearm(now);
    assert_eq!(journal.model.next_send, Some(now + Duration::minutes(7)));
    journal.model.next_send = journal.model.next_send.map(|at| at + Duration::minutes(1));
    assert_eq!(journal.model.interval_minutes, 7);
    journal.append(Event::Transcript {
        segment: speech(now + Duration::seconds(1), "work"),
    })?;
    let batch = journal
        .model
        .batch(now + Duration::seconds(5))
        .ok_or_else(|| anyhow::anyhow!("No batch"))?;
    journal.append(Event::SendRequested {
        batch: batch.clone(),
    })?;
    assert_eq!(journal.model.last_sent, None);
    journal.append(Event::SendAccepted {
        id: batch.id,
        at: now + Duration::seconds(6),
    })?;
    assert_eq!(
        journal.model.next_send,
        Some(now + Duration::minutes(7) + Duration::seconds(6))
    );
    Ok(())
}

#[test]
fn interrupted_final_append_is_preserved_and_recovered() -> Result<()> {
    use std::io::Write;
    let directory = tempfile::tempdir()?;
    let mut journal = Journal::open(directory.path())?;
    let now = Utc::now();
    journal.append(Event::Conversation {
        at: now,
        destination: Some(target("work")),
    })?;
    let path = directory
        .path()
        .join(format!("{}.jsonl", now.format("%Y-%m-%d")));
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(b"{\"event\":\"send_requ")?;
    file.sync_all()?;
    drop(file);
    drop(journal);
    let mut recovered = Journal::open(directory.path())?;
    assert_eq!(
        recovered.model.destination.as_ref().map(|d| d.id.as_str()),
        Some("work")
    );
    recovered.append(Event::Transcript {
        segment: speech(now + Duration::seconds(1), "after restart"),
    })?;
    drop(recovered);
    assert_eq!(Journal::open(directory.path())?.model.segments.len(), 1);
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 2);
    Ok(())
}

#[test]
fn legacy_writer_rejection_is_recovered_as_unsent_without_advancing_range() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut journal = Journal::open(directory.path())?;
    let now = Utc::now();
    journal.append(Event::Conversation {
        at: now,
        destination: Some(target("open-in-desktop")),
    })?;
    journal.append(Event::Transcript {
        segment: speech(now + Duration::seconds(1), "keep for retry"),
    })?;
    let batch = journal
        .model
        .batch(now + Duration::seconds(5))
        .ok_or_else(|| anyhow::anyhow!("No batch"))?;
    journal.append(Event::SendRequested {
        batch: batch.clone(),
    })?;
    journal.append(Event::SendUncertain { id:batch.id, at:now+Duration::seconds(6),message:"Codex rejected request: {\"code\":-32600,\"message\":\"thread example already has an active writer\"}".into() })?;
    drop(journal);
    let recovered = Journal::open(directory.path())?;
    assert!(!recovered.model.uncertain);
    assert!(recovered.model.pending.is_none());
    assert!(recovered.model.last_sent.is_none());
    assert_eq!(
        recovered
            .model
            .batch(now + Duration::seconds(8))
            .map(|batch| batch.segments.len()),
        Some(1)
    );
    Ok(())
}
