# Team-up 0.2.1 validation — 6 November 2026

This update fixes delivery to a chat already owned by the desktop app. The prior 0.2.0 test created and owned its own receiving agent, so it did not cover the desktop writer conflict reported during normal use.

## Delivery contract

The production sending path contains no `thread/resume`, fork, takeover or private-agent fallback. It discovers the existing desktop owner, sends the selected transcript through that owner, and requires its turn acknowledgement. A missing owner is a definite non-delivery; **Open in Codex** is an explicit user action. Connection loss after submission stays uncertain and is never automatically retried.

Known legacy writer-conflict rejections are recovered as unsent without advancing the range or last-send timer. The old error was raised before submission; recovery does not send anything automatically.

## Results

- 15 ordinary regression tests passed; formatting and strict Clippy passed.
- Live UI → actual macOS output capture → Nemotron → existing desktop chat owner → acknowledged manual send and automatic second send passed (35.14 seconds). Persisted receiving-chat history contained both messages and an agent response.
- Three additional real-audio tests passed: output capture with retained WAV, Nemotron journal recovery, and Whisper without retained audio (29.49 seconds).
- The live test exposed a genuine capture overflow during finalization. Capture now buffers up to 35 seconds by sample count rather than 64 device callbacks; inference has a 30-second finalization timeout. A regression test checks different callback sizes and the memory bound.
- Installed 0.2.1 at `~/Applications/Team-up.app`, verified its signature and executable match the release build, opened it, inspected the actual main panel and settings, and verified the focused mute shortcut in both directions. Recording was stopped and auto-send off during installation. Existing preferences and recordings were preserved; the previous app bundle was backed up by the installer.
- Global shortcut registration and assignment are tested, but a physical shortcut while another app is focused still needs a keyboard check: the computer-use tool sends app-scoped keys, which do not prove OS-level global dispatch.

## Coverage

- Ordinary regression suite: owner routing, no-owner rejection, dropped acknowledgement, pre-forwarding rejection, UI controls, shortcuts, storage, range isolation, restart recovery and legacy rejection recovery.
- A read-only directed request to the actual desktop owner of the reported open chat passed, without resuming it or sending content.
- Live UI-to-agent test: creates a dedicated destination, explicitly opens it in Codex, verifies a desktop owner exists, captures actual output audio, clicks Send without stopping recording, verifies the transcript and an agent response, then verifies an automatic second send to the same chat.
- Formatting and strict Clippy include forbidden unsafe code and denied unwrap/expect in our crates.
- Appearance uses pinned `egui_native_look = 0.2.0` controls, platform fonts, accent and system light/dark appearance. The structure is retained; the prototype's custom palette is removed.

Current output: `target/validation/followup-tests.log`, `followup-clippy.log`, `followup-live-delivery.log`, `followup-real-audio.log`, `desktop-owner-probe.log`, `followup-bundle.log`, and `installed-0.2.1.png`. These distinguish the new desktop-owner tests from the earlier private-agent tests.

The desktop coordination protocol is isolated in `src/desktop.rs` because it is versioned by the installed desktop application. The local socket is used only to address its existing owner. UI inspection of Codex itself is restricted by the computer-use tool; receiving-message verification uses the local protocol and actual persisted chat history.

The supported destination remains Codex; model backends remain Nemotron and Whisper. Source and old recordings remain backed up outside the project as documented in README history and the earlier archive.
