use anyhow::{Result, ensure};
use std::time::Duration;
use xa11y::{App, AppExt, DragOptions};
fn main() -> Result<()> {
    let app = App::by_name("team-up-gpui-spike", Duration::from_secs(5))?;
    let command = std::env::args().nth(1).unwrap_or_else(|| "tree".into());
    app.locator("window").activate()?;
    std::thread::sleep(Duration::from_millis(350));
    match command.as_str() {
        "toggle" => {
            if app
                .locator("button[name='● Simulated recording · Stop']")
                .count()?
                > 0
            {
                app.locator("button[name='● Simulated recording · Stop']")
                    .press()?;
                std::thread::sleep(Duration::from_millis(350));
            }
            app.locator("button[name='Start simulation']").press()?;
            std::thread::sleep(Duration::from_millis(350));
            app.locator("button[name='● Simulated recording · Stop']")
                .wait_visible(Duration::from_secs(3))?;
            println!("PASS: simulation toggled through AX action");
        }
        "settings" => {
            app.locator("button[name='Settings']").press()?;
            std::thread::sleep(Duration::from_millis(350));
            app.locator("button[name='Engine: Nemotron']")
                .wait_visible(Duration::from_secs(3))?;
            app.locator("button[name='Close']").press()?;
            std::thread::sleep(Duration::from_millis(350));
            println!("PASS: settings opened and closed");
        }
        "chart" | "track" => {
            let name = if command == "chart" {
                "Voice activity chart"
            } else {
                "Transcript range selector"
            };
            let element = app.locator(&format!("group[name='{name}']")).element()?;
            let b = element
                .bounds
                .ok_or_else(|| anyhow::anyhow!("Region has no bounds"))?;
            println!("{name}: {b:?}");
            let (a, z) = if command == "chart" {
                (
                    (b.x as f64 + b.width as f64 * 0.86, b.y as f64 + 50.),
                    (b.x as f64 + b.width as f64 * 0.96, b.y as f64 + 50.),
                )
            } else {
                (
                    (b.x as f64 + b.width as f64 / 2., b.y as f64 + 15.),
                    (b.x as f64 + b.width as f64 / 2., b.y as f64 + 220.),
                )
            };
            xa11y::input_sim()?
                .mouse()
                .move_to((a.0 as i32, a.1 as i32))?;
            std::thread::sleep(Duration::from_millis(350));
            xa11y::input_sim()?.mouse().drag_with(
                (a.0 as i32, a.1 as i32),
                (z.0 as i32, z.1 as i32),
                DragOptions::new().duration(Duration::from_millis(600)),
            )?;
            std::thread::sleep(Duration::from_millis(350));
            app.locator("button[name='Save annotation']")
                .wait_visible(Duration::from_secs(3))?;
            println!("PASS: xa11y drag opened note popup");
        }
        "save" => {
            app.locator("text_field[name='Annotation label']")
                .set_value("xa11y planning")?;
            app.locator("text_field[name='Annotation note']")
                .set_value("Saved through semantic UI actions")?;
            ensure!(
                app.locator("text_field[name='Annotation label']")
                    .element()?
                    .value
                    .as_deref()
                    == Some("xa11y planning"),
                "Field value did not update"
            );
            app.locator("button[name='Save annotation']").press()?;
            std::thread::sleep(Duration::from_millis(350));
            app.locator("button[name='Save annotation']")
                .wait_detached(Duration::from_secs(3))?;
            println!("PASS: text inputs set and Save pressed");
        }
        "back" => app.locator("button[name='↓ Back to live']").press()?,
        "screenshot" => xa11y::screenshot_element(&app.locator("window").element()?)?
            .save_png("evidence/xa11y-window.png")?,
        _ => {}
    }
    println!("{:#?}", app.tree(Some(15))?);
    Ok(())
}
