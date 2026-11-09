#![forbid(unsafe_code)]
use team_up::shortcuts::{Action, Shortcuts, from_press};
#[test]
fn shortcut_capture_requires_modifier_and_rejects_duplicates() -> anyhow::Result<()> {
    use eframe::egui::{Key, Modifiers};
    let mut shortcuts = Shortcuts::default();
    assert!(from_press(Key::M, Modifiers::NONE).is_err());
    let binding = from_press(
        Key::K,
        Modifiers {
            mac_cmd: true,
            command: true,
            shift: true,
            ..Modifiers::NONE
        },
    )?;
    shortcuts.replace(Action::Mute, binding.clone())?;
    assert_eq!(shortcuts.get(Action::Mute), binding);
    assert!(shortcuts.replace(Action::Send, binding).is_err());
    assert!(shortcuts.parsed()?.len() == 4);
    Ok(())
}
