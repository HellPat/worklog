use anyhow::Result;
use team_up_gpui_spike::*;
#[test]
fn ranges_preserve_direction_bounds_and_precision() -> Result<()> {
    let s = Span::new(100, 200)?;
    assert_eq!(s, Span { from: 100, to: 200 });
    assert_eq!(Span::new(200, 100)?, s);
    assert!(Span::new(100, 100).is_err());
    for (t, yes) in [
        (99, false),
        (100, true),
        (150, true),
        (200, true),
        (201, false),
    ] {
        assert_eq!(s.contains(t), yes);
    }
    for (f, t) in [
        (-0.5, 100),
        (0., 100),
        (0.123, 112),
        (0.126, 113),
        (0.5, 150),
        (1., 200),
        (1.5, 200),
    ] {
        assert_eq!(s.at(f), t);
    }
    Ok(())
}
#[test]
fn extent_uses_only_activity_and_preserves_gaps() -> Result<()> {
    let samples = vec![
        Sample {
            time: 0,
            me: 0.,
            them: 0.,
        },
        Sample {
            time: 90,
            me: 0.,
            them: 0.2,
        },
        Sample {
            time: 10,
            me: 0.3,
            them: 0.,
        },
        Sample {
            time: 100,
            me: 0.,
            them: 0.,
        },
    ];
    assert_eq!(extent(&samples)?, Span { from: 10, to: 90 });
    assert!(extent(&[]).is_err());
    assert!(extent(&samples[..1]).is_err());
    assert!(extent(&samples[1..2]).is_err());
    Ok(())
}
#[test]
fn local_time_and_full_hour_ticks() -> Result<()> {
    let a = parse_local("08:42:00.123")?;
    assert_eq!(local(a, true), "08:42:00.123");
    assert_eq!(local(a, false), "08:42");
    assert_eq!(local(i64::MAX, false), "");
    assert!(parse_local("bad").is_err());
    let h = parse_local("09:00:00.000")?;
    assert_eq!(hours(Span::new(a, h)?), vec![h]);
    assert!(hours(Span::new(a, h - 1)?).is_empty());
    assert_eq!(
        hours(Span::new(h, h + 7_200_000)?),
        vec![h + 3_600_000, h + 7_200_000]
    );
    Ok(())
}
#[test]
fn sqlite_roundtrip_and_validation() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("sample.sqlite");
    let mut store = Store::open(&path)?;
    store.seed()?;
    store.seed()?;
    let u = store.utterances()?;
    assert_eq!(u.len(), 5);
    assert_eq!(u[0].id, 0);
    assert_eq!(u[0].speaker, "Me");
    assert_eq!(u[0].time, parse_local("14:30:04.120")?);
    assert_eq!(u[4].id, 4);
    assert!(u[4].text.contains("latest text"));
    let note = Note {
        span: Span::new(12, 36)?,
        label: "Planning".into(),
        body: "Keep this locally".into(),
    };
    store.save(&note)?;
    assert!(
        store
            .save(&Note {
                label: " \t".into(),
                ..note.clone()
            })
            .is_err()
    );
    assert!(
        store
            .save(&Note {
                span: Span { from: 20, to: 20 },
                ..note.clone()
            })
            .is_err()
    );
    assert!(
        store
            .save(&Note {
                span: Span { from: 20, to: 19 },
                ..note.clone()
            })
            .is_err()
    );
    assert!(store.reply(0, " \n ").is_err());
    assert!(store.reply(99, "no parent").is_err());
    store.reply(0, "First reply")?;
    store.reply(0, "Second reply")?;
    store.reply(1, "Other thread")?;
    drop(store);
    let reopened = Store::open(&path)?;
    assert_eq!(reopened.notes()?, vec![note]);
    assert_eq!(reopened.replies(0)?, vec!["First reply", "Second reply"]);
    assert_eq!(reopened.replies(1)?, vec!["Other thread"]);
    assert!(reopened.replies(4)?.is_empty());
    Ok(())
}
#[test]
fn fixture_matches_explicit_activity_contract() -> Result<()> {
    let dual = fixture(false)?;
    let solo = fixture(true)?;
    assert_eq!(dual.len(), 405);
    assert_eq!(solo.len(), 405);
    let start = parse_local("08:42:00.000")?;
    for (i, (d, s)) in dual.iter().zip(&solo).enumerate() {
        assert_eq!(d.time, start + i as i64 * 60_000);
        assert_eq!(s.time, d.time);
        assert_eq!(s.them, 0.);
        assert_eq!(d.me, s.me);
        let active = (i < 37 || (348..=404).contains(&i)) && i % 4 != 0;
        assert_eq!(
            d.me,
            if active {
                ((i * 17 % 61) + 20) as f64 / 100.
            } else {
                0.
            }
        );
        assert_eq!(
            d.them,
            if (348..=404).contains(&i) && i % 3 != 0 {
                ((i * 13 % 57) + 18) as f64 / 100.
            } else {
                0.
            }
        );
    }
    assert_eq!(
        extent(&dual)?,
        Span {
            from: start + 60_000,
            to: start + 404 * 60_000
        }
    );
    assert_eq!(
        extent(&solo)?,
        Span {
            from: start + 60_000,
            to: start + 403 * 60_000
        }
    );
    Ok(())
}

