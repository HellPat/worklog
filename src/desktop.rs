//! Desktop thread-owner coordination, isolated from the public app-server protocol.
//! Framing and method versions match the installed desktop client. Never take over
//! an open thread or remove its writer lock: the existing owner performs the send.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

const FRAME_LIMIT: usize = 8 * 1024 * 1024;
#[cfg(unix)]
pub struct Desktop {
    socket: std::os::unix::net::UnixStream,
    client: String,
    owner: String,
}
#[cfg(unix)]
impl Desktop {
    pub fn discover(path: &Path, thread: &str) -> Result<Option<Self>> {
        let socket = match std::os::unix::net::UnixStream::connect(path) {
            Ok(socket) => socket,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        socket.set_read_timeout(Some(Duration::from_secs(8)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut client = Self {
            socket,
            client: "initializing-client".into(),
            owner: String::new(),
        };
        let response = client
            .request("initialize", json!({"clientType":"team-up"}), 0, false)
            .context("Desktop initialization failed")?;
        client.client = response["result"]["clientId"]
            .as_str()
            .context("Desktop did not identify the connection")?
            .into();
        let response = client.request(
            "thread-owner-discovery",
            json!({"hostId":"local","conversationId":thread}),
            1,
            false,
        )?;
        if response["resultType"] == "error" && response["error"] == "no-client-found" {
            return Ok(None);
        }
        ensure_success(&response)?;
        client.owner = response["handledByClientId"]
            .as_str()
            .context("Desktop did not identify the chat owner")?
            .into();
        Ok(Some(client))
    }
    pub fn send(
        &mut self,
        batch: &crate::model::Batch,
    ) -> std::result::Result<(), crate::delivery::SendFailure> {
        use crate::delivery::SendFailure;
        let response = self.request("thread-follower-start-turn", json!({
            "conversationId":batch.destination.id,
            "turnStart":{"request":{"threadId":batch.destination.id,"input":[{"type":"text","text":batch.prompt()}],"clientUserMessageId":batch.id.to_string()},"context":{"inheritThreadSettings":true}}
        }), 2, true).map_err(|error| SendFailure::Unconfirmed(error.to_string()))?;
        if response["resultType"] != "success" {
            let error = response["error"]
                .as_str()
                .unwrap_or("Unknown desktop error")
                .to_owned();
            return Err(
                if [
                    "no-client-found",
                    "client-cannot-handle-request",
                    "no-handler-for-request",
                    "request-version-mismatch",
                ]
                .contains(&error.as_str())
                {
                    SendFailure::Rejected(error)
                } else {
                    SendFailure::Unconfirmed(error)
                },
            );
        }
        if response["result"]["result"]["turn"]["id"]
            .as_str()
            .is_none()
        {
            return Err(SendFailure::Unconfirmed(
                "Desktop replied without a turn acknowledgement. Check the receiving chat.".into(),
            ));
        }
        Ok(())
    }
    pub fn read_model_settings(&mut self, thread: &str) -> Result<()> {
        let response = self.request(
            "thread-follower-read-model-settings",
            json!({"conversationId":thread}),
            1,
            true,
        )?;
        ensure_success(&response)
    }
    fn request(
        &mut self,
        method: &str,
        params: Value,
        version: u32,
        directed: bool,
    ) -> Result<Value> {
        self.socket
            .set_read_timeout(Some(Duration::from_secs(if directed { 30 } else { 15 })))?;
        let id = Uuid::new_v4().to_string();
        let mut request = json!({"type":"request","requestId":id,"sourceClientId":self.client,"version":version,"method":method,"params":params,"timeoutMs":if directed { 15000 } else { 1000 }});
        if directed {
            request["targetClientId"] = self.owner.clone().into();
        }
        write_frame(&mut self.socket, &request)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() >= deadline {
                bail!("Desktop request timed out");
            }
            let response = read_frame(&mut self.socket)?;
            match response["type"].as_str() {
                Some("client-discovery-request") => write_frame(
                    &mut self.socket,
                    &json!({"type":"client-discovery-response","requestId":response["requestId"],"response":{"canHandle":false}}),
                )?,
                Some("request") => write_frame(
                    &mut self.socket,
                    &json!({"type":"response","requestId":response["requestId"],"resultType":"error","error":"no-handler-for-request"}),
                )?,
                Some("response") if response["requestId"] == id => return Ok(response),
                _ => {}
            }
        }
    }
}
pub fn socket_path() -> Result<std::path::PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or(std::path::PathBuf::from(std::env::var("HOME")?).join(".codex"));
    Ok(home.join("ipc/ipc.sock"))
}
fn ensure_success(value: &Value) -> Result<()> {
    if value["resultType"] != "success" {
        bail!(
            "Desktop rejected the request: {}",
            value["error"].as_str().unwrap_or("Unknown desktop error")
        );
    }
    Ok(())
}
pub fn write_frame(writer: &mut impl Write, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > FRAME_LIMIT {
        bail!("Desktop message exceeds the frame limit");
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub fn read_frame(reader: &mut impl Read) -> Result<Value> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > FRAME_LIMIT {
        bail!("Desktop response exceeds the frame limit");
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
