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