#[test]
fn activity_track_keeps_turns_silence_and_overlap_distinct() {
    let samples: Vec<_> = [(1., 0.), (1., 0.), (0., 1.), (0., 0.), (1., 1.)]
        .into_iter()
        .enumerate()
        .map(|(i, (me, them))| Sample {
            time: i as i64 * 60_000,
            me,
            them,
        })
        .collect();
    assert_eq!(
        activity_runs(&samples),
        vec![
            (
                Span {
                    from: 0,
                    to: 120_000
                },
                1
            ),
            (
                Span {
                    from: 120_000,
                    to: 180_000
                },
                2
            ),
            (
                Span {
                    from: 240_000,
                    to: 300_000
                },
                3
            ),
        ]
    );
}

#[test]
fn day_track_includes_the_last_sample_and_rounds_to_hours() -> Result<()> {
    let span = day_extent(&fixture(false)?)?;
    assert_eq!(local(span.from, false), "08:00");
    assert_eq!(local(span.to, false), "16:00");
    assert_eq!(day_extent(&fixture(true)?)?, span);
    Ok(())
}

#[test]
fn message_boundaries_include_exact_first_and_last_messages() {
    let messages: Vec<_> = [100, 200, 300]
        .into_iter()
        .enumerate()
        .map(|(i, time)| Utterance {
            id: i as i64,
            time,
            speaker: "Me".into(),
            text: "Example".into(),
        })
        .collect();
    let span = message_span(&messages, 1, 1).unwrap_or(Span { from: 0, to: 0 });
    assert_eq!(span, Span { from: 200, to: 201 });
    assert!(!span.contains(100));
    assert!(span.contains(200));
    assert!(!span.contains(300));
    assert_eq!(
        message_span(&messages, 0, 2),
        Some(Span { from: 100, to: 301 })
    );
    assert_eq!(message_span(&messages, 2, 1), None);
    assert_eq!(message_span(&messages, 0, 3), None);
}

