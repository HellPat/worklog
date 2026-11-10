#![deny(clippy::unwrap_used, clippy::expect_used)]
use anyhow::{Context, Result, ensure};
use cucumber::{World, given, then, when};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

const APP: &str = match option_env!("WORKLOG_UI_APP") {
    Some(name) => name,
    None => "team-up-gpui-spike",
};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
#[derive(Debug, Default, World)]
struct Desktop {
    child: Option<Child>,
    folder: PathBuf,
    label: String,
}
fn stop_test_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(format!("-{}", child.id()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
impl Drop for Desktop {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            stop_test_child(&mut child);
        }
    }
}
impl Desktop {
    // Ordinary CLI invocation/JSON handling; the existing tool owns all OS input and locators.
    fn command(&self, args: &[&str]) -> Result<Value> {
        let output = Command::new(root().join("tools/node_modules/.bin/agent-desktop"))
            .env(
                "AGENT_DESKTOP_HOME",
                root().join("evidence/agent-desktop-state"),
            )
            .args(args)
            .output()?;
        let body = String::from_utf8(output.stdout)?;
        if !self.folder.as_os_str().is_empty() {
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.folder.join("commands.jsonl"))?;
            writeln!(
                log,
                "{}",
                serde_json::json!({"args":args,"response":body,"stderr":String::from_utf8_lossy(&output.stderr)})
            )?;
        }
        let response: Value = serde_json::from_str(&body).context("CLI did not return JSON")?;
        ensure!(
            output.status.success() && response["ok"] == true,
            "agent-desktop refused or failed: {response}"
        );
        Ok(response["data"].clone())
    }
    fn snapshot(&self) -> Result<Value> {
        let data = self.command(&["snapshot", "--app", APP, "--include-bounds"])?;
        ensure!(data["complete"] == true, "Incomplete accessibility tree");
        Ok(data)
    }
    fn named(&self, name: &str) -> Result<Value> {
        fn walk(v: &Value, name: &str, out: &mut Vec<Value>) {
            if v["name"].as_str() == Some(name) {
                out.push(v.clone());
            }
            if let Some(children) = v["children"].as_array() {
                for c in children {
                    walk(c, name, out);
                }
            }
        }
        let tree = self.snapshot()?;
        let mut matches = vec![];
        walk(&tree["tree"], name, &mut matches);
        ensure!(
            matches.len() == 1,
            "Expected exactly one {name}, got {}",
            matches.len()
        );
        matches.pop().context("Missing element")
    }
    fn reference(&self, name: &str) -> Result<String> {
        self.named(name)?["ref_id"]
            .as_str()
            .map(String::from)
            .context("Element is not addressable")
    }
    fn click(&self, name: &str) -> Result<()> {
        let reference = self.reference(name)?;
        self.command(&["--headed", "click", &reference])?;
        Ok(())
    }
    fn wait(&self, text: &str) -> Result<()> {
        self.command(&["wait", "--app", APP, "--text", text, "--timeout", "10000"])?;
        Ok(())
    }
    fn set_field(&self, name: &str, text: &str) -> Result<()> {
        let reference = self.reference(name)?;
        // AXValue is delivered asynchronously. Observe its result without replaying
        // a mutation when the driver's immediate verification races the UI thread.
        let delivery = self.command(&["set-value", &reference, text]);
        self.command(&["--headed", "focus-window", "--app", APP])?;
        let current = self.reference(name)?;
        let observed = self.command(&[
            "wait",
            "--element",
            &current,
            "--predicate",
            "value",
            "--value",
            text,
            "--timeout",
            "10000",
        ]);
        if observed.is_err() {
            delivery?;
        }
        observed?;
        Ok(())
    }
    fn launch(&mut self) -> Result<()> {
        let log = File::create(self.folder.join("app.log"))?;
        let audio_path = root().join("evidence/audio-transcript.json");
        if self.folder.ends_with("audio") {
            ensure!(
                audio_path.exists(),
                "Run the real audio fixture preparation first"
            );
        }
        let mut command = Command::new(
            std::env::var_os("WORKLOG_UI_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_team-up-gpui-spike"))),
        );
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        self.child = Some(
            command
                .env("TEAM_UP_SPIKE_DB", self.folder.join("fixture.sqlite"))
                .env(
                    "TEAM_UP_SPIKE_TRANSCRIPT",
                    if self.folder.ends_with("audio") {
                        audio_path
                    } else {
                        PathBuf::new()
                    },
                )
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .spawn()?,
        );
        self.command(&[
            "wait",
            "--app",
            APP,
            "--window",
            "Worklog — sample data",
            "--timeout",
            "10000",
        ])?;
        self.wait("Hide sidebar")?;
        self.command(&["--headed", "focus-window", "--app", APP])?;
        Ok(())
    }
}
#[given(expr = "a fresh launched fixture for {string}")]
fn launched(world: &mut Desktop, case: String) -> Result<()> {
    let run = std::env::var("SPIKE_RUN").unwrap_or_else(|_| "run-1".into());
    world.folder = root().join("evidence").join(run).join(&case);
    ensure!(
        !world.folder.exists(),
        "Use a new SPIKE_RUN name; preserving earlier evidence"
    );
    fs::create_dir_all(&world.folder)?;
    world.label = format!("Planning {case}");
    world.launch()
}
#[when("I start and stop the recording simulation")]
fn toggle(world: &mut Desktop) -> Result<()> {
    semantic(world, "2026-11-10 · 0 spoken words")?;
    world.wait("Start recording")?;
    world.click("Start recording")?;
    world.wait("Stop recording")?;
    world.click("Stop recording")?;
    world.wait("Start recording")?;
    {
        let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
        let mut statement = db.prepare("SELECT text FROM utterances")?;
        let words = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .map(|s| s.split_whitespace().count())
            .sum::<usize>();
        semantic(world, &format!("2026-11-09 · {words} spoken words"))?;
    }
    Ok(())
}
#[when("I open and close Settings")]
fn settings(world: &mut Desktop) -> Result<()> {
    world.click("Settings")?;
    world.wait("Engine: Nemotron")?;
    world.click("Back")?;
    world.command(&[
        "snapshot",
        "--app",
        APP,
        "--wait-for-gone",
        "button:Engine: Nemotron",
        "--wait-timeout",
        "5000",
    ])?;
    Ok(())
}
#[when(expr = "I drag a range on the {string}")]
fn select(world: &mut Desktop, region: String) -> Result<()> {
    world.command(&["--headed", "focus-window", "--app", APP])?;
    let node = world.named(&region)?;
    let b = &node["bounds"];
    let x = b["x"].as_f64().context("No x")?;
    let y = b["y"].as_f64().context("No y")?;
    let w = b["width"].as_f64().context("No width")?;
    let (from, to) = if region == "Voice activity chart" {
        (
            format!("{},{}", (x + w * 0.86).round(), y + 50.),
            format!("{},{}", (x + w * 0.96).round(), y + 50.),
        )
    } else {
        (
            format!("{},{}", x + w / 2., y + 15.),
            format!("{},{}", x + w / 2., y + 220.),
        )
    };
    world.command(&[
        "--headed",
        "drag",
        "--from-xy",
        &from,
        "--to-xy",
        &to,
        "--duration",
        "600",
        "--drop-delay",
        "200",
    ])?;
    world.wait("Save annotation")?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("popup.png")
            .to_str()
            .context("Path encoding")?,
    ])?;
    Ok(())
}
#[when("I enter and save an annotation")]
fn save(world: &mut Desktop) -> Result<()> {
    for (name, text) in [
        ("Annotation label", world.label.as_str()),
        ("Annotation note", "Keep this passage local"),
    ] {
        let reference = world.reference(name)?;
        world.command(&["set-value", &reference, text])?;
        let fresh = world.reference(name)?;
        world.command(&[
            "wait",
            "--element",
            &fresh,
            "--predicate",
            "value",
            "--value",
            text,
            "--timeout",
            "5000",
        ])?;
    }
    world.click("Save annotation")?;
    world.wait("◇ Label span")?;
    Ok(())
}
#[then("the note is displayed and stored in SQLite")]
fn persisted(world: &mut Desktop) -> Result<()> {
    world.wait(&world.label)?;
    let db = rusqlite::Connection::open_with_flags(
        world.folder.join("fixture.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (label, body, start, end): (String, String, i64, i64) =
        db.query_row("SELECT label,body,start,end FROM notes", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
    ensure!(
        label == world.label && body == "Keep this passage local" && start < end,
        "Incorrect persisted annotation"
    );

    let count: i64 = db.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?;
    ensure!(count == 1, "Duplicate annotation");
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("saved.png")
            .to_str()
            .context("Path encoding")?,
    ])?;
    Ok(())
}
#[then("the note remains displayed after reopening the app")]
fn reopen(world: &mut Desktop) -> Result<()> {
    if let Some(mut child) = world.child.take() {
        child.kill()?;
        child.wait()?;
    }
    world.launch()?;
    world.wait(&world.label)?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("reopened.png")
            .to_str()
            .context("Path encoding")?,
    ])?;
    Ok(())
}
fn main() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(
        Desktop::cucumber()
            .max_concurrent_scenarios(1)
            .after(|_, _, _, _, world| {
                Box::pin(async move {
                    if let Some(world) = world {
                        if let Some(mut child) = world.child.take() {
                            stop_test_child(&mut child);
                        }
                    }
                })
            })
            .fail_on_skipped()
            .run_and_exit(root().join("features")),
    );
    Ok(())
}

