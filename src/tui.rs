use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{path::Path, time::Duration};

use crate::{
    metadata::sort_key_label,
    models::{ArtViewMode, DrawInfo, PlayerState, Track, UiMode},
    player::current_elapsed,
    terminal_image::ascii_art_lines,
};

pub fn draw_loading_ui(f: &mut Frame, playlist_path: &Path, track_count: usize) {
    let area = center_rect(f.size(), 56, 7);
    let widget = Paragraph::new(format!(
        "Loading playlist...\n{}\nScanning {} track(s) and building cache",
        playlist_path.display(),
        track_count
    ))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL).title("bu-rust-mp3"));
    f.render_widget(Clear, area);
    f.render_widget(widget, area);
}

pub fn draw_ui(f: &mut Frame, state: &PlayerState, supports_kitty: bool) -> DrawInfo {
    let draw_info = match state.ui_mode {
        UiMode::Playlist => draw_playlist(f, state),
        UiMode::NowPlaying => draw_now_playing(f, state, supports_kitty),
    };
    if state.show_help {
        draw_help_popup(f);
    }
    if state.show_lyrics {
        draw_lyrics_popup(f, state);
    }
    draw_info
}

pub fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}

fn draw_help_popup(f: &mut Frame) {
    let area = center_rect(f.size(), 60, 22);
    let content = [
        "Available keys",
        "",
        "space  play / pause",
        "p      previous track",
        "n      next track",
        "l      show current lyrics",
        "b      toggle album art / ASCII art",
        "/      search playlist",
        "up/down move playlist cursor",
        "enter  play selected track",
        "esc    clear search / close help",
        "t      sort by title",
        "a      sort by artist",
        "L      sort by album",
        "s      sort by path",
        "r      toggle reverse sort",
        "R/F5   refresh playlist",
        "k      toggle kitty image mode",
        "ctrl+1/F1 playlist view",
        "ctrl+2/F2 now playing view",
        "q      quit",
        "",
        "? / esc / enter close help",
    ]
    .join("\n");
    let popup = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title("Help"))
        .alignment(Alignment::Left);
    f.render_widget(Clear, area);
    f.render_widget(popup, area);
}

