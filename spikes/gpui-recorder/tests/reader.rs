use anyhow::Result;
use serde_json::{Value, json};
use team_up_gpui_spike::{
    Store,
    model::Segment,
    reader::{KINDS, Reader, Scope},
};
fn scope() -> Scope {
    Scope {
        from: 0,
        to: 100_000,
        kinds: KINDS.iter().map(|s| s.to_string()).collect(),
        after: None,
    }
}
fn segment(time: i64, text: &str) -> Segment {
    Segment {
        id: uuid::Uuid::new_v4(),
        source: "microphone".into(),
        start: chrono::DateTime::from_timestamp_millis(time).unwrap(),
        end: chrono::DateTime::from_timestamp_millis(time + 100).unwrap(),
        text: text.into(),
    }
}
fn event_id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
#[test]
fn cursor_catches_late_speech_edits_deletions_and_is_read_only() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("reader.sqlite");
    let store = Store::open(&path)?;
    store.ingest_segment(&segment(10_000, "First final utterance"))?;
    let breakpoint = store.add_timeline_entry(20_000, "", true)?;
    let reader = Reader::open(&path)?;
    let first = reader.snapshot(scope(), 200)?;
    assert_eq!(first.events.len(), 2);
    assert!(reader.next(&first.next_cursor, 200)?.events.is_empty());
    // This finishes later, but starts before everything already read.
    let late = store
        .ingest_segment(&segment(500, "Late recognition"))?
        .unwrap();
    store.update_breakpoint_label(breakpoint, "Changed | label\nUnicode 🐝")?;
    let next = reader.next(&first.next_cursor, 200)?;
    assert_eq!(
        next.events.len(),
        2,
        "Text and metadata in one commit should be coalesced"
    );
    assert!(
        next.events
            .iter()
            .any(|e| event_id(e) == format!("message:{}", late.id))
    );
    assert!(
        next.events
            .iter()
            .any(|e| e["label"] == "Changed | label\nUnicode 🐝")
    );
    store.update_breakpoint_label(breakpoint, "Changed | label\nUnicode 🐝")?;
    assert!(
        reader.next(&next.next_cursor, 200)?.events.is_empty(),
        "Saving an unchanged label produced a spurious update"
    );
    let db = rusqlite::Connection::open(&path)?;
    db.execute("DELETE FROM timeline_entries WHERE id=?1", [breakpoint])?;
    let deletion = reader.next(&next.next_cursor, 200)?;
    assert_eq!(deletion.events.len(), 1);
    assert_eq!(deletion.events[0]["operation"], "delete");
    let before: i64 = db.query_row("SELECT MAX(seq) FROM reader_changes", [], |r| r.get(0))?;
    assert!(reader.next(&deletion.next_cursor, 200)?.events.is_empty());
    let after: i64 = db.query_row("SELECT MAX(seq) FROM reader_changes", [], |r| r.get(0))?;
    assert_eq!(
        before, after,
        "Reader queries must not create their own activity events"
    );
    Ok(())
}
#[test]
fn snapshot_pages_are_frozen_and_cursor_resumes_after_restart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("paging.sqlite");
    let store = Store::open(&path)?;
    for time in [1000, 2000, 3000] {
        store.ingest_segment(&segment(time, &format!("Speech {time}")))?;
    }
    let reader = Reader::open(&path)?;
    let first = reader.snapshot(scope(), 1)?;
    assert!(first.has_more);
    let changed = first.events[0]["id"].as_str().unwrap().to_owned();
    let db = rusqlite::Connection::open(&path)?;
    db.execute(
        "UPDATE utterances SET text='Edited while paginating' WHERE id=2",
        [],
    )?;
    let new = store
        .ingest_segment(&segment(1500, "Arrived during pagination"))?
        .unwrap();
    let second = reader.next(&first.next_cursor, 1)?;
    assert_eq!(
        second.events[0]["text"], "Speech 2000",
        "Snapshot changed between pages"
    );
    let third = reader.next(&second.next_cursor, 1)?;
    assert!(!third.has_more);
    assert_eq!(third.events[0]["text"], "Speech 3000");
    drop(reader);
    drop(store);
    let reader = Reader::open(&path)?;
    let updates = reader.next(&third.next_cursor, 200)?;
    assert_eq!(updates.events.len(), 2);
    assert!(
        updates
            .events
            .iter()
            .any(|e| e["text"] == "Edited while paginating")
    );
    assert!(
        updates
            .events
            .iter()
            .any(|e| event_id(e) == format!("message:{}", new.id))
    );
    assert!(!updates.events.iter().any(|e| event_id(e) == changed));
    Ok(())
}
#[test]
fn filters_track_entries_moving_out_of_range_and_message_anchors() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("filters.sqlite");
    let store = Store::open(&path)?;
    let anchor = store.ingest_segment(&segment(2000, "Anchor"))?.unwrap();
    store.ingest_segment(&segment(1000, "Before"))?;
    store.ingest_segment(&segment(3000, "After"))?;
    let breakpoint = store.add_timeline_entry(3500, "", true)?;
    let reader = Reader::open(&path)?;
    let mut selected = scope();
    selected.after = Some(reader.anchor(&format!("message:{}", anchor.id), false)?);
    let read = reader.snapshot(selected, 200)?;
    assert_eq!(read.events.len(), 2);
    assert!(
        read.events
            .iter()
            .all(|e| e["time"].as_str().unwrap() > "1970-01-01T00:00:02.000Z")
    );
    let mut selected = scope();
    selected.from = 3000;
    selected.to = 4000;
    selected.kinds = vec!["breakpoint".into()];
    let before = reader.snapshot(selected, 200)?;
    assert_eq!(before.events.len(), 1);
    let db = rusqlite::Connection::open(&path)?;
    db.execute(
        "UPDATE timeline_entries SET time=5000 WHERE id=?1",
        [breakpoint],
    )?;
    let next = reader.next(&before.next_cursor, 200)?;
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0]["operation"], "delete");
    assert_eq!(reader.anchor(&format!("entry:{breakpoint}"), true)?.0, 5000);
    Ok(())
}
#[test]
fn old_database_backfills_once_and_foreign_cursors_are_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("old.sqlite");
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch(
        include_str!("../schema.sql")
            .split("-- Durable cursor feed")
            .next()
            .unwrap(),
    )?;
    db.execute(
        "INSERT INTO utterances VALUES(42,1000,'Them','Existing recording')",
        [],
    )?;
    drop(db);
    Store::open(&path)?;
    let reader = Reader::open(&path)?;
    let first = reader.snapshot(scope(), 200)?;
    assert_eq!(first.events.len(), 1);
    Store::open(&path)?;
    assert!(
        reader.next(&first.next_cursor, 200)?.events.is_empty(),
        "Reopen duplicated the backfill"
    );
    let other = dir.path().join("other.sqlite");
    Store::open(&other)?;
    assert!(Reader::open(&other)?.next(&first.next_cursor, 200).is_err());
    assert!(reader.next("garbage", 200).is_err());
    Ok(())
}
#[test]
fn gcf_and_json_preserve_full_activity_payload_and_special_characters() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("activity.sqlite");
    let store = Store::open(&path)?;
    let value = json!({"event_id":"native-1","time":1000,"application":"Browser","action":"click","url":"https://example.test/?q=|#full","metadata":{"text":"# header | value\n\"quoted\" 🐝".repeat(1000),"future":[null,true,{"x":1}]}});
    let event = serde_json::from_value(value)?;
    store.ingest_activity(event)?;
    let reader = Reader::open(&path)?;
    let full = reader.activity("activity:native-1")?;
    let encoded = gcf::encode_generic(&full).map_err(anyhow::Error::msg)?;
    assert_eq!(
        gcf::decode_generic(&encoded).map_err(anyhow::Error::msg)?,
        full
    );
    let page = reader.snapshot(scope(), 200)?;
    assert_eq!(page.events.len(), 1);
    assert!(page.events[0].get("metadata").is_none());
    assert_eq!(page.events[0]["url"], "https://example.test/?q=|#full");
    Ok(())
}
#[test]
fn deleting_comments_and_transcript_emits_tombstones_in_the_original_day() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cascade.sqlite");
    let store = Store::open(&path)?;
    let utterance = store.ingest_segment(&segment(5000, "Speech"))?.unwrap();
    store.reply(utterance.id, "Legacy comment")?;
    let reader = Reader::open(&path)?;
    let mut selected = scope();
    selected.from = 4000;
    let first = reader.snapshot(selected, 200)?;
    assert_eq!(first.events.len(), 2);
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch("PRAGMA foreign_keys=ON;")?;
    db.execute("DELETE FROM replies WHERE utterance=?1", [utterance.id])?;
    db.execute("DELETE FROM utterances WHERE id=?1", [utterance.id])?;
    let next = reader.next(&first.next_cursor, 200)?;
    assert_eq!(next.events.len(), 2);
    assert!(next.events.iter().all(|e| e["operation"] == "delete"));
    Ok(())
}
