#![forbid(unsafe_code)]
#![cfg(unix)]
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value, json};
use std::{os::unix::net::UnixListener, thread};
use team_up::{
    delivery::{self, SendFailure},
    desktop::{read_frame, write_frame},
    model::{Batch, Destination, Segment},
};
use uuid::Uuid;
fn batch() -> Batch {
    let now = Utc::now();
    Batch {
        id: Uuid::new_v4(),
        destination: Destination {
            provider: "Codex".into(),
            id: Uuid::new_v4().to_string(),
            title: "Existing open chat".into(),
        },
        start: now,
        end: now,
        segments: vec![Segment {
            id: Uuid::new_v4(),
            source: "microphone".into(),
            start: now,
            end: now,
            text: "Only this conversation".into(),
        }],
    }
}
fn peer(
    path: &std::path::Path,
    response: Option<Value>,
) -> Result<thread::JoinHandle<Result<Vec<String>>>> {
    let listener = UnixListener::bind(path)?;
    Ok(thread::spawn(move || {
        let (mut socket, _) = listener.accept()?;
        let mut methods = vec![];
        let init = read_frame(&mut socket)?;
        methods.push(init["method"].as_str().context("Method")?.into());
        write_frame(
            &mut socket,
            &json!({"type":"response","requestId":init["requestId"],"resultType":"success","result":{"clientId":"bridge"}}),
        )?;
        let owner = read_frame(&mut socket)?;
        methods.push(owner["method"].as_str().context("Method")?.into());
        assert_eq!(owner["version"], 1);
        write_frame(
            &mut socket,
            &json!({"type":"response","requestId":owner["requestId"],"resultType":"success","handledByClientId":"existing-desktop-owner","result":{}}),
        )?;
        let send = read_frame(&mut socket)?;
        methods.push(send["method"].as_str().context("Method")?.into());
        assert_eq!(send["targetClientId"], "existing-desktop-owner");
        assert_eq!(send["version"], 2);
        assert_eq!(
            send["params"]["turnStart"]["request"]["threadId"],
            owner["params"]["conversationId"]
        );
        assert!(
            send["params"]["turnStart"]["request"]
                .get("approvalPolicy")
                .is_none()
        );
        if let Some(mut response) = response {
            response["type"] = "response".into();
            response["requestId"] = send["requestId"].clone();
            write_frame(&mut socket, &response)?;
        }
        Ok(methods)
    }))
}
#[test]
fn sends_to_existing_owner_without_resuming_or_taking_writer() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ipc.sock");
    let worker = peer(
        &path,
        Some(json!({"resultType":"success","result":{"result":{"turn":{"id":"accepted-turn"}}}})),
    )?;
    delivery::send_via_desktop(&batch(), &path)?;
    let methods = worker
        .join()
        .map_err(|_| anyhow::anyhow!("Peer failed"))??;
    assert_eq!(
        methods,
        vec![
            "initialize",
            "thread-owner-discovery",
            "thread-follower-start-turn"
        ]
    );
    Ok(())
}
#[test]
fn disconnect_after_submit_is_uncertain_and_not_retried() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ipc.sock");
    let worker = peer(&path, None)?;
    assert!(matches!(
        delivery::send_via_desktop(&batch(), &path),
        Err(SendFailure::Unconfirmed(_))
    ));
    assert_eq!(
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("Peer failed"))??
            .len(),
        3
    );
    Ok(())
}
#[test]
fn missing_desktop_is_a_definite_non_delivery() -> Result<()> {
    let dir = tempfile::tempdir()?;
    assert!(matches!(
        delivery::send_via_desktop(&batch(), &dir.path().join("absent.sock")),
        Err(SendFailure::Rejected(_))
    ));
    Ok(())
}
#[test]
fn router_rejection_before_forwarding_is_not_uncertain() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ipc.sock");
    let worker = peer(
        &path,
        Some(json!({"resultType":"error","error":"no-client-found"})),
    )?;
    assert!(matches!(
        delivery::send_via_desktop(&batch(), &path),
        Err(SendFailure::Rejected(_))
    ));
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("Peer failed"))??;
    Ok(())
}
#[test]
#[ignore = "read-only probe of a real desktop-owned chat; set TEAM_UP_OWNER_PROBE_THREAD"]
fn real_desktop_owner_can_be_addressed_without_resuming() -> Result<()> {
    let id = std::env::var("TEAM_UP_OWNER_PROBE_THREAD")?;
    let mut desktop = team_up::desktop::Desktop::discover(&team_up::desktop::socket_path()?, &id)?
        .context("No desktop owner")?;
    desktop.read_model_settings(&id)?;
    println!(
        "Existing desktop owner answered a directed read-only request; no resume or message sent."
    );
    Ok(())
}
