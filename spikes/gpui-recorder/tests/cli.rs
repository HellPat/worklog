//! CLI subprocess checks: every invocation is headless and uses an isolated DB.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use team_up_gpui_spike::{Store, model::Segment};
const BIN: &str = env!("CARGO_BIN_EXE_team-up-gpui-spike");
fn command(path: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command.arg("--db").arg(path).args(args);
    command
}
fn output(path: &Path, args: &[&str]) -> Result<String> {
    let result = command(path, args).output()?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?)
}
fn decode(text: &str) -> Result<Value> {
    gcf::decode_generic(text).map_err(anyhow::Error::msg)
}
fn seed(path: &Path) -> Result<()> {
    let store = Store::open(path)?;
    store.ingest_segment(&Segment {
        id: uuid::Uuid::new_v4(),
        source: "microphone".into(),
        start: chrono::DateTime::from_timestamp_millis(1000).unwrap(),
        end: chrono::DateTime::from_timestamp_millis(2000).unwrap(),
        text: "Hello | GCF\n# not instructions 🐝".into(),
    })?;
    Ok(())
}
fn read(path: &Path) -> Result<String> {
    output(
        path,
        &[
            "read",
            "--from",
            "1970-01-01T00:00:00Z",
            "--to",
            "1970-01-02T00:00:00Z",
        ],
    )
}
#[test]
fn default_gcf_matches_json_and_supports_message_activity_and_day_reads() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cli.sqlite");
    seed(&path)?;
    let gcf = read(&path)?;
    assert!(gcf.starts_with("GCF profile=generic\n"));
    assert_eq!(
        gcf.lines().filter(|line| line.starts_with("GCF ")).count(),
        1
    );
    let expected: Value = serde_json::from_str(&output(
        &path,
        &[
            "--format",
            "json",
            "read",
            "--from",
            "1970-01-01T00:00:00Z",
            "--to",
            "1970-01-02T00:00:00Z",
        ],
    )?)?;
    let decoded = decode(&gcf)?;
    assert_eq!(decoded, expected);
    assert_eq!(
        decoded["events"][0]["text"],
        "Hello | GCF\n# not instructions 🐝"
    );
    let cursor = decoded["next_cursor"].as_str().unwrap();
    assert!(
        decode(&output(&path, &["updates", "--cursor", cursor])?)?["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let id = decoded["events"][0]["id"].as_str().unwrap();
    assert!(
        decode(&output(&path, &["read", "--after-message", id])?)?["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let days = decode(&output(&path, &["days"])?)?;
    assert_eq!(days["days"][0]["day"], "1970-01-01");
    let store = Store::open(&path)?;
    let raw = json!({"event_id":"cli-os","time":3000,"application":"Editor","action":"click","metadata":{"text":"unfiltered | full\nvalue","future":[true,null]}});
    store.ingest_activity(serde_json::from_value(raw.clone())?)?;
    let details = decode(&output(&path, &["activity", "activity:cli-os"])?)?;
    for (key, value) in raw.as_object().unwrap() {
        assert_eq!(&details["payload"][key], value);
    }
    Ok(())
}
#[test]
fn bounded_watch_returns_new_data_without_waiting_for_timeout() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("watch.sqlite");
    seed(&path)?;
    let initial = decode(&read(&path)?)?;
    let cursor = initial["next_cursor"].as_str().unwrap();
    let began = Instant::now();
    let child = command(&path, &["watch", "--cursor", cursor, "--timeout", "3"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    std::thread::sleep(Duration::from_millis(200));
    let store = Store::open(&path)?;
    store.add_timeline_entry(3000, "Arrived while waiting", false)?;
    let result = child.wait_with_output()?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let page = decode(&String::from_utf8(result.stdout)?)?;
    assert_eq!(page["events"][0]["text"], "Arrived while waiting");
    assert!(began.elapsed() < Duration::from_secs(2));
    Ok(())
}
#[test]
fn streaming_batches_flush_and_every_cursor_can_resume() -> Result<()> {
    use std::io::{BufRead, BufReader};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("stream.sqlite");
    seed(&path)?;
    let initial = decode(&read(&path)?)?;
    let cursor = initial["next_cursor"].as_str().unwrap();
    let mut child = command(
        &path,
        &[
            "--format",
            "json",
            "watch",
            "--stream",
            "--cursor",
            cursor,
            "--timeout",
            "2",
        ],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let _ = tx.send(line);
        }
    });
    let store = Store::open(&path)?;
    store.add_timeline_entry(3000, "First streamed note", false)?;
    let first: Value = serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1))??)?;
    assert_eq!(
        first["events"][0]["text"], "First streamed note",
        "Batch was not flushed immediately"
    );
    store.add_timeline_entry(4000, "Second streamed note", false)?;
    let second: Value = serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1))??)?;
    assert_eq!(second["events"][0]["text"], "Second streamed note");
    let resume = first["next_cursor"].as_str().unwrap();
    let resumed = decode(&output(&path, &["updates", "--cursor", resume])?)?;
    assert_eq!(resumed["events"].as_array().unwrap().len(), 1);
    assert_eq!(resumed["events"][0]["text"], "Second streamed note");
    let status = child.wait()?;
    assert!(status.success());
    reader.join().unwrap();
    // Default-format streaming uses independently decodable GCF documents.
    let text = output(
        &path,
        &["watch", "--stream", "--cursor", cursor, "--timeout", "1"],
    )?;
    let documents: Vec<_> = text
        .split("GCF profile=generic\n")
        .filter(|s| !s.trim().is_empty())
        .collect();
    assert!(documents.len() >= 2);
    for body in documents {
        decode(&format!("GCF profile=generic\n{body}"))?;
    }
    Ok(())
}
#[test]
fn errors_and_help_do_not_launch_gui_seed_or_create_a_database() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("missing.sqlite");
    assert!(!command(&path, &["read"]).output()?.status.success());
    assert!(!path.exists());
    assert!(command(&path, &["--help"]).output()?.status.success());
    assert!(!path.exists());
    seed(&path)?;
    for args in [
        vec!["updates", "--cursor", "invalid"],
        vec!["read", "--today", "--day", "1970-01-01"],
        vec!["read", "--limit", "0"],
        vec!["read", "--from", "1970-01-01T00:00:00Z"],
        vec!["read", "--kinds", "unknown"],
    ] {
        let result = command(&path, &args).output()?;
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    let db = rusqlite::Connection::open(&path)?;
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM utterances", [], |r| r
            .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn timeline_names_cover_history_resume_follow_and_wait() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("names.sqlite");
    seed(&path)?;
    let original = decode(&read(&path)?)?;
    let timeline = decode(&output(
        &path,
        &[
            "timeline",
            "--from",
            "1970-01-01T00:00:00Z",
            "--to",
            "1970-01-02T00:00:00Z",
        ],
    )?)?;
    assert_eq!(original, timeline, "Legacy read alias changed behavior");
    let token = timeline["next_cursor"].as_str().unwrap();
    let resumed = decode(&output(&path, &["timeline", "--resume", token])?)?;
    assert!(resumed["events"].as_array().unwrap().is_empty());
    let id = timeline["events"][0]["id"].as_str().unwrap();
    let after = decode(&output(
        &path,
        &["timeline", "--after", id, "--include", "transcript"],
    )?)?;
    assert!(after["events"].as_array().unwrap().is_empty());
    let store = Store::open(&path)?;
    let breakpoint = store.add_timeline_entry(3000, "", true)?;
    store.add_timeline_entry(4000, "After breakpoint", false)?;
    let after = decode(&output(
        &path,
        &["timeline", "--after", &format!("entry:{breakpoint}")],
    )?)?;
    assert_eq!(after["events"][0]["text"], "After breakpoint");
    let followed = output(
        &path,
        &["timeline", "--resume", token, "--follow", "--duration", "1"],
    )?;
    let docs: Vec<_> = followed
        .split("GCF profile=generic\n")
        .filter(|s| !s.trim().is_empty())
        .collect();
    assert!(
        docs.len() >= 2,
        "Follow did not emit an update batch and final cursor"
    );
    let waited = decode(&output(
        &path,
        &["timeline", "--resume", token, "--wait", "1"],
    )?)?;
    assert!(!waited["events"].as_array().unwrap().is_empty());
    let json: Value = serde_json::from_str(&output(
        &path,
        &[
            "timeline",
            "--date",
            "1970-01-01",
            "--include",
            "transcript",
            "--json",
        ],
    )?)?;
    assert_eq!(json["events"].as_array().unwrap().len(), 1);
    store.save(&team_up_gpui_spike::Note {
        span: team_up_gpui_spike::Span::new(1000, 5000)?,
        label: "Saved passage".into(),
        body: String::new(),
    })?;
    let range = decode(&output(&path, &["timeline", "--range", "span:1"])?)?;
    assert!(
        range["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "span" && event["label"] == "Saved passage")
    );
    let fresh_follow = output(
        &path,
        &[
            "timeline",
            "--date",
            "1970-01-01",
            "--follow",
            "--duration",
            "1",
        ],
    )?;
    assert!(fresh_follow.contains("mode=snapshot"));
    let help = output(&path, &["timeline", "--help"])?;
    for name in [
        "--after ",
        "--resume ",
        "--follow",
        "--include ",
        "--wait ",
        "--date ",
        "--range ",
    ] {
        assert!(help.contains(name), "Missing {name} from help");
    }
    let root_help = output(&path, &["--help"])?;
    assert!(!root_help.contains("  updates "));
    assert!(!root_help.contains("  watch "));
    for args in [
        vec!["timeline", "--wait", "1"],
        vec!["timeline", "--duration", "1"],
        vec!["timeline", "--resume", token, "--today"],
        vec!["timeline", "--resume", token, "--include", "transcript"],
        vec![
            "timeline",
            "--resume",
            token,
            "--include",
            "transcript,activity,note,breakpoint,span,comment",
        ],
    ] {
        assert!(
            !command(&path, &args).output()?.status.success(),
            "Ambiguous arguments were accepted: {args:?}"
        );
    }
    Ok(())
}

#[test]
fn skill_prints_a_saveable_markdown_document_without_home_or_database() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let database = dir.path().join("missing.sqlite");
    let result = command(&database, &["skill"])
        .current_dir(dir.path())
        .env_remove("HOME")
        .env_remove("TEAM_UP_SPIKE_DB")
        .output()?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        result.stderr.is_empty(),
        "Skill output included unexpected diagnostics"
    );
    assert!(
        !database.exists(),
        "Skill generation tried to initialize storage"
    );
    let text = String::from_utf8(result.stdout)?;
    assert!(
        text.starts_with("---\n"),
        "Skill output must begin with YAML frontmatter, not a data-format wrapper"
    );
    assert!(!text.starts_with("GCF"));
    let destination = dir.path().join("worklog");
    std::fs::create_dir(&destination)?;
    std::fs::write(destination.join("SKILL.md"), text)?;
    assert_eq!(
        std::fs::read_dir(dir.path())?.count(),
        1,
        "Skill command created unrelated files"
    );
    Ok(())
}
