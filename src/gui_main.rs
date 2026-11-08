//! MP3CutAd GUI 入口（需要 `gui` feature）

#![windows_subsystem = "windows"]

fn main() -> eframe::Result<()> {
    mp3cutad_rs::gui::run()
}