#[test]
fn declarative_schema_upgrade_preserves_rows_and_is_repeatable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("upgrade.sqlite");
    reconcile_schema(
        &path,
        "CREATE TABLE records(id INTEGER PRIMARY KEY, item TEXT NOT NULL);",
    )?;
    let db = rusqlite::Connection::open(&path)?;
    db.execute("INSERT INTO records VALUES(1,'preserve me')", [])?;
    drop(db);
    let desired = "CREATE TABLE records(id INTEGER PRIMARY KEY, item TEXT NOT NULL, detail TEXT DEFAULT 'added');";
    reconcile_schema(&path, desired)?;
    reconcile_schema(&path, desired)?;
    let db = rusqlite::Connection::open(&path)?;
    let row: (String, String) =
        db.query_row("SELECT item,detail FROM records WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    assert_eq!(row, ("preserve me".into(), "added".into()));
    assert!(reconcile_schema(&path, "THIS IS NOT SQL").is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn pre_sqldef_spike_schema_upgrades_without_losing_messages() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("legacy.sqlite");
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch("CREATE TABLE utterances(id INTEGER PRIMARY KEY,time INTEGER NOT NULL,speaker TEXT NOT NULL,text TEXT NOT NULL); CREATE TABLE notes(id INTEGER PRIMARY KEY,start INTEGER NOT NULL,end INTEGER NOT NULL,label TEXT NOT NULL,body TEXT NOT NULL,CHECK(end>start)); INSERT INTO utterances VALUES(0,100,'Me','Keep this'); INSERT INTO notes VALUES(0,90,110,'Label','Body');")?;
    drop(db);
    let store = Store::open(&path)?;
    assert_eq!(store.utterances()?[0].text, "Keep this");
    assert_eq!(store.notes()?[0].body, "Body");
    drop(store);
    Store::open(&path)?;
    Ok(())
}

#[test]
fn editing_a_saved_span_updates_instead_of_duplicating_it() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("edit.sqlite");
    let mut store = Store::open(&path)?;
    store.seed()?;
    let original = Note {
        span: Span {
            from: parse_local("14:30:00.000")?,
            to: parse_local("14:32:00.000")?,
        },
        label: "Planning".into(),
        body: "Original".into(),
    };
    store.save(&original)?;
    for label in ["", " \t", "Untitled span", " Untitles Span "] {
        let invalid = Note {
            label: label.into(),
            ..original.clone()
        };
        assert!(store.save(&invalid).is_err());
        assert!(store.update_note(&original, &invalid).is_err());
        assert_eq!(store.notes()?, vec![original.clone()]);
    }
    let edited = Note {
        span: Span {
            from: parse_local("14:32:00.000")?,
            to: parse_local("14:34:00.000")?,
        },
        label: "Revised".into(),
        body: "Keep detail".into(),
    };
    store.update_note(&original, &edited)?;
    assert_eq!(store.notes()?, vec![edited]);
    let db = rusqlite::Connection::open(path)?;
    let links: i64 = db.query_row("SELECT count(*) FROM span_messages", [], |r| r.get(0))?;
    assert_eq!(links, 2);
    Ok(())
}

#[test]
fn captured_transcript_is_idempotent_persistent_and_links_existing_spans() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("captured.sqlite");
    let store = Store::open(&path)?;
    let start =
        chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")?.with_timezone(&chrono::Utc);
    let segment = team_up_gpui_spike::model::Segment {
        id: uuid::Uuid::new_v4(),
        source: "microphone".into(),
        start,
        end: start + chrono::Duration::seconds(3),
        text: "A real captured utterance".into(),
    };
    store.save(&Note {
        span: Span {
            from: start.timestamp_millis(),
            to: segment.end.timestamp_millis(),
        },
        label: "Decision".into(),
        body: String::new(),
    })?;
    assert!(store.ingest_segment(&segment)?.is_some());
    assert!(store.ingest_segment(&segment)?.is_none());
    drop(store);
    let store = Store::open(&path)?;
    assert_eq!(store.utterances()?.len(), 1);
    assert_eq!(store.utterances()?[0].text, segment.text);
    assert_eq!(
        store.recorded_activity()?,
        vec![(
            Span {
                from: start.timestamp_millis(),
                to: segment.end.timestamp_millis()
            },
            1
        )]
    );
    let db = rusqlite::Connection::open(path)?;
    let links: i64 = db.query_row("SELECT count(*) FROM span_messages", [], |r| r.get(0))?;
    assert_eq!(links, 1);
    Ok(())
}
#[test]
fn real_source_activity_preserves_true_overlaps_and_silence() {
    use team_up_gpui_spike::combine_activity;
    assert_eq!(
        combine_activity(&[
            (Span { from: 0, to: 10 }, 1),
            (Span { from: 5, to: 15 }, 2),
            (Span { from: 20, to: 25 }, 1)
        ]),
        vec![
            (Span { from: 0, to: 5 }, 1),
            (Span { from: 5, to: 10 }, 3),
            (Span { from: 10, to: 15 }, 2),
            (Span { from: 20, to: 25 }, 1)
        ]
    );
    assert_eq!(
        combine_activity(&[(Span { from: 0, to: 10 }, 1), (Span { from: 5, to: 15 }, 1)]),
        vec![(Span { from: 0, to: 15 }, 1)]
    );
}

