use crate::model::{Batch, Destination};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
};

// The local app server reads the installed Codex chat store. No credentials or
// private transcript files are passed to the destination: only the selected batch.
struct Codex {
    child: Child,
    input: Option<ChildStdin>,
    output: std::sync::mpsc::Receiver<std::result::Result<Value, String>>,
    next: u64,
}
impl Codex {
    fn connect() -> Result<Self> {
        let mut child = Command::new("/opt/homebrew/bin/codex")
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().context("Missing Codex input")?;
        let stdout = child.stdout.take().context("Missing Codex output")?;
        let (sender, output) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let parsed = line.map_err(|e| e.to_string()).and_then(|line| {
                    serde_json::from_str::<Value>(&line).map_err(|e| e.to_string())
                });
                if sender.send(parsed).is_err() {
                    break;
                }
            }
        });
        let mut connection = Self {
            child,
            input: Some(input),
            output,
            next: 1,
        };
        connection.call("initialize", json!({"clientInfo":{"name":"team_up","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        connection.write(&json!({"method":"initialized"}))?;
        Ok(connection)
    }
    fn write(&mut self, value: &Value) -> Result<()> {
        let input = self.input.as_mut().context("Codex input is closed")?;
        writeln!(input, "{value}")?;
        input.flush()?;
        Ok(())
    }
    fn server_request(&mut self, value: &Value) -> Result<()> {
        if value.get("method").is_some()
            && let Some(id) = value.get("id")
        {
            self.write(&json!({"id":id,"error":{"code":-32601,"message":"Team-up delivers transcripts only. Handle interactive approvals in the receiving agent client."}}))?;
        }
        Ok(())
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        self.write(&json!({"id":id,"method":method,"params":params}))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let value = self
                .output
                .recv_timeout(remaining)
                .context("Codex did not acknowledge the request within 20 seconds")?
                .map_err(anyhow::Error::msg)?;
            self.server_request(&value)?;
            if value["id"] == id {
                if let Some(error) = value.get("error") {
                    return Err(RpcRejected(format!("Codex rejected request: {error}")).into());
                }
                return Ok(value["result"].clone());
            }
        }
    }
}
impl Drop for Codex {
    fn drop(&mut self) {
        self.input.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn list() -> Result<Vec<Destination>> {
    let mut client = Codex::connect()?;
    let result = client.call(
        "thread/list",
        json!({"limit":100,"archived":false,"sortKey":"updated_at","sortDirection":"desc"}),
    )?;
    let data = result["data"]
        .as_array()
        .context("Codex returned no chat list")?;
    Ok(data
        .iter()
        .filter_map(|thread| {
            let id = thread["id"].as_str()?;
            let title = thread["name"]
                .as_str()
                .or_else(|| thread["preview"].as_str())
                .unwrap_or("Untitled chat");
            Some(Destination {
                provider: "Codex".into(),
                id: id.into(),
                title: title.chars().take(100).collect(),
            })
        })
        .collect())
}
pub fn create(title: &str) -> Result<Destination> {
    let mut client = Codex::connect()?;
    let result = client.call("thread/start", json!({"cwd":std::env::var("HOME")?,"approvalPolicy":"on-request","sandbox":"workspace-write"}))?;
    let id = result["thread"]["id"]
        .as_str()
        .context("Codex did not return a chat ID")?;
    client.call("thread/name/set", json!({"threadId":id,"name":title}))?;
    Ok(Destination {
        provider: "Codex".into(),
        id: id.into(),
        title: title.into(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendFailure {
    Rejected(String),
    Unconfirmed(String),
}
impl std::fmt::Display for SendFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(message) | Self::Unconfirmed(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for SendFailure {}
#[derive(Debug)]
struct RpcRejected(String);
impl std::fmt::Display for RpcRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RpcRejected {}

/// Deliver to the existing desktop owner only. No private writer, resume, fork,
/// takeover, or queued-success fallback is permitted in the message path.
pub fn send(batch: &Batch) -> std::result::Result<(), SendFailure> {
    let path = crate::desktop::socket_path().map_err(|e| SendFailure::Rejected(e.to_string()))?;
    send_via_desktop(batch, &path)
}
pub fn send_via_desktop(
    batch: &Batch,
    path: &std::path::Path,
) -> std::result::Result<(), SendFailure> {
    if batch.destination.provider != "Codex" {
        return Err(SendFailure::Rejected(
            "This destination is not configured".into(),
        ));
    }
    #[cfg(unix)]
    if let Some(mut desktop) = crate::desktop::Desktop::discover(path, &batch.destination.id)
        .map_err(|e| SendFailure::Rejected(e.to_string()))?
    {
        return desktop.send(batch);
    }
    Err(SendFailure::Rejected(
        "Open this chat in Codex, then send again. The transcript has not been sent.".into(),
    ))
}
pub fn open_chat(destination: &Destination) -> Result<()> {
    let id = uuid::Uuid::parse_str(&destination.id)?;
    let status = Command::new("/usr/bin/open")
        .arg(format!("codex://threads/{id}"))
        .status()?;
    if !status.success() {
        return Err(anyhow::anyhow!("Could not open the selected chat in Codex"));
    }
    Ok(())
}

/// Read back a specific destination for integration verification. The panel does
/// not display or import the receiving chat's history into the recording log.
pub fn read_chat(id: &str) -> Result<Value> {
    Codex::connect()?.call("thread/read", json!({"threadId":id,"includeTurns":true}))
}
