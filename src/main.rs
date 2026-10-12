#![cfg_attr(not(test), windows_subsystem = "windows")]

mod audio;
mod model;
mod overlay;
mod settings;
mod ui;

fn main() {
    if let Err(error) = ui::run() {
        ui::startup_error(&error);
    }
}