#[test]
fn timeline_notes_and_breakpoints_are_independent_and_survive_restart() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("notes.sqlite");
    let mut store = Store::open(&path)?;
    store.seed()?;
    let messages = store.utterances()?;
    let last = messages
        .last()
        .ok_or_else(|| anyhow::anyhow!("Missing fixture"))?;
    let time = last.time + 1;
    assert!(store.add_timeline_entry(time, "  ", false).is_err());
    store.add_timeline_entry(time, " A useful note ", false)?;
    store.add_timeline_entry(time + 1, "", true)?;
    store.add_timeline_entry(time + 2, "Next topic", true)?;
    // A standalone note on a later day cannot accidentally attach to yesterday.
    store.add_timeline_entry(time + 86_400_000, "New day", false)?;
    assert!(store.notes()?.is_empty());
    drop(store);
    let store = Store::open(&path)?;
    let entries = store.timeline_entries()?;
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].body, "A useful note");
    assert_eq!(entries[0].utterance_id, Some(last.id));
    assert!(!entries[0].breakpoint);
    assert!(entries[1].breakpoint);
    assert!(entries[1].body.is_empty());
    assert_eq!(entries[2].body, "Next topic");
    assert_eq!(entries[3].utterance_id, None);
    assert_eq!(store.utterances()?.len(), messages.len());
    Ok(())
}

#[test]
fn day_minimap_advances_without_speech_and_handles_dst() -> Result<()> {
    use team_up_gpui_spike::{day_time_span, time_at_timeline_y, timeline_y_at};
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:34:56+01:00")?.timestamp_millis();
    let day =
        chrono::NaiveDate::from_ymd_opt(2026, 11, 10).ok_or_else(|| anyhow::anyhow!("Date"))?;
    let live = day_time_span(day, now);
    assert_eq!(live.to, now);
    assert_eq!(day_time_span(day, now + 1000).to, now + 1000);
    assert_eq!(day_time_span(day, now + 1000).from, live.from);
    for (date, hours) in [("2026-03-29", 23), ("2026-10-25", 25), ("2026-11-09", 24)] {
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
        let span = day_time_span(date, now);
        assert_eq!(span.to - span.from, hours * 3_600_000);
    }
    let span = Span { from: 0, to: 1000 };
    let anchors = [(200, 20.), (500, 80.), (900, 160.)];
    assert_eq!(time_at_timeline_y(&anchors, 50., 0., 200., span), 350);
    assert_eq!(timeline_y_at(&anchors, 350), Some(50.));
    assert_eq!(span.at(0.25), 250);
    Ok(())
}

