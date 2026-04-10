use anyhow::{Context, Result};
use rodio::{Decoder, Sink, Source};
use std::{
    fs::File,
    io::BufReader,
    time::{Duration, Instant},
};

use crate::{
    cache::{playlist_cache_path, save_playlist_cache, save_resume_state},
    metadata::{build_tracks_from_scan, sort_tracks},
    models::{Args, PlayerState, Track},
    scanner::scan_playlist,
    tui::draw_loading_ui,
};

pub fn refresh_playlist(
    state: &mut PlayerState,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> Result<()> {
    let args = Args {
        path: state.playlist_path.clone(),
        sort: state.sort_key,
        reverse: state.sort_reverse,
    };
    let scanned = scan_playlist(&args)?;
    if scanned.is_empty() {
        anyhow::bail!("No .mp3 files found in {}", state.playlist_path.display());
    }

    terminal
        .draw(|f| draw_loading_ui(f, &state.playlist_path, scanned.len()))
        .context("draw loading UI")?;

    let current_path = state.tracks.get(state.current).map(|t| t.path.clone());
    let selected_path = state.tracks.get(state.selected).map(|t| t.path.clone());
    let tracks = build_tracks_from_scan(&scanned, args.sort, args.reverse);
    let cache_path = playlist_cache_path(&state.playlist_path);
    save_playlist_cache(&cache_path, &state.playlist_path, &scanned, &tracks).ok();

    state.tracks = tracks;

    if let Some(path) = current_path {
        if let Some(index) = state.tracks.iter().position(|track| track.path == path) {
            state.current = index;
        } else {
            state.current = state.selected.min(state.tracks.len().saturating_sub(1));
            load_track(state)?;
        }
    }

    if let Some(path) = selected_path {
        if let Some(index) = state.tracks.iter().position(|track| track.path == path) {
            state.selected = index;
        } else {
            state.selected = state.current.min(state.tracks.len().saturating_sub(1));
        }
    } else {
        state.selected = state.current.min(state.tracks.len().saturating_sub(1));
    }

    save_resume_state(state).ok();
    Ok(())
}

pub fn load_track(state: &mut PlayerState) -> Result<()> {
    state.sink.stop();
    state.sink = Sink::try_new(&state.stream_handle).context("audio sink")?;
    state.elapsed_before_pause = Duration::ZERO;
    state.started_at = Some(Instant::now());
    state.paused = false;

    let track = &state.tracks[state.current];
    let file = File::open(&track.path).with_context(|| track.path.display().to_string())?;
    let decoder = Decoder::new(BufReader::new(file)).context("decode mp3")?;
    state.total_duration = decoder.total_duration();
    state.sink.append(decoder);
    state.sink.play();
    state.selected = state.current;
    save_resume_state(state).ok();
    Ok(())
}

pub fn toggle_pause(state: &mut PlayerState) {
    if state.paused {
        state.paused = false;
        state.started_at = Some(Instant::now());
        state.sink.play();
    } else {
        state.paused = true;
        if let Some(started) = state.started_at.take() {
            state.elapsed_before_pause += started.elapsed();
        }
        state.sink.pause();
    }
}

pub fn next_track(state: &mut PlayerState) -> Result<()> {
    if state.current + 1 < state.tracks.len() {
        state.current += 1;
        load_track(state)?;
    }
    Ok(())
}

pub fn prev_track(state: &mut PlayerState) -> Result<()> {
    if state.current > 0 {
        state.current -= 1;
        load_track(state)?;
    }
    Ok(())
}

pub fn advance_if_possible(state: &mut PlayerState) -> Result<bool> {
    if state.current + 1 < state.tracks.len() {
        state.current += 1;
        load_track(state)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn move_playlist_selection(state: &mut PlayerState, delta: isize) {
    let last = state.tracks.len().saturating_sub(1) as isize;
    let next = (state.selected as isize + delta).clamp(0, last);
    state.selected = next as usize;
}

pub fn handle_playlist_search_key(
    state: &mut PlayerState,
    key: crossterm::event::KeyEvent,
) -> Result<bool> {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !state.search_mode {
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc => {
            state.search_mode = false;
            state.search_query.clear();
            Ok(true)
        }
        KeyCode::Enter => {
            state.search_mode = false;
            if state.search_query.is_empty() {
                return play_selected_track(state).map(|_| true);
            }
            Ok(true)
        }
        KeyCode::Backspace => {
            state.search_query.pop();
            apply_search_selection(state);
            Ok(true)
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.search_query.push(c);
            apply_search_selection(state);
            Ok(true)
        }
        _ => Ok(true),
    }
}

pub fn play_selected_track(state: &mut PlayerState) -> Result<()> {
    if state.selected != state.current {
        state.current = state.selected;
        load_track(state)?;
    }
    Ok(())
}

pub fn current_elapsed(state: &PlayerState) -> Duration {
    if state.paused {
        state.elapsed_before_pause
    } else if let Some(started) = state.started_at {
        state.elapsed_before_pause + started.elapsed()
    } else {
        state.elapsed_before_pause
    }
}

pub fn scroll_lyrics(state: &mut PlayerState, delta: i16) {
    let current = state.lyrics_scroll as i32;
    let max_scroll = max_lyrics_scroll(&state.tracks[state.current]) as i32;
    let next = (current + delta as i32).clamp(0, max_scroll);
    state.lyrics_scroll = next as u16;
}

pub fn max_lyrics_scroll(track: &Track) -> u16 {
    track
        .lyrics
        .as_deref()
        .map(|lyrics| lyrics.lines().count().saturating_sub(24) as u16)
        .unwrap_or(0)
}

pub fn resort_playlist(state: &mut PlayerState, sort: crate::models::SortKey) {
    let current_path = state.tracks[state.current].path.clone();
    let selected_path = state.tracks[state.selected].path.clone();
    state.sort_key = sort;
    sort_tracks(&mut state.tracks, state.sort_key, state.sort_reverse);
    if let Some(idx) = state.tracks.iter().position(|t| t.path == current_path) {
        state.current = idx;
    }
    if let Some(idx) = state.tracks.iter().position(|t| t.path == selected_path) {
        state.selected = idx;
    }
}

pub fn toggle_reverse(state: &mut PlayerState) {
    let current_path = state.tracks[state.current].path.clone();
    let selected_path = state.tracks[state.selected].path.clone();
    state.sort_reverse = !state.sort_reverse;
    sort_tracks(&mut state.tracks, state.sort_key, state.sort_reverse);
    if let Some(idx) = state.tracks.iter().position(|t| t.path == current_path) {
        state.current = idx;
    }
    if let Some(idx) = state.tracks.iter().position(|t| t.path == selected_path) {
        state.selected = idx;
    }
}

fn apply_search_selection(state: &mut PlayerState) {
    let query = state.search_query.trim();
    if query.is_empty() {
        return;
    }
    if let Some(index) = state
        .tracks
        .iter()
        .position(|track| track_matches_query(track, query))
    {
        state.selected = index;
    }
}

fn track_matches_query(track: &Track, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    let title = track.title.as_deref().unwrap_or("");
    let artist = track.artist.as_deref().unwrap_or("");
    let album = track.album.as_deref().unwrap_or("");
    title.to_ascii_lowercase().contains(&query)
        || artist.to_ascii_lowercase().contains(&query)
        || album.to_ascii_lowercase().contains(&query)
}