#[when("I adjust the span using the timeline")]
fn refine(world: &mut Desktop) -> Result<()> {
    world.wait("Annotation label")?;
    select(world, "Voice activity chart".into())?;
    ensure!(
        world.reference("Start at message 4").is_err(),
        "Removed transcript controls returned"
    );
    world.wait("Annotation label")?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("refined.png")
            .to_str()
            .context("Path encoding")?,
    ])?;
    Ok(())
}

#[when("I start and end a span without details")]
fn start_blank(world: &mut Desktop) -> Result<()> {
    world.click("＋ Start span")?;
    world.wait("New span label")?;
    world.wait("New span description")?;
    world.click("Start without details")?;
    world.wait("■ End span")?;
    world.click("■ End span")?;
    world.wait("Annotation label")?;
    Ok(())
}
#[then("the unnamed span requires a label before saving")]
fn blank_saved(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open_with_flags(
        world.folder.join("fixture.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let count: i64 = db.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?;
    ensure!(count == 0, "Unnamed span was saved");
    ensure!(
        world.named("Annotation label")?["value"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "A fallback label was prefilled"
    );
    let button = world.named("Save annotation")?;
    ensure!(
        button["enabled"] == false
            || !button["available_actions"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|x| x == "Click")),
        "Unnamed save is actionable"
    );
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("footer.png")
            .to_str()
            .context("Path encoding")?,
    ])?;
    Ok(())
}

