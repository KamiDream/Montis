mod app;
mod audio_cache;
mod avatar;
mod cover_cache;
mod download_store;
mod library_cache;
mod liked_store;
mod local_files;
mod player_store;
mod ring;
mod theme;
mod ui_settings;
mod views;
mod visualizer;
mod viz;
mod widgets;

fn main() -> iced::Result {
    app::run()
}