#[test]
fn connected_breakpoints_keep_comments_and_labels_independent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ranges.sqlite");
    let store = Store::open(&path)?;
    let time =
        chrono::DateTime::parse_from_rfc3339("2026-10-14T10:15:00+02:00")?.timestamp_millis();
    let a = store.add_timeline_entry(time, "", true)?;
    let b = store.add_timeline_entry(time + 900_000, "", true)?;
    let comment = store.add_timeline_entry(time + 100_000, "Historical note", false)?;
    let note = Note {
        span: Span {
            from: time,
            to: time + 900_000,
        },
        label: "Research".into(),
        body: String::new(),
    };
    store.save_connected_span(&note, b, a)?;
    assert_eq!(store.timeline_entries()?.len(), 3);
    store.update_breakpoint_label(a, " Beginning ")?;
    assert!(store.update_breakpoint_label(comment, "Invalid").is_err());
    drop(store);
    let store = Store::open(&path)?;
    assert_eq!(store.notes()?.len(), 1);
    assert_eq!(store.timeline_entries()?.len(), 3);
    assert_eq!(store.timeline_entries()?[0].label, "Beginning");
    let refined = Note {
        span: Span {
            from: time,
            to: time + 1_200_000,
        },
        ..note.clone()
    };
    store.update_note(&note, &refined)?;
    assert_eq!(
        store
            .timeline_entries()?
            .iter()
            .find(|entry| entry.id == b)
            .map(|entry| entry.time),
        Some(time + 1_200_000)
    );
    store.remove_range(&refined)?;
    assert!(store.notes()?.is_empty());
    assert_eq!(store.timeline_entries()?.len(), 3);
    assert_eq!(
        store
            .timeline_entries()?
            .iter()
            .find(|entry| entry.id == comment)
            .map(|entry| entry.body.as_str()),
        Some("Historical note")
    );
    Ok(())
}

#[test]
fn legacy_spans_become_linked_breakpoints_without_losing_data() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("legacy-ranges.sqlite");
    let old = include_str!("../schema.sql")
        .split("CREATE TABLE span_breakpoints")
        .next()
        .ok_or_else(|| anyhow::anyhow!("Schema"))?
        .replace("    \"label\" TEXT NOT NULL DEFAULT '',\n", "");
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch(&old)?;
    db.execute(
        "INSERT INTO notes(start,end,label,body) VALUES(100,200,'Existing range','Keep me')",
        [],
    )?;
    drop(db);
    for _ in 0..2 {
        let store = Store::open(&path)?;
        assert_eq!(store.notes()?.len(), 1);
        assert_eq!(store.notes()?[0].body, "Keep me");
        assert_eq!(store.timeline_entries()?.len(), 2);
        assert!(
            store
                .timeline_entries()?
                .iter()
                .all(|entry| entry.breakpoint && entry.label.is_empty())
        );
    }
    Ok(())
}

#[test]
fn range_gesture_distinguishes_clicks_and_marks_both_directions() {
    let mut click = RangeGesture::new(100, 20.);
    assert_eq!(click.update(500, 23.), None);
    assert!(!click.is_marking());
    assert_eq!(click.update(500, 24.), Some(Span { from: 100, to: 500 }));
    assert!(click.is_marking());
    assert_eq!(click.update(50, 19.), Some(Span { from: 50, to: 100 }));
}

#[test]
fn native_activity_survives_restart_without_truncation_or_day_leakage() -> Result<()> {
    use activity::{ActivityPayload, CaptureMessage, parse_message};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("activity.sqlite");
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
    let start = day_time_span(day, 0).from;
    let end = day_time_span(day.succ_opt().unwrap(), 0).from;
    assert_eq!(end - start, 25 * 3_600_000);
    let raw = serde_json::json!({"type":"activity", "event_id":"native-1", "time":end-1,
        "application":"Browser", "action":"accessibility", "url":"https://example.org/path?q=full#fragment",
        "metadata":{"AXValue":"long text ".repeat(10000),"AXSelectedText":"all selected text", "unavailable":{"code":-25204}},
        "future_os_field":{"values":[1,true,null,"unfiltered"]}});
    let CaptureMessage::Activity(event) = parse_message(&raw.to_string())? else {
        panic!("expected activity")
    };
    let store = Store::open(&path)?;
    assert!(store.ingest_activity(event.clone())?.is_some());
    assert!(store.ingest_activity(event.clone())?.is_none());
    store.ingest_activity(ActivityPayload {
        event_id: "earlier".into(),
        time: start,
        ..event.clone()
    })?;
    store.ingest_activity(ActivityPayload {
        event_id: "next-day".into(),
        time: end,
        ..event.clone()
    })?;
    drop(store);
    let store = Store::open(&path)?;
    let entries = store.activity_for_day(day)?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].event.time, start);
    assert_eq!(*entries[1].event, event);
    assert_eq!(
        serde_json::to_value(entries[1].event.as_ref())?["future_os_field"],
        raw["future_os_field"]
    );
    assert_eq!(store.activity_for_day(day.succ_opt().unwrap())?.len(), 1);
    assert!(store.activity_for_day(day.pred_opt().unwrap())?.is_empty());
    Ok(())
}

