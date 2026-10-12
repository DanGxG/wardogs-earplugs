#![cfg_attr(not(test), windows_subsystem = "windows")]

mod audio;
mod model;
mod overlay;
mod settings;
mod ui;
mod update;

fn main() {
    let result = update::helper_mode().unwrap_or_else(ui::run);
    if let Err(error) = result {
        ui::startup_error(&error);
    }
}
