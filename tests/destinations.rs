#![forbid(unsafe_code)]
#[test]
#[ignore = "reads chat metadata from the installed Codex daemon without sending any message"]
fn installed_codex_lists_existing_chats() -> anyhow::Result<()> {
    let chats = team_up::delivery::list()?;
    anyhow::ensure!(!chats.is_empty(), "Codex returned no chats");
    anyhow::ensure!(
        chats.iter().all(|chat| !chat.id.is_empty()),
        "Chat lacks an identity"
    );
    println!(
        "Found {} existing Codex chats; no messages sent",
        chats.len()
    );
    Ok(())
}
