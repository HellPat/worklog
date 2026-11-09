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

const APP: &str = "team-up-gpui-spike";
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
#[derive(Debug, Default, World)]
struct Desktop {
    child: Option<Child>,
    folder: PathBuf,
    label: String,
}
impl Drop for Desktop {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
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
        self.command(&["wait", "--app", APP, "--text", text, "--timeout", "5000"])?;
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
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_team-up-gpui-spike"))
                .env("TEAM_UP_SPIKE_DB", self.folder.join("fixture.sqlite"))
                .env(
                    "TEAM_UP_SPIKE_TRANSCRIPT",
                    if self.folder.ends_with("audio") {
                        audio_path
                    } else {
                        std::path::PathBuf::new()
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
            "Team-up GPUI spike",
            "--timeout",
            "10000",
        ])?;
        self.command(&["--headed", "focus-window", "--app", APP])?;
        self.wait("Start simulation")?;
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
    world.click("Start simulation")?;
    world.wait("● Simulated recording · Stop")?;
    world.click("● Simulated recording · Stop")?;
    world.wait("Start simulation")?;
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
    world.wait("Annotation saved locally")?;
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
    if world.label.contains("chart-refine") {
        ensure!(
            start == team_up_gpui_spike::parse_local("14:33:22.910")? && end == start + 1,
            "Refined message boundaries were not persisted"
        );
    }
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
            .fail_on_skipped()
            .run_and_exit(root().join("features")),
    );
    Ok(())
}

#[when("I refine the span to one exact message")]
fn refine(world: &mut Desktop) -> Result<()> {
    world.wait("Refine span")?;
    world.click("Start at message 4")?;
    world.click("End at message 4")?;
    world.wait("Included messages: 1")?;
    for (name, value) in [("From", "14:33:22.910"), ("To", "14:33:22.911")] {
        let reference = world.reference(name)?;
        world.command(&[
            "wait",
            "--element",
            &reference,
            "--predicate",
            "value",
            "--value",
            value,
            "--timeout",
            "5000",
        ])?;
    }
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
    world.click("Settings")?;
    world.wait("Engine: Nemotron")?;
    world.click("Back")?;
    world.wait("New span description")?;
    world.click("Start without details")?;
    world.wait("■ End span")?;
    world.click("■ End span")?;
    world.wait("Span ended and saved locally")?;
    Ok(())
}
#[then("the unlabeled span is stored")]
fn blank_saved(world: &mut Desktop) -> Result<()> {
    let db = rusqlite::Connection::open_with_flags(
        world.folder.join("fixture.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (label, body, start, end): (String, String, i64, i64) =
        db.query_row("SELECT label,body,start,end FROM notes", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
    ensure!(
        label == "Untitled span" && body.is_empty() && end > start,
        "Optional span details did not persist"
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
    world.click("Save reply")?;
    world.wait("Reply saved locally")?;
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
    world.wait("Refine span")?;
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
    world.wait("Span ended and saved locally")?;
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
    world.wait("Refine span")?;
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
    world.wait("Refine span")?;
    Ok(())
}