#[test]
fn streaming_speech_keeps_its_position_around_breakpoints_when_finalized() -> Result<()> {
    let earlier = Utterance {
        id: 1,
        time: 100,
        speaker: "Them".into(),
        text: "Earlier speech".into(),
    };
    let me = LiveUtterance {
        source: "microphone".into(),
        time: 200,
        text: "In progress".into(),
    };
    let them = LiveUtterance {
        source: "system audio".into(),
        time: 400,
        text: "Other speaker".into(),
    };
    let drafts = vec![them.clone(), me.clone()];
    let before = ordered_transcript_bubbles(&[earlier.clone()], &drafts);
    assert_eq!(
        before
            .iter()
            .map(|row| (row.time, row.speaker.as_str()))
            .collect::<Vec<_>>(),
        vec![(100, "Them"), (200, "Me"), (400, "Them")]
    );
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("breakpoint.sqlite");
    let store = Store::open(&path)?;
    store.add_timeline_entry(300, "", true)?;
    store.add_timeline_entry(500, "Note after both speakers", false)?;
    let annotations = store.timeline_entries()?;
    let relative_positions = |rows: &[TranscriptBubble]| {
        annotations
            .iter()
            .map(|entry| rows.partition_point(|row| row.time < entry.time))
            .collect::<Vec<_>>()
    };
    // Finals may arrive in reverse order across the two capture streams.
    let them_final = Utterance {
        id: 2,
        time: them.time,
        speaker: "Them".into(),
        text: "Other speaker completed".into(),
    };
    let during = ordered_transcript_bubbles(
        &[earlier.clone(), them_final.clone()],
        std::slice::from_ref(&me),
    );
    assert_eq!(relative_positions(&before), relative_positions(&during));
    let me_final = Utterance {
        id: 3,
        time: me.time,
        speaker: "Me".into(),
        text: "In progress, now completed".into(),
    };
    let after = ordered_transcript_bubbles(&[them_final, me_final, earlier], &[]);
    assert_eq!(relative_positions(&before), relative_positions(&after));
    assert_eq!(
        after.iter().map(|row| row.time).collect::<Vec<_>>(),
        vec![100, 200, 400]
    );
    assert!(after.iter().all(|row| row.utterance_id.is_some()));
    drop(store);
    let saved = Store::open(&path)?.timeline_entries()?;
    assert_eq!(saved[0].time, 300);
    assert_eq!(saved[1].time, 500);
    Ok(())
}

#[test]
fn live_delta_growth_does_not_reorder_timeline_rows() {
    let mut drafts = vec![
        LiveUtterance {
            source: "microphone".into(),
            time: 200,
            text: "A few words".into(),
        },
        LiveUtterance {
            source: "system audio".into(),
            time: 200,
            text: "Simultaneous speech".into(),
        },
    ];
    let before = ordered_transcript_bubbles(&[], &drafts)
        .into_iter()
        .map(|row| (row.time, row.source))
        .collect::<Vec<_>>();
    drafts[0].text.push_str(&" and more words".repeat(50));
    drafts.reverse();
    let after = ordered_transcript_bubbles(&[], &drafts)
        .into_iter()
        .map(|row| (row.time, row.source))
        .collect::<Vec<_>>();
    assert_eq!(before, after);
}
