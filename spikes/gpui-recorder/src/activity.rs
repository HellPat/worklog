//! Local macOS activity capture. The display is condensed; stored payloads are not.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ActivityPayload {
    pub event_id: String,
    pub time: i64,
    pub application: String,
    pub action: String,
    #[serde(default)]
    pub bundle_id: String,
    #[serde(default)]
    pub window: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl ActivityPayload {
    pub fn summary(&self) -> String {
        match self.action.as_str() {
            "application" => format!("Switched to {}", self.application),
            "navigation" => format!("Opened a page in {}", self.application),
            "click" | "right_click" => {
                let target = if self.target.is_empty() {
                    &self.role
                } else {
                    &self.target
                };
                if target.is_empty() {
                    format!("Clicked in {}", self.application)
                } else {
                    format!("Clicked {target}")
                }
            }
            "capture_started" => "Computer activity capture started".into(),
            "capture_stopped" => "Computer activity capture stopped".into(),
            "permission_changed" => {
                if self.metadata["event"]["authorized"] == true {
                    "Accessibility permission granted".into()
                } else {
                    "Accessibility permission unavailable".into()
                }
            }
            "accessibility" => {
                let notification = self.metadata["event"]["notification"]
                    .as_str()
                    .unwrap_or("Accessibility event");
                format!(
                    "{} · {}",
                    self.application,
                    notification.trim_start_matches("AX")
                )
            }
            "focus" => {
                if self.target.is_empty() {
                    format!("Focus changed in {}", self.application)
                } else {
                    format!("Focused {}", self.target)
                }
            }
            _ => format!("{} · {}", self.application, self.action),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ActivityEntry {
    pub id: i64,
    pub event: Arc<ActivityPayload>,
}
#[derive(Clone, Debug)]
pub struct ActivityGroup {
    pub entries: Vec<ActivityEntry>,
}
impl ActivityGroup {
    pub fn id(&self) -> i64 {
        self.entries.first().map(|entry| entry.id).unwrap_or(0)
    }
    pub fn time(&self) -> i64 {
        self.entries
            .first()
            .map(|entry| entry.event.time)
            .unwrap_or(0)
    }
    pub fn summary(&self) -> String {
        if self.entries.len() == 1 {
            self.entries[0].event.summary()
        } else {
            format!(
                "{} · {} events",
                self.entries[0].event.application,
                self.entries.len()
            )
        }
    }
}
pub fn groups(entries: &[ActivityEntry]) -> Vec<ActivityGroup> {
    let mut result: Vec<ActivityGroup> = Vec::new();
    for entry in entries {
        let join = result
            .last()
            .and_then(|group| group.entries.last())
            .is_some_and(|last| {
                last.event.bundle_id == entry.event.bundle_id
                    && last.event.application == entry.event.application
                    && entry.event.time - last.event.time <= 5000
                    && entry.event.time >= last.event.time
                    && !matches!(
                        entry.event.action.as_str(),
                        "capture_started" | "capture_stopped"
                    )
                    && !matches!(
                        last.event.action.as_str(),
                        "capture_started" | "capture_stopped"
                    )
            });
        if join {
            if let Some(group) = result.last_mut() {
                group.entries.push(entry.clone());
            }
        } else {
            result.push(ActivityGroup {
                entries: vec![entry.clone()],
            });
        }
    }
    result
}
pub enum CaptureMessage {
    Status(bool),
    Activity(ActivityPayload),
    Error(String),
}
pub fn parse_message(line: &str) -> Result<CaptureMessage> {
    let value: Value = serde_json::from_str(line)?;
    Ok(match value["type"].as_str() {
        Some("status") => CaptureMessage::Status(
            value["authorized"]
                .as_bool()
                .context("Missing accessibility status")?,
        ),
        Some("activity") => CaptureMessage::Activity(serde_json::from_value(value)?),
        Some("error") => CaptureMessage::Error(
            value["message"]
                .as_str()
                .unwrap_or("Activity capture failed")
                .into(),
        ),
        _ => CaptureMessage::Activity(ActivityPayload {
            event_id: uuid::Uuid::new_v4().to_string(),
            time: chrono::Utc::now().timestamp_millis(),
            application: "macOS".into(),
            action: "native_event".into(),
            metadata: value,
            ..Default::default()
        }),
    })
}
const HELPER_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.hellpat.worklog.activity</string>
<key>CFBundleName</key><string>Worklog Activity</string>
<key>CFBundleDisplayName</key><string>Worklog Activity</string>
<key>CFBundleExecutable</key><string>worklog-activity</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>NSAccessibilityUsageDescription</key><string>Log your app, window, and control activity locally in your Worklog timeline.</string>
</dict></plist>"#;
fn install_helper(directory: &std::path::Path, bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    let bundle = directory.join("Worklog Activity.app");
    let contents = bundle.join("Contents");
    let path = contents.join("MacOS/worklog-activity");
    let plist = contents.join("Info.plist");
    use std::hash::{Hash, Hasher};
    let mut hash = std::hash::DefaultHasher::new();
    bytes.hash(&mut hash);
    HELPER_PLIST.hash(&mut hash);
    let fingerprint = format!("{:016x}", hash.finish());
    let stamp = directory.join("activity-source-fingerprint");
    if !path.is_file()
        || std::fs::read_to_string(&stamp).ok().as_deref() != Some(fingerprint.as_str())
        || std::fs::read_to_string(&plist).ok().as_deref() != Some(HELPER_PLIST)
    {
        std::fs::create_dir_all(path.parent().context("Missing helper directory")?)?;
        let staged = path.with_extension(format!("{}.new", std::process::id()));
        std::fs::write(&staged, bytes)?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(staged, &path)?;
        std::fs::write(&plist, HELPER_PLIST)?;
        let signed = Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--identifier",
                "com.hellpat.worklog.activity",
                "--requirements",
                "=designated => identifier \"com.hellpat.worklog.activity\";",
            ])
            .arg(&bundle)
            .output()
            .context("Could not sign the Accessibility helper")?;
        if !signed.status.success() {
            anyhow::bail!(
                "Could not sign the Accessibility helper: {}",
                String::from_utf8_lossy(&signed.stderr)
            );
        }
        std::fs::write(&stamp, &fingerprint)?;
    }
    Ok(path)
}
fn helper_path() -> Result<PathBuf> {
    let directory = PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?)
        .join("Library/Application Support/worklog/helpers");
    install_helper(
        &directory,
        include_bytes!(concat!(env!("OUT_DIR"), "/worklog-activity")),
    )
}
#[derive(Default)]
pub struct Capture {
    child: Option<Child>,
    events: Option<Receiver<CaptureMessage>>,
    reader: Option<thread::JoinHandle<()>>,
    restart_requested: bool,
    permission_events: Option<Receiver<CaptureMessage>>,
}
impl Capture {
    pub fn start(&mut self) -> Result<()> {
        if self.child.is_some() {
            self.restart_requested = true;
            return Ok(());
        }
        let mut child = Command::new(helper_path()?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Could not start local computer activity capture")?;
        let output = child
            .stdout
            .take()
            .context("Activity output is unavailable")?;
        let (tx, rx) = mpsc::channel();
        self.reader = Some(thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let message = match line {
                    Ok(line) => parse_message(&line).unwrap_or_else(|error| {
                        CaptureMessage::Error(format!(
                            "Activity event could not be decoded: {error}; raw event: {line}"
                        ))
                    }),
                    Err(error) => CaptureMessage::Error(error.to_string()),
                };
                if tx.send(message).is_err() {
                    break;
                }
            }
        }));
        self.child = Some(child);
        self.events = Some(rx);
        Ok(())
    }
    pub fn stop(&mut self) {
        self.restart_requested = false;
        if let Some(child) = &mut self.child {
            drop(child.stdin.take());
        }
    }
    pub fn request_accessibility(&mut self) -> Result<()> {
        let path = helper_path()?;
        let bundle = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .context("Missing helper app")?
            .to_path_buf();
        let (tx, rx) = mpsc::channel();
        self.permission_events = Some(rx);
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let status = Command::new("/usr/bin/open")
                    .args(["-n", "-a"])
                    .arg(bundle)
                    .args(["--args", "--request"])
                    .status()?;
                if !status.success() {
                    anyhow::bail!("macOS could not open the Accessibility permission helper");
                }
                let mut child = Command::new(path)
                    .arg("--watch-permission")
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()?;
                let output = child
                    .stdout
                    .take()
                    .context("Missing permission status output")?;
                for line in BufReader::new(output).lines() {
                    if tx.send(parse_message(&line?)?).is_err() {
                        let _ = child.kill();
                        break;
                    }
                }
                let status = child.wait()?;
                if !status.success() {
                    anyhow::bail!("Accessibility status check failed");
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = tx.send(CaptureMessage::Error(error.to_string()));
            }
        });
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<CaptureMessage> {
        if self
            .child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_some())
            && self
                .reader
                .as_ref()
                .is_none_or(|reader| reader.is_finished())
        {
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            self.child = None;
        }
        let mut messages: Vec<_> = self
            .events
            .as_ref()
            .map(|events| events.try_iter().collect())
            .unwrap_or_default();
        if let Some(events) = &self.permission_events {
            messages.extend(events.try_iter());
        }
        if self.child.is_none() && self.restart_requested {
            self.restart_requested = false;
            if let Err(error) = self.start() {
                messages.push(CaptureMessage::Error(error.to_string()));
            }
        }
        messages
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_helper_has_stable_bundle_identity_and_is_not_reinstalled() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/worklog-activity"));
        let executable = install_helper(dir.path(), bytes)?;
        let before = std::fs::metadata(&executable)?.modified()?;
        assert_eq!(install_helper(dir.path(), bytes)?, executable);
        assert_eq!(before, std::fs::metadata(&executable)?.modified()?);
        let bundle = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let verified = Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(bundle)
            .output()?;
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
        let identity = Command::new("/usr/bin/codesign")
            .args(["-d", "-r-"])
            .arg(bundle)
            .output()?;
        assert!(identity.status.success());
        let requirement = format!(
            "{}{}",
            String::from_utf8_lossy(&identity.stdout),
            String::from_utf8_lossy(&identity.stderr)
        );
        assert!(
            requirement.contains("identifier \"com.hellpat.worklog.activity\""),
            "{requirement}"
        );
        Ok(())
    }
    #[test]
    fn unknown_native_messages_keep_the_entire_payload() {
        let raw = serde_json::json!({"type":"future_notification", "attributes":{"text":"full value","error":-1}});
        let CaptureMessage::Activity(event) = parse_message(&raw.to_string()).unwrap() else {
            panic!("expected raw event")
        };
        assert_eq!(event.metadata, raw);
    }
    #[test]
    fn grouping_retains_every_event_and_session_boundaries() {
        let entries: Vec<_> = [
            (0, "capture_started"),
            (1, "focus"),
            (2, "click"),
            (8000, "focus"),
            (8001, "capture_stopped"),
        ]
        .into_iter()
        .enumerate()
        .map(|(id, (time, action))| ActivityEntry {
            id: id as i64,
            event: Arc::new(ActivityPayload {
                time,
                action: action.into(),
                application: "Editor".into(),
                bundle_id: "editor".into(),
                ..Default::default()
            }),
        })
        .collect();
        let grouped = groups(&entries);
        assert_eq!(
            grouped.iter().map(|g| g.entries.len()).collect::<Vec<_>>(),
            vec![1, 2, 1, 1]
        );
        assert_eq!(
            grouped
                .iter()
                .flat_map(|g| g.entries.iter().map(|e| e.id))
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
    }
}
