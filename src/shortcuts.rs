use anyhow::{Result, bail};
use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Mute,
    Send,
    Later,
    Earlier,
}
impl Action {
    pub const ALL: [Self; 4] = [Self::Mute, Self::Send, Self::Later, Self::Earlier];
    pub fn label(self) -> &'static str {
        match self {
            Self::Mute => "Mute / unmute microphone",
            Self::Send => "Send now",
            Self::Later => "Next send +1 minute",
            Self::Earlier => "Next send -1 minute",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Mute => 0,
            Self::Send => 1,
            Self::Later => 2,
            Self::Earlier => 3,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shortcuts {
    pub bindings: [String; 4],
}
impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            bindings: [
                "super+shift+KeyM",
                "super+shift+Enter",
                "super+shift+ArrowUp",
                "super+shift+ArrowDown",
            ]
            .map(str::to_owned),
        }
    }
}
impl Shortcuts {
    pub fn get(&self, action: Action) -> &str {
        &self.bindings[action.index()]
    }
    pub fn replace(&mut self, action: Action, binding: String) -> Result<()> {
        let key: HotKey = binding.parse()?;
        if self.bindings.iter().enumerate().any(|(index, other)| {
            index != action.index()
                && other
                    .parse::<HotKey>()
                    .is_ok_and(|other| other.id == key.id)
        }) {
            bail!("That shortcut is already assigned to another action.");
        }
        self.bindings[action.index()] = binding;
        Ok(())
    }
    pub fn parsed(&self) -> Result<Vec<(Action, HotKey)>> {
        Action::ALL
            .into_iter()
            .map(|action| Ok((action, self.get(action).parse()?)))
            .collect()
    }
    pub fn display(&self, action: Action) -> String {
        self.get(action)
            .replace("super+", "Cmd + ")
            .replace("meta+", "Cmd + ")
            .replace("control+", "Ctrl + ")
            .replace("alt+", "Option + ")
            .replace("shift+", "Shift + ")
            .replace("Key", "")
    }
}
pub struct Registry {
    manager: GlobalHotKeyManager,
    pub registered: Vec<(Action, HotKey)>,
}
impl Registry {
    pub fn new(shortcuts: &Shortcuts) -> Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let registered = shortcuts.parsed()?;
        for (_, key) in &registered {
            manager.register(*key)?;
        }
        Ok(Self {
            manager,
            registered,
        })
    }
    pub fn suspend(&self) -> Result<()> {
        for (_, key) in &self.registered {
            self.manager.unregister(*key)?;
        }
        Ok(())
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        for (_, key) in &self.registered {
            let _ = self.manager.unregister(*key);
        }
    }
}

pub fn from_press(key: eframe::egui::Key, modifiers: eframe::egui::Modifiers) -> Result<String> {
    let mut parts = vec![];
    if modifiers.mac_cmd {
        parts.push("super".to_owned());
    }
    if modifiers.ctrl {
        parts.push("control".to_owned());
    }
    if modifiers.alt {
        parts.push("alt".to_owned());
    }
    if parts.is_empty() {
        bail!("Include Command, Control or Option.");
    }
    if modifiers.shift {
        parts.push("shift".to_owned());
    }
    let name = key.name();
    let code = match key {
        eframe::egui::Key::Plus | eframe::egui::Key::Equals => "Equal".into(),
        eframe::egui::Key::Minus => "Minus".into(),
        _ if name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic()) => {
            format!("Key{name}")
        }
        _ if name.len() == 1 && name.chars().all(|c| c.is_ascii_digit()) => format!("Digit{name}"),
        _ => name.to_owned(),
    };
    parts.push(code);
    let binding = parts.join("+");
    let _: HotKey = binding.parse()?;
    Ok(binding)
}
