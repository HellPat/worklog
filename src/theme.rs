//! Use the control library's macOS metrics, fonts, palette, accent and appearance.
use eframe::egui;
pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_native_look::install_fonts(&mut fonts, egui_native_look::Look::system());
    ctx.set_fonts(fonts);
    egui_native_look::install(ctx, egui_native_look::Theme::system());
    ctx.set_theme(egui::ThemePreference::System);
    egui_native_look::follow_system(ctx);
}