fn draw_lyrics_popup(f: &mut Frame, state: &PlayerState) {
    let track = &state.tracks[state.current];
    let area = center_rect(f.size(), 84, 28);
    let title = track.title.as_deref().unwrap_or_else(|| {
        track
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
    });
    let content = track
        .lyrics
        .as_deref()
        .unwrap_or("No lyrics found in MP3 metadata.");
    let popup = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Lyrics | {}", title)),
        )
        .scroll((state.lyrics_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(Clear, area);
    f.render_widget(popup, area);
}

fn draw_playlist(f: &mut Frame, state: &PlayerState) -> DrawInfo {
    let size = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(size);

    let title = format!(
        "bu-rust-mp3  |  {} / {}  |  sort: {}{}",
        state.current + 1,
        state.tracks.len(),
        sort_key_label(state.sort_key),
        if state.sort_reverse { " (rev)" } else { "" }
    );
    let header = Paragraph::new(title).block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    let track = &state.tracks[state.current];
    let status = if state.paused { "Paused" } else { "Playing" };
    let display_title = track.title.as_deref().unwrap_or_else(|| {
        track
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
    });
    let now_playing = format!("{}: {}", status, display_title);
    let now = Paragraph::new(now_playing).block(Block::default().borders(Borders::ALL));
    f.render_widget(now, chunks[1]);

    render_progress(f, chunks[2], state);

    let search_label = if state.search_mode {
        format!("Search: {}_", state.search_query)
    } else if state.search_query.is_empty() {
        "Search: /".to_string()
    } else {
        format!("Search: {}", state.search_query)
    };
    let search = Paragraph::new(search_label).block(Block::default().borders(Borders::ALL));
    f.render_widget(search, chunks[3]);

    let items = state
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let name = t
                .title
                .as_deref()
                .unwrap_or_else(|| t.path.file_name().and_then(|s| s.to_str()).unwrap_or("?"));
            let prefix = if i == state.current { "▶ " } else { "  " };
            let text = if i == state.current {
                format!("{}{}  [playing]", prefix, name)
            } else {
                format!("{}{}", prefix, name)
            };
            ListItem::new(text)
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Playlist"))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected));
    f.render_stateful_widget(list, chunks[4], &mut list_state);

    let help = Paragraph::new("Controls: space play/pause | p prev | n next | ? help");
    f.render_widget(help, chunks[5]);
    DrawInfo::default()
}

fn draw_now_playing(f: &mut Frame, state: &PlayerState, supports_kitty: bool) -> DrawInfo {
    let size = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Length(5),
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(size);

    let title = format!(
        "bu-rust-mp3  |  {} / {}  |  sort: {}{}",
        state.current + 1,
        state.tracks.len(),
        sort_key_label(state.sort_key),
        if state.sort_reverse { " (rev)" } else { "" }
    );
    let header = Paragraph::new(title).block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    let track = &state.tracks[state.current];
    let status = if state.paused { "Paused" } else { "Playing" };
    let display_title = track.title.as_deref().unwrap_or_else(|| {
        track
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
    });
    let artist = track.artist.as_deref().unwrap_or("-");
    let album = track.album.as_deref().unwrap_or("-");
    let art_status = if track.art_png.is_some() {
        "embedded"
    } else {
        "none"
    };
    let kitty_status = if supports_kitty || state.force_kitty {
        if state.force_kitty {
            "on (forced)"
        } else {
            "on"
        }
    } else {
        "off"
    };
    let art_view = match state.art_view_mode {
        ArtViewMode::AlbumArt => "album art",
        ArtViewMode::AsciiArt => "ascii",
    };
    let info = format!(
        "{}\nTitle : {}\nArtist: {}\nAlbum : {}\nArt   : {} | View: {} | Kitty: {}",
        status, display_title, artist, album, art_status, art_view, kitty_status
    );
    let info = Paragraph::new(info).block(Block::default().borders(Borders::ALL));
    f.render_widget(info, chunks[1]);

    let art_block = Block::default().borders(Borders::ALL).title("Album Art");
    let art_inner = art_block.inner(chunks[2]);
    f.render_widget(art_block, chunks[2]);

    let use_kitty = state.art_view_mode == ArtViewMode::AlbumArt
        && (supports_kitty || state.force_kitty)
        && track.art_png.is_some();
    let art_rect = square_art_rect(art_inner);
    if !use_kitty {
        render_art_fallback(f, art_rect, track, state, supports_kitty);
    }

    render_progress(f, chunks[3], state);

    let help =
        Paragraph::new("Controls: space play/pause | p prev | n next | b art/ascii | ? help");
    f.render_widget(help, chunks[4]);

    DrawInfo {
        art_rect: Some(art_rect),
        use_kitty,
    }
}

fn render_progress(f: &mut Frame, area: Rect, state: &PlayerState) {
    let elapsed = current_elapsed(state);
    let gauge = if let Some(total) = state.total_duration {
        let ratio = (elapsed.as_secs_f64() / total.as_secs_f64()).min(1.0);
        Gauge::default()
            .block(Block::default().borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(ratio)
            .label(format!(
                "{} / {}",
                fmt_duration(elapsed),
                fmt_duration(total)
            ))
    } else {
        Gauge::default()
            .block(Block::default().borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(0.0)
            .label(fmt_duration(elapsed))
    };
    f.render_widget(gauge, area);
}

fn render_art_fallback(
    f: &mut Frame,
    art_rect: Rect,
    track: &Track,
    state: &PlayerState,
    supports_kitty: bool,
) {
    if let Some(png) = track.art_png.as_deref() {
        match state.art_view_mode {
            ArtViewMode::AsciiArt => {
                if let Some(lines) = ascii_art_lines(png, art_rect.width, art_rect.height) {
                    let art = Paragraph::new(lines.join("\n"));
                    f.render_widget(art, art_rect);
                } else {
                    let art = Paragraph::new("No art").alignment(Alignment::Center);
                    f.render_widget(art, art_rect);
                }
            }
            ArtViewMode::AlbumArt => {
                let message = if supports_kitty || state.force_kitty {
                    "Album art unavailable"
                } else {
                    "Album art view needs Kitty image support.\nPress b for ASCII art."
                };
                let art = Paragraph::new(message)
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true });
                f.render_widget(art, art_rect);
            }
        }
    } else {
        let art = Paragraph::new("No art").alignment(Alignment::Center);
        f.render_widget(art, art_rect);
    }
}

fn center_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn square_art_rect(area: Rect) -> Rect {
    let Some(cell_aspect_ratio) = terminal_cell_aspect_ratio() else {
        let square = area.width.min(area.height);
        return center_rect(area, square, square);
    };

    let max_height_by_width = (area.width as f32 / cell_aspect_ratio).floor() as u16;
    let height = area.height.min(max_height_by_width).max(1);
    let width = ((height as f32) * cell_aspect_ratio)
        .round()
        .clamp(1.0, area.width as f32) as u16;

    center_rect(area, width, height)
}

fn terminal_cell_aspect_ratio() -> Option<f32> {
    let window = crossterm::terminal::window_size().ok()?;
    if window.columns == 0 || window.rows == 0 || window.width == 0 || window.height == 0 {
        return None;
    }

    let cell_width = window.width as f32 / window.columns as f32;
    let cell_height = window.height as f32 / window.rows as f32;
    if cell_width <= 0.0 || cell_height <= 0.0 {
        return None;
    }

    Some(cell_height / cell_width)
}
