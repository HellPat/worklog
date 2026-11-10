//! Screen transitions consume the old state, as in Cliffle's typestate pattern.
//! The runtime enum only selects which typed screen GPUI renders.
//!
//! Settings has no workspace operations:
//! ```compile_fail
//! use team_up_gpui_spike::navigation::DayScreen;
//! let settings = DayScreen::default().settings();
//! settings.start_span();
//! ```
#[derive(Default, Debug, PartialEq)]
pub struct DayScreen;
#[derive(Debug, PartialEq)]
pub struct StartScreen;
#[derive(Debug, PartialEq)]
pub struct RefineScreen;
#[derive(Debug, PartialEq)]
pub struct SettingsScreen {
    return_to: Workspace,
}
impl DayScreen {
    pub fn start_span(self) -> StartScreen {
        StartScreen
    }
    pub fn refine(self) -> RefineScreen {
        RefineScreen
    }
    pub fn settings(self) -> SettingsScreen {
        SettingsScreen {
            return_to: Workspace::Day(self),
        }
    }
}
impl StartScreen {
    pub fn begin(self) -> DayScreen {
        DayScreen
    }
    pub fn settings(self) -> SettingsScreen {
        SettingsScreen {
            return_to: Workspace::Start(self),
        }
    }
}
impl RefineScreen {
    pub fn finish(self) -> DayScreen {
        DayScreen
    }
    pub fn settings(self) -> SettingsScreen {
        SettingsScreen {
            return_to: Workspace::Refine(self),
        }
    }
}
impl SettingsScreen {
    pub fn close(self) -> Workspace {
        self.return_to
    }
}
#[derive(Debug, PartialEq)]
pub enum Workspace {
    Day(DayScreen),
    Start(StartScreen),
    Refine(RefineScreen),
}
#[derive(Debug, PartialEq)]
pub enum Navigation {
    Workspace(Workspace),
    Settings(SettingsScreen),
}
impl Default for Navigation {
    fn default() -> Self {
        Self::Workspace(Workspace::Day(DayScreen))
    }
}
impl Navigation {
    pub fn is_settings(&self) -> bool {
        matches!(self, Self::Settings(_))
    }
    pub fn is_starting(&self) -> bool {
        matches!(self, Self::Workspace(Workspace::Start(_)))
    }
    pub fn is_refining(&self) -> bool {
        matches!(self, Self::Workspace(Workspace::Refine(_)))
    }
    pub fn open_settings(&mut self) {
        *self = match std::mem::take(self) {
            Self::Workspace(Workspace::Day(s)) => Self::Settings(s.settings()),
            Self::Workspace(Workspace::Start(s)) => Self::Settings(s.settings()),
            Self::Workspace(Workspace::Refine(s)) => Self::Settings(s.settings()),
            settings => settings,
        };
    }
    pub fn back(&mut self) {
        *self = match std::mem::take(self) {
            Self::Settings(s) => Self::Workspace(s.close()),
            Self::Workspace(Workspace::Start(s)) => Self::Workspace(Workspace::Day(s.begin())),
            Self::Workspace(Workspace::Refine(s)) => Self::Workspace(Workspace::Day(s.finish())),
            day => day,
        };
    }
    pub fn start(&mut self) {
        if matches!(self, Self::Workspace(Workspace::Day(_))) {
            let Self::Workspace(Workspace::Day(day)) = std::mem::take(self) else {
                return;
            };
            *self = Self::Workspace(Workspace::Start(day.start_span()));
        }
    }
    pub fn refine(&mut self) {
        if matches!(self, Self::Workspace(Workspace::Day(_))) {
            let Self::Workspace(Workspace::Day(day)) = std::mem::take(self) else {
                return;
            };
            *self = Self::Workspace(Workspace::Refine(day.refine()));
        }
    }
    pub fn show_day(&mut self) {
        *self = Self::default();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_restores_each_workspace_without_allowing_start() {
        for workspace in [
            Workspace::Day(DayScreen),
            Workspace::Start(StartScreen),
            Workspace::Refine(RefineScreen),
        ] {
            let mut nav = Navigation::Workspace(workspace);
            let starting = nav.is_starting();
            let refining = nav.is_refining();
            nav.open_settings();
            nav.start();
            nav.refine();
            assert!(nav.is_settings());
            nav.back();
            assert_eq!(nav.is_starting(), starting);
            assert_eq!(nav.is_refining(), refining);
        }
    }
}
