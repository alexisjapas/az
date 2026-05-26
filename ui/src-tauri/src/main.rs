// Empêche d'ouvrir une console supplémentaire sous Windows en mode release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    az_ui_lib::run()
}
