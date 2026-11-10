# Team-up 2

A small macOS menu bar recorder built in Rust with egui, eframe, tray-icon and egui_native_look. Controls, fonts, accent colour, and light/dark appearance follow the macOS look through the control library; these remain Rust-rendered controls. Local speech recognition, an append-only day log, and bounded transcript delivery to a chosen Codex chat. No webview or database.

## Use

Start recording, then choose a chat already open in Codex. Use **Open in Codex** for a new or closed chat. **Send now** finalizes the current audio without stopping capture and delivers the finalized transcript for this conversation. Acknowledged speech is never sent twice. The button shows time since the last acknowledged send. Auto-send defaults to seven minutes and is off until explicitly enabled. Its +/− controls change only the next deadline.

Choosing another chat starts a new conversation boundary and disables auto-send. Earlier day-log content stays local. The date/time field narrows the send start within the current conversation. Changing that field does not reset the last-send timer.

Microphone mute leaves system audio running. **Keep recorded audio** is off by default and can be changed during recording. Transcripts are always saved; enabled audio retention writes separate PCM16 WAV files for microphone and system audio. Closing/hiding the panel keeps recording. **Quit Team-up** finishes the audio stream before exiting.

The top-right settings icon opens device, model, folder, shortcut and menu-bar preferences. Click a shortcut and press your keys; Escape cancels. Shortcuts require Command, Control or Option, and duplicate assignments are rejected. Capture settings and storage moves require recording to be stopped; mute and retention remain live controls. The tray shows input/output levels and the optional auto-send countdown.

## Local files and recovery

The chosen folder contains daily `YYYY-MM-DD.jsonl` journals. Events include finalized text with capture timestamps/source, conversation changes, microphone mute, recording changes, Send clicks, frozen send batches and delivery acknowledgements. Only selected transcript text is sent, never the day-log file or its path. Optional audio is in `audio/`.

An interrupted final append is quarantined beside the journal. An interrupted or unconfirmed send requires checking the receiving chat and choosing **It arrived** or **It did not arrive**. The app never automatically retries an uncertain delivery. A phrase spanning a conversation cutoff is excluded rather than leaking earlier speech into the new conversation.

Preferences: `~/Library/Application Support/team-up/tray-settings.json`. One process owns recording at a time. Auto-send does not resume automatically after relaunch.

## Installed prerequisites

This build uses the already-installed local dependencies on this Mac:

- Nemotron 3.5 Streaming Q8_0 through the installed NeMo-Speech 0.3.0 executable, with an automatically managed loopback-only streaming server.
- Parakeet TDT 0.6B v3 Q8_0 through NeMo-Speech 0.3.0, using the local complete-audio endpoint for bounded chunks.
- Whisper large-v3-turbo Q5_0 through `/opt/homebrew/bin/whisper-cli`, finalizing bounded chunks. Temporary inference WAVs are removed afterward.
- `/opt/homebrew/bin/codex` for chat metadata, optional creation and read-back verification. Message delivery uses the running desktop application's local owner/follower coordination socket. Team-up never resumes, forks or takes over the selected chat, starts a private agent for sending, or removes writer locks. The existing desktop owner handles the message and its approvals. If no owner is available, nothing is sent and the transcript remains ready for retry. **Open in Codex** is an explicit navigation action, never an automatic delivery fallback.

The model files and original recordings were preserved. Whistle's model remains on disk, but its former unsafe FFI adapter is not compiled into this rewrite. The destination adapter in this build is Codex; Claude, ChatGPT desktop and OpenCode adapters are not implemented. The UI/core are separable for later platform and adapter work, but macOS is the currently validated platform. System-output capture requires macOS 14.6 or later and OS capture permissions.

## Build and validate

Dependencies are pinned in `Cargo.toml` and `Cargo.lock`. Our crates forbid unsafe code; Clippy denies unwrap and expect. Dependencies contain their own native platform bindings.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --tests
cargo test --test desktop_delivery
cargo test --test real_audio -- --ignored --nocapture --test-threads=1
cargo test --test destinations -- --ignored --nocapture
TEAM_UP_LIVE_DELIVERY_E2E=1 cargo test --test live_delivery -- --ignored --nocapture
./scripts/bundle.sh
```

The real-audio tests load actual local models and play speech through actual system output. They require finalization **before** capture stops, verify send-time flushing, and check WAV retention. Run audio tests sequentially so concurrent playback cannot contaminate another test.

The live-delivery test operates the actual egui controls to create a dedicated chat, explicitly open it in the real desktop app, verify that desktop ownership exists, start capture, send while recording remains active, and verify that the speech appears in the receiving chat and its agent responds. A second automatic send must reach the same desktop-owned chat. Socket regression tests check that no resume or takeover occurs and that a lost acknowledgement remains uncertain. Tests that bind local Unix sockets need local IPC permission when run inside a sandbox. It sends only the supplied test speech and requires explicit opt-in because it uses a real agent. Ordinary tests never send cloud messages.

The signed local bundle is built at `target/Team-up.app`. It is ad-hoc signed for this Mac, not notarized for distribution. Development and generated output stay under `target/`.
