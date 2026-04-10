use rodio::{OutputStreamHandle, Sink};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Track {
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub lyrics: Option<String>,
    pub art_png: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
pub enum SortKey {
    Path,
    Name,
    Mtime,
    Title,
    Artist,
    Album,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Playlist,
    NowPlaying,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArtViewMode {
    AlbumArt,
    AsciiArt,
}

pub struct Args {
    pub path: PathBuf,
    pub sort: SortKey,
    pub reverse: bool,
}

#[derive(Clone)]
pub struct ScannedTrack {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_secs: u64,
}

pub struct PlayerState {
    pub playlist_path: PathBuf,
    pub tracks: Vec<Track>,
    pub current: usize,
    pub selected: usize,
    pub search_query: String,
    pub search_mode: bool,
    pub show_help: bool,
    pub show_lyrics: bool,
    pub lyrics_scroll: u16,
    pub paused: bool,
    pub started_at: Option<Instant>,
    pub elapsed_before_pause: Duration,
    pub total_duration: Option<Duration>,
    pub sink: Sink,
    pub stream_handle: OutputStreamHandle,
    pub sort_key: SortKey,
    pub sort_reverse: bool,
    pub ui_mode: UiMode,
    pub force_kitty: bool,
    pub art_view_mode: ArtViewMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ArtSig {
    pub index: usize,
    pub width: u16,
    pub height: u16,
}

#[derive(Default)]
pub struct DrawInfo {
    pub art_rect: Option<ratatui::prelude::Rect>,
    pub use_kitty: bool,
}

pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub lyrics: Option<String>,
    pub art_png: Option<Vec<u8>>,
}
