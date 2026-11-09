use crate::model::{Event, Model};
use anyhow::{Context, Result};
use chrono::Utc;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub struct Journal {
    pub directory: PathBuf,
    pub model: Model,
}
impl Journal {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory)?;
        let mut paths: Vec<_> = fs::read_dir(&directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        paths.sort();
        let mut model = Model::new(Utc::now());
        let mut legacy_rejected = None;
        let newest = paths.last().cloned();
        for path in paths {
            let bytes = fs::read(&path)?;
            let mut offset = 0;
            for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
                if line.iter().all(u8::is_ascii_whitespace) {
                    offset += line.len();
                    continue;
                }
                match serde_json::from_slice::<Event>(line) {
                    Ok(event) => {
                        if let Event::SendUncertain { id, message, .. } = &event
                            && message.starts_with("Codex rejected request:")
                            && message.contains("already has an active writer")
                        {
                            legacy_rejected = Some(*id);
                        }
                        model.apply(&event);
                        if !line.ends_with(b"\n") {
                            let mut file = OpenOptions::new().append(true).open(&path)?;
                            file.write_all(b"\n")?;
                            file.sync_data()?;
                        }
                    }
                    Err(_) if newest.as_ref() == Some(&path) && !line.ends_with(b"\n") => {
                        // A crash can interrupt the final append. No delivery is dispatched
                        // until its complete intent has been synced, so retaining that tail
                        // separately and recovering the preceding events is safe.
                        let quarantine = path.with_extension(format!(
                            "incomplete-{}",
                            Utc::now().timestamp_millis()
                        ));
                        fs::write(quarantine, line)?;
                        let file = OpenOptions::new().write(true).open(&path)?;
                        file.set_len(offset as u64)?;
                        file.sync_data()?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("Invalid journal {} line {}", path.display(), index + 1)
                        });
                    }
                }
                offset += line.len();
            }
        }
        let mut journal = Self { directory, model };
        if let Some(id) = legacy_rejected
            && journal
                .model
                .pending
                .as_ref()
                .is_some_and(|batch| batch.id == id)
        {
            journal.append(Event::SendFailed { id, at: Utc::now(), message: "Previous writer-conflict rejection happened before message submission. Ready to send again.".into() })?;
        }
        Ok(journal)
    }
    pub fn append(&mut self, event: Event) -> Result<()> {
        let path = self
            .directory
            .join(format!("{}.jsonl", Utc::now().format("%Y-%m-%d")));
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        file.sync_data()?;
        self.model.apply(&event);
        Ok(())
    }
}