#[when("I comment on the first message")]
fn comment(world: &mut Desktop) -> Result<()> {
    world.click("Comment on message 1")?;
    let reference = world.reference("Reply")?;
    world.command(&["set-value", &reference, "Useful context"])?;
    world.click("Add comment")?;
    world.wait("Useful context")?;
    Ok(())
}
#[then("the comment survives reopening")]
fn comment_reopen(world: &mut Desktop) -> Result<()> {
    if let Some(mut child) = world.child.take() {
        child.kill()?;
        child.wait()?;
    }
    world.launch()?;
    world.click("Comment on message 1")?;
    world.wait("Useful context")?;
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    let count: i64 = db.query_row(
        "SELECT count(*) FROM replies WHERE utterance=0 AND body='Useful context'",
        [],
        |r| r.get(0),
    )?;
    ensure!(count == 1, "Comment did not persist");
    Ok(())
}
#[when("I label the imported audio transcript")]
fn label_audio(world: &mut Desktop) -> Result<()> {
    world.click("◇ Label span")?;
    world.wait("Annotation label")?;
    world.named("Voice activity chart")?;
    Ok(())
}
#[then("the audio transcript and span are independently stored and linked")]
fn audio_linked(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    let transcript: Vec<(i64, String)> = db
        .prepare("SELECT time,text FROM utterances ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let original: Value =
        serde_json::from_slice(&fs::read(root().join("evidence/audio-transcript.json"))?)?;
    ensure!(
        transcript.len() == original.as_array().context("Audio array")?.len(),
        "Transcript changed"
    );
    for (i, (time, text)) in transcript.iter().enumerate() {
        ensure!(
            original[i]["time"] == *time && original[i]["text"] == *text,
            "ASR transcript changed"
        );
    }
    let links: i64 = db.query_row("SELECT count(*) FROM span_messages", [], |r| r.get(0))?;
    ensure!(links > 0, "Span missing explicit transcript links");
    db.execute_batch("PRAGMA foreign_keys=ON; SAVEPOINT independence; DELETE FROM notes;")?;
    let count: i64 = db.query_row("SELECT count(*) FROM utterances", [], |r| r.get(0))?;
    ensure!(
        count == transcript.len() as i64,
        "Deleting span removed transcript"
    );
    db.execute_batch("ROLLBACK TO independence; RELEASE independence;")?;
    Ok(())
}

#[when("I start and end a named span")]
fn named_span(world: &mut Desktop) -> Result<()> {
    world.click("＋ Start span")?;
    for (field, value) in [
        ("New span label", "Planning"),
        ("New span description", "Agree next steps"),
    ] {
        let reference = world.reference(field)?;
        world.command(&["set-value", &reference, value])?;
    }
    world.click("Start span")?;
    world.wait("■ End span")?;
    world.click("■ End span")?;
    world.wait("Open span Planning")?;
    Ok(())
}
#[then("the running span details are stored")]
fn named_stored(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    let (label, body): (String, String) =
        db.query_row("SELECT label,body FROM notes", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    ensure!(
        label == "Planning" && body == "Agree next steps",
        "Running span details lost"
    );
    Ok(())
}
#[when("I keep my draft through Settings and adjust the timeline")]
fn draft_navigation(world: &mut Desktop) -> Result<()> {
    let reference = world.reference("Annotation label")?;
    world.command(&["set-value", &reference, "Draft"])?;
    world.click("Settings")?;
    world.click("Back")?;
    world.wait("Annotation label")?;
    let reference = world.reference("Annotation label")?;
    world.command(&[
        "wait",
        "--element",
        &reference,
        "--predicate",
        "value",
        "--value",
        "Draft",
        "--timeout",
        "5000",
    ])?;
    select(world, "Voice activity chart".into())?;
    world.wait("Annotation label")?;
    Ok(())
}

fn semantic(world: &Desktop, name: &str) -> Result<()> {
    let reference = world.reference(name)?;
    world.command(&["click", &reference])?;
    Ok(())
}
#[when("I collapse and reopen the sidebar and choose an empty date")]
fn sidebar_navigation(world: &mut Desktop) -> Result<()> {
    semantic(world, "Hide sidebar")?;
    world.wait("Show sidebar")?;
    semantic(world, "Show sidebar")?;
    world.wait("Hide sidebar")?;
    semantic(world, "2026-11-10 · 0 spoken words")?;
    Ok(())
}
#[then("the selected date contains no transcript messages")]
fn empty_date(world: &mut Desktop) -> Result<()> {
    world.wait("No transcript for this day")?;
    ensure!(
        world.reference("Comment on message 1").is_err(),
        "Another day's transcript remained visible"
    );
    Ok(())
}
#[when("I reset a marked span")]
fn reset_span(world: &mut Desktop) -> Result<()> {
    semantic(world, "◇ Label span")?;
    world.wait("Reset")?;
    let reference = world.reference("Annotation note")?;
    world.command(&["set-value", &reference, "Keep my draft"])?;
    semantic(world, "Reset")?;
    let reference = world.reference("Annotation note")?;
    world.command(&[
        "wait",
        "--element",
        &reference,
        "--predicate",
        "value",
        "--value",
        "Keep my draft",
        "--timeout",
        "5000",
    ])?;
    Ok(())
}
#[then("saving the cleared span is disabled")]
fn reset_disabled(world: &mut Desktop) -> Result<()> {
    ensure!(
        world.reference("Reset").is_err(),
        "Reset remains visible without a selection"
    );
    world.wait("＋ Start span")?;
    world.wait("No messages in this span")?;
    world.wait("Included messages: 0")?;
    let button = world.named("Save annotation")?;
    let click = button["available_actions"]
        .as_array()
        .is_some_and(|xs| xs.iter().any(|x| x == "Click"));
    ensure!(
        button["enabled"] == false || !click,
        "Save is still actionable after reset"
    );
    Ok(())
}

#[when("I browse saved spans across days")]
fn span_tree(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    let time: i64 = db.query_row("SELECT MIN(time) FROM utterances", [], |r| r.get(0))?;
    db.execute(
        "INSERT INTO notes (start,end,label,body) VALUES (?1,?2,'Earlier span','')",
        [time - 86400000, time - 86400000 + 1000],
    )?;
    db.execute(
        "INSERT INTO notes (start,end,label,body) VALUES (?1,?2,'Recent span','')",
        [time, time + 1000],
    )?;
    semantic(world, "Hide sidebar")?;
    world.wait("Show sidebar")?;
    semantic(world, "Show sidebar")?;
    world.wait("Hide sidebar")?;
    world.wait("Open span Recent span")?;
    ensure!(
        world.reference("Open span Earlier span").is_err(),
        "Other day's span is visible"
    );
    ensure!(
        world.reference("Start recording").is_err(),
        "Record is available on a past day"
    );
    semantic(world, "2026-11-08 · 0 spoken words")?;
    world.wait("Open span Earlier span")?;
    ensure!(
        world.reference("Open span Recent span").is_err(),
        "Previous day's span remains visible"
    );
    ensure!(
        world.reference("＋ Start span").is_err(),
        "Start span available without activity"
    );
    ensure!(
        world.reference("◇ Label span").is_err(),
        "Label span available without activity"
    );
    semantic(world, "2026-11-10 · 0 spoken words")?;
    world.wait("Open whole day span")?;
    world.wait("Start recording")?;
    ensure!(
        world.reference("＋ Start span").is_err(),
        "Start span available on empty today"
    );
    ensure!(
        world.reference("◇ Label span").is_err(),
        "Label span available on empty today"
    );
    semantic(world, "Settings")?;
    world.wait("Microphone input device")?;
    world.wait("System audio output device")?;
    ensure!(
        world.reference("Start recording").is_err(),
        "Recording button is visible in Settings"
    );
    semantic(world, "Back")?;
    world.wait("Start recording")?;
    semantic(world, "Start recording")?;
    world.wait("Stop recording")?;
    world.wait("Microphone level")?;
    semantic(world, "Stop recording")?;
    world.wait("Start recording")?;
    semantic(world, "Open whole day span")?;
    world.wait("No messages in this span")?;
    ensure!(
        world.reference("Save annotation").is_err(),
        "Empty day has span save action"
    );
    Ok(())
}

#[when("I add a labeled span through the conversation popover")]
fn conversation_popover(world: &mut Desktop) -> Result<()> {
    let action = world.named("◇ Label span")?["bounds"].clone();
    let frame = world.snapshot()?["tree"]["bounds"].clone();
    let right = frame["x"].as_f64().unwrap_or(0.) + frame["width"].as_f64().unwrap_or(0.);
    ensure!(
        action["x"].as_f64().unwrap_or(0.) >= right - 224.,
        "Span action is outside right sidebar"
    );
    popup_action(world, "◇ Label span")?;
    world.wait("Annotation label")?;
    let bounds = world.named("Span form")?["bounds"].clone();
    let window = world.snapshot()?["tree"]["bounds"].clone();
    let center = bounds["x"].as_f64().unwrap_or(0.) + bounds["width"].as_f64().unwrap_or(0.) / 2.;
    let window_center =
        window["x"].as_f64().unwrap_or(0.) + window["width"].as_f64().unwrap_or(0.) / 2.;
    ensure!(
        (center - window_center).abs() < 3.,
        "Span form is not centered"
    );
    for value in ["", "Untitled span", "Untitles Span"] {
        world.set_field("Annotation label", value)?;
        let save = world.named("Save annotation")?;
        ensure!(
            save["enabled"] == false
                || !save["available_actions"]
                    .as_array()
                    .is_some_and(|xs| xs.iter().any(|x| x == "Click")),
            "Blank/default span is savable"
        );
    }
    world.set_field("Annotation label", "Draft decision")?;
    popup_action(world, "Cancel")?;
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    let count: i64 = db.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?;
    ensure!(count == 0, "Cancel saved a span");
    popup_action(world, "◇ Label span")?;
    world.wait("Annotation label")?;
    let draft = world.reference("Annotation label")?;
    world.command(&[
        "wait",
        "--element",
        &draft,
        "--predicate",
        "value",
        "--value",
        "Draft decision",
        "--timeout",
        "10000",
    ])?;
    for (name, text) in [
        ("Annotation label", world.label.as_str()),
        ("Annotation note", "Keep this passage local"),
    ] {
        world.set_field(name, text)?;
    }
    popup_action(world, "Save annotation")?;
    world.wait(&format!("Open span {}", world.label))?;
    persisted(world)?;
    Ok(())
}

fn popup_action(world: &Desktop, name: &str) -> Result<()> {
    semantic(world, name)?;
    world.command(&["--headed", "focus-window", "--app", APP])?;
    Ok(())
}

#[when("I save a span after switching to the next fixture day")]
fn save_on_selected_day(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    db.execute("UPDATE utterances SET time=time+86400000", [])?;
    popup_action(world, "Hide sidebar")?;
    world.wait("Show sidebar")?;
    popup_action(world, "Show sidebar")?;
    world.wait("Hide sidebar")?;
    popup_action(world, "2026-11-10 · 48 spoken words")?;
    world.wait("◇ Label span")?;
    popup_action(world, "◇ Label span")?;
    world.wait("Annotation label")?;
    world.set_field("Annotation label", "Next-day decision")?;
    popup_action(world, "Save annotation")?;
    world.wait("Open span Next-day decision")?;
    let (from, to): (i64, i64) = db.query_row("SELECT start,end FROM notes", [], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    let first: i64 = db.query_row(
        "SELECT time FROM utterances ORDER BY time DESC LIMIT 1 OFFSET 1",
        [],
        |r| r.get(0),
    )?;
    let last: i64 = db.query_row("SELECT MAX(time) FROM utterances", [], |r| r.get(0))?;
    ensure!(
        from == first && to == last + 1,
        "Saved range lost its selected date or precision"
    );
    let future = world.named("2026-11-11 · 0 spoken words")?;
    ensure!(
        future["enabled"] == false
            || !future["available_actions"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|x| x == "Click")),
        "Future date remains selectable"
    );
    Ok(())
}

#[when("I browse previous months and return to today's empty transcript")]
fn previous_months(world: &mut Desktop) -> Result<()> {
    popup_action(world, "Previous month")?;
    world.wait("2026-10-14 · 0 spoken words")?;
    popup_action(world, "2026-10-14 · 0 spoken words")?;
    world.wait("No transcript for this day")?;
    ensure!(
        world.reference("↓ Back to live").is_err(),
        "Empty history shows Back to live"
    );
    popup_action(world, "Next month")?;
    popup_action(world, "2026-11-10 · 0 spoken words")?;
    world.wait("Start recording in transcript")?;
    ensure!(
        world.reference("↓ Back to live").is_err(),
        "Empty today shows Back to live"
    );
    ensure!(
        world.reference("Start recording").is_ok(),
        "Sidebar recording action is absent"
    );
    Ok(())
}
#[when("I record the real fixture audio from the empty transcript")]
fn record_real_fixture(world: &mut Desktop) -> Result<()> {
    ensure!(
        std::env::var_os("WORKLOG_AUDIO_FIXTURE").is_some(),
        "Set WORKLOG_AUDIO_FIXTURE to run this local ASR test"
    );
    popup_action(world, "2026-11-10 · 0 spoken words")?;
    popup_action(world, "Hide sidebar")?;
    world.wait("Start recording in transcript")?;
    ensure!(
        world.reference("↓ Back to live").is_err(),
        "Empty transcript shows Back to live"
    );
    popup_action(world, "Start recording in transcript")?;
    world.command(&[
        "wait",
        "--app",
        APP,
        "--text",
        "Live transcript Me:",
        "--timeout",
        "120000",
    ])?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("streaming-draft.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    world.command(&[
        "wait",
        "--app",
        APP,
        "--text",
        "meeting",
        "--timeout",
        "120000",
    ])?;
    world.command(&[
        "wait",
        "--app",
        APP,
        "--text",
        "Start recording",
        "--timeout",
        "60000",
    ])?;
    ensure!(
        world.reference("↓ Back to live").is_err(),
        "Already-live transcript shows Back to live"
    );
    ensure!(
        !world
            .snapshot()?
            .to_string()
            .contains("Live transcript Me:"),
        "Finalized speech retains its draft bubble"
    );
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("streaming-final.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    ensure!(
        world.reference("No transcript for this day").is_err(),
        "Empty state remains after live speech"
    );
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    let count: i64 = db.query_row("SELECT count(*) FROM transcript_audio", [], |r| r.get(0))?;
    ensure!(count > 0, "No actual ASR segments were persisted");
    let content: String = db.query_row("SELECT group_concat(u.text,' ') FROM utterances u JOIN transcript_audio a ON a.utterance_id=u.id", [], |r| r.get(0))?;
    ensure!(
        content.to_lowercase().contains("meeting"),
        "Fixture audio did not reach the actual transcript"
    );
    if let Some(mut child) = world.child.take() {
        stop_test_child(&mut child);
    }
    world.launch()?;
    world.wait("meeting")?;
    Ok(())
}

#[when("I use the live conversation notes and breakpoint composer")]
fn live_composer(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    db.execute("UPDATE utterances SET time=time+86400000", [])?;
    popup_action(world, "Hide sidebar")?;
    popup_action(world, "Show sidebar")?;
    popup_action(world, "2026-11-10 · 48 spoken words")?;
    world.wait("Comment or take a note")?;
    for removed in [
        "Transcript",
        "Latest passage",
        "Reset",
        "Add span",
        "＋ Start span",
        "Comment on message 1",
        "↓ Back to live",
    ] {
        ensure!(
            world.reference(removed).is_err(),
            "Unexpected control: {removed}"
        );
    }
    let transcript = world.named("Conversation transcript")?["bounds"].clone();
    let chart = world.named("Voice activity chart")?["bounds"].clone();
    let ratio = transcript["width"].as_f64().context("Transcript width")?
        / chart["width"].as_f64().context("Chart width")?;
    ensure!(
        (0.47..0.56).contains(&ratio),
        "Conversation width is not half the main area: {ratio}"
    );
    world.wait("Minute 15:26")?;
    let (time, speaker, text): (i64, String, String) = db.query_row(
        "SELECT time,speaker,text FROM utterances ORDER BY time DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let last = world.named(&format!(
        "{}, {}: {}",
        speaker,
        team_up_gpui_spike::local(time, false),
        text
    ))?["bounds"]
        .clone();
    let bottom = |b: &Value| b["y"].as_f64().unwrap_or(0.) + b["height"].as_f64().unwrap_or(0.);
    let transcript = world.named("Conversation transcript")?["bounds"].clone();
    ensure!(
        (bottom(&last) - bottom(&transcript)).abs() < 4.,
        "Conversation is not aligned to the bottom"
    );
    let x = transcript["x"].as_f64().context("Transcript x")? + 100.;
    let y = transcript["y"].as_f64().context("Transcript y")? + 100.;
    world.command(&["--headed", "hover", "--xy", &format!("{x},{y}")])?;
    world.command(&[
        "--headed",
        "mouse-wheel",
        "--x",
        &x.to_string(),
        "--y",
        &y.to_string(),
        "--dy",
        "5",
    ])?;
    world.wait("↓ Back to live")?;
    popup_action(world, "↓ Back to live")?;
    ensure!(
        world.reference("↓ Back to live").is_err(),
        "Back to live remains at the latest message"
    );
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("live.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    let tick = world.named("Minute 15:26")?["bounds"].clone();
    let x = tick["x"].as_f64().context("Tick x")? + 10.;
    let y = tick["y"].as_f64().context("Tick y")? + 8.;
    world.command(&["--headed", "hover", "--xy", &format!("{x},{y}")])?;
    world.command(&["wait", "700"])?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("minute-hover.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    world.set_field("Comment or take a note", "Remember to follow up tomorrow")?;
    popup_action(world, "Add comment or note")?;
    world.wait("Remember to follow up tomorrow")?;
    world.set_field("Comment or take a note", "Next topic begins here")?;
    popup_action(world, "Breakpoint")?;
    world.wait("Next topic begins here")?;
    let (notes, breaks): (i64, i64) = db.query_row(
        "SELECT count(*),sum(breakpoint) FROM timeline_entries",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        notes == 2 && breaks == 1,
        "Composer did not save a separate note and breakpoint"
    );
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("breakpoints.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    popup_action(world, "Open whole day span")?;
    world.wait("Add span")?;
    ensure!(
        world.reference("Annotation label").is_err(),
        "Marking immediately opened the form"
    );
    ensure!(world.reference("Reset").is_err(), "Reset is still visible");
    popup_action(world, "Add span")?;
    world.wait("Annotation label")?;
    world.set_field("Annotation label", "Marked discussion")?;
    popup_action(world, "Save annotation")?;
    world.wait("Open span Marked discussion")?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("conversation.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    if let Some(mut child) = world.child.take() {
        stop_test_child(&mut child);
    }
    world.launch()?;
    world.wait("Next topic begins here")?;
    world.wait("Remember to follow up tomorrow")?;
    Ok(())
}

#[when("I use the centered timeline and create breakpoints without recording")]
fn driver_timeline(world: &mut Desktop) -> Result<()> {
    fn pick<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
        if node["name"].as_str() == Some(name) {
            return Some(node);
        }
        node["children"]
            .as_array()?
            .iter()
            .find_map(|child| pick(child, name))
    }
    world.wait("Hide sidebar")?;
    ensure!(
        world.reference("Show sidebar").is_err(),
        "Sidebar is collapsed by default"
    );
    ensure!(
        world.reference("↓ Back to live").is_err(),
        "Live edge shows Back to live"
    );
    let layout = world.snapshot()?;
    let transcript = pick(&layout["tree"], "Conversation transcript")
        .context("Missing transcript")?["bounds"]
        .clone();
    let composer =
        pick(&layout["tree"], "Conversation notes").context("Missing composer")?["bounds"].clone();
    let minimap =
        pick(&layout["tree"], "Day minimap").context("Missing minimap")?["bounds"].clone();
    let left = |b: &Value| b["x"].as_f64().unwrap_or(0.);
    let right = |b: &Value| left(b) + b["width"].as_f64().unwrap_or(0.);
    ensure!(
        left(&composer) >= left(&transcript) - 1. && right(&composer) <= right(&transcript) + 1.,
        "Composer overlaps the sidebar or exceeds the transcript column"
    );
    ensure!(
        right(&composer) < left(&minimap),
        "Composer overlaps the minimap"
    );
    let axis =
        pick(&layout["tree"], "Transcript range selector").context("Missing ruler")?["bounds"]
            .clone();
    let center = |b: &Value| b["x"].as_f64().unwrap_or(0.) + b["width"].as_f64().unwrap_or(0.) / 2.;
    ensure!(
        (center(&transcript) - center(&axis)).abs() < 2.,
        "Time scale is not centered"
    );
    let db = rusqlite::Connection::open(world.folder.join("fixture.sqlite"))?;
    for speaker in ["Me", "Them"] {
        let (time, text): (i64, String) = db.query_row(
            "SELECT time,text FROM utterances WHERE speaker=?1 ORDER BY time DESC LIMIT 1",
            [speaker],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let name = format!(
            "{}, {}: {}",
            speaker,
            team_up_gpui_spike::local(time, false),
            text
        );
        let bubble =
            pick(&layout["tree"], &name).context("Missing speech bubble")?["bounds"].clone();
        let x = bubble["x"].as_f64().context("Bubble x")?;
        let right = x + bubble["width"].as_f64().context("Bubble width")?;
        if speaker == "Me" {
            ensure!(
                (x - center(&axis) - 16.).abs() < 3.,
                "Me bubble is not anchored just right of the ruler"
            );
        } else {
            ensure!(
                (center(&axis) - right - 16.).abs() < 3.,
                "Them bubble is not anchored just left of the ruler"
            );
        }
    }
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("centered-ruler.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    popup_action(world, "2026-11-10 · 0 spoken words")?;
    popup_action(world, "Hide sidebar")?;
    world.wait("Start recording in transcript")?;
    let controls = world.snapshot()?;
    let breakpoint =
        pick(&controls["tree"], "Breakpoint").context("Missing breakpoint")?["bounds"].clone();
    let record =
        pick(&controls["tree"], "Start recording").context("Missing record")?["bounds"].clone();
    let field = pick(&controls["tree"], "Comment or take a note")
        .context("Missing composer field")?["bounds"]
        .clone();
    let composer =
        pick(&controls["tree"], "Conversation notes").context("Missing composer")?["bounds"]
            .clone();
    let group_center = (breakpoint["x"].as_f64().context("Breakpoint x")?
        + record["x"].as_f64().context("Record x")?
        + record["width"].as_f64().context("Record width")?)
        / 2.;
    ensure!(
        (group_center - center(&composer)).abs() < 2.,
        "Record and Breakpoint are not centered together"
    );
    let y = |b: &Value| b["y"].as_f64().unwrap_or(0.);
    ensure!(
        (y(&breakpoint) - y(&record)).abs() < 2.,
        "Breakpoint and Record are not on the same row"
    );
    ensure!(
        y(&record) + record["height"].as_f64().unwrap_or(0.) <= y(&field),
        "Actions are not above the comment field"
    );
    popup_action(world, "Breakpoint")?;
    world.command(&[
        "wait",
        "--app",
        APP,
        "--text",
        "Breakpoint at",
        "--timeout",
        "10000",
    ])?;
    let (count, body): (i64, String) =
        db.query_row("SELECT count(*),body FROM timeline_entries", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    ensure!(
        count == 1 && body.is_empty(),
        "Empty-text breakpoint was not saved"
    );
    ensure!(
        world.reference("Stop recording").is_err(),
        "Breakpoint started recording"
    );
    world.set_field("Comment or take a note", "A note before recording")?;
    popup_action(world, "Add comment or note")?;
    world.wait("A note before recording")?;
    world.command(&[
        "screenshot",
        "--app",
        APP,
        world
            .folder
            .join("independent-breakpoint.png")
            .to_str()
            .context("Screenshot path")?,
    ])?;
    if let Some(mut child) = world.child.take() {
        stop_test_child(&mut child);
    }
    world.launch()?;
    world.wait("A note before recording")?;
    ensure!(
        world.reference("Hide sidebar").is_ok(),
        "Sidebar is collapsed after restart"
    );
    Ok(())
}
