#![forbid(unsafe_code)]
use team_up::ui::Panel;
fn main() -> eframe::Result {
    let home = std::env::var("HOME").map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let directory = std::path::PathBuf::from(home).join("Library/Application Support/team-up");
    std::fs::create_dir_all(&directory)
        .map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join("tray.lock"))
        .map_err(|error| eframe::Error::AppCreation(error.into()))?;
    lock.try_lock().map_err(|error| {
        eframe::Error::AppCreation(format!("Team-up is already running: {error}").into())
    })?;
    let panel = Panel::open().map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Team-up")
            .with_inner_size([380.0, 520.0])
            .with_resizable(false)
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "Team-up",
        options,
        Box::new(move |cc| {
            team_up::theme::apply(&cc.egui_ctx);
            Ok(Box::new(panel))
        }),
    )
}
