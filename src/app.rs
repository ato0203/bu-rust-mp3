use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use rodio::{OutputStream, Sink};
use std::{
    io,
    time::{Duration, Instant},
};

use crate::{
    cache::{
        load_cached_tracks, playlist_cache_path, restore_last_played_index, save_playlist_cache,
    },
    metadata::build_tracks_from_scan,
    models::{Args, ArtSig, ArtViewMode, PlayerState, SortKey, UiMode},
    player::{
        advance_if_possible, handle_playlist_search_key, load_track, max_lyrics_scroll,
        move_playlist_selection, next_track, play_selected_track, prev_track, refresh_playlist,
        resort_playlist, scroll_lyrics, toggle_pause, toggle_reverse,
    },
    scanner::scan_playlist,
    terminal_image::{clear_kitty_image, env_force_kitty, render_kitty_image, supports_kitty},
    tui::{draw_loading_ui, draw_ui},
};

pub fn run_app(args: Args, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let playlist_path = args.path.clone();
    let scanned = scan_playlist(&args)?;
    if scanned.is_empty() {
        anyhow::bail!("No .mp3 files found in {}", args.path.display());
    }

    let cache_path = playlist_cache_path(&args.path);
    let tracks = if let Some(tracks) = load_cached_tracks(&cache_path, &args, &scanned)? {
        tracks
    } else {
        terminal
            .draw(|f| draw_loading_ui(f, &args.path, scanned.len()))
            .context("draw loading UI")?;
        let tracks = build_tracks_from_scan(&scanned, args.sort, args.reverse);
        save_playlist_cache(&cache_path, &args.path, &scanned, &tracks).ok();
        tracks
    };

    let (_stream, stream_handle) = OutputStream::try_default().context("audio output")?;
    let sink = Sink::try_new(&stream_handle).context("audio sink")?;

    let mut state = PlayerState {
        playlist_path: playlist_path.clone(),
        tracks,
        current: 0,
        selected: 0,
        search_query: String::new(),
        search_mode: false,
        show_help: false,
        show_lyrics: false,
        lyrics_scroll: 0,
        paused: false,
        started_at: None,
        elapsed_before_pause: Duration::ZERO,
        total_duration: None,
        sink,
        stream_handle,
        sort_key: args.sort,
        sort_reverse: args.reverse,
        ui_mode: UiMode::Playlist,
        force_kitty: env_force_kitty(),
        art_view_mode: ArtViewMode::AlbumArt,
    };

    restore_last_played_index(&mut state, &playlist_path);
    load_track(&mut state)?;

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    let supports_kitty = supports_kitty();
    let mut kitty_drawn = false;
    let mut last_art_sig: Option<ArtSig> = None;

    loop {
        let mut draw_info = crate::models::DrawInfo::default();
        terminal
            .draw(|f| {
                draw_info = draw_ui(f, &state, supports_kitty);
            })
            .context("draw UI")?;

        if draw_info.use_kitty {
            if let Some(rect) = draw_info.art_rect {
                if let Some(png) = state.tracks[state.current].art_png.as_deref() {
                    let sig = ArtSig {
                        index: state.current,
                        width: rect.width,
                        height: rect.height,
                    };
                    if last_art_sig != Some(sig) {
                        render_kitty_image(terminal.backend_mut(), rect, png)
                            .context("render kitty image")?;
                        kitty_drawn = true;
                        last_art_sig = Some(sig);
                    }
                }
            }
        } else if kitty_drawn {
            clear_kitty_image(terminal.backend_mut()).ok();
            kitty_drawn = false;
            last_art_sig = None;
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).context("poll event")? {
            if let Event::Key(key) = event::read().context("read event")? {
                if key.kind == KeyEventKind::Press {
                    if state.show_help {
                        match key.code {
                            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => {
                                state.show_help = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if state.show_lyrics {
                        match key.code {
                            KeyCode::Char('l') | KeyCode::Esc | KeyCode::Enter => {
                                state.show_lyrics = false;
                                state.lyrics_scroll = 0;
                            }
                            KeyCode::Up => scroll_lyrics(&mut state, -1),
                            KeyCode::Down => scroll_lyrics(&mut state, 1),
                            KeyCode::PageUp => scroll_lyrics(&mut state, -8),
                            KeyCode::PageDown => scroll_lyrics(&mut state, 8),
                            KeyCode::Home => state.lyrics_scroll = 0,
                            KeyCode::End => {
                                state.lyrics_scroll =
                                    max_lyrics_scroll(&state.tracks[state.current]);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if state.ui_mode == UiMode::Playlist
                        && handle_playlist_search_key(&mut state, key)?
                    {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('?') => state.show_help = true,
                        KeyCode::Char('l') => {
                            state.show_lyrics = true;
                            state.lyrics_scroll = 0;
                        }
                        KeyCode::Char(' ') => toggle_pause(&mut state),
                        KeyCode::Char('/') if state.ui_mode == UiMode::Playlist => {
                            state.search_mode = true;
                        }
                        KeyCode::Up if state.ui_mode == UiMode::Playlist => {
                            move_playlist_selection(&mut state, -1);
                        }
                        KeyCode::Down if state.ui_mode == UiMode::Playlist => {
                            move_playlist_selection(&mut state, 1);
                        }
                        KeyCode::Enter if state.ui_mode == UiMode::Playlist => {
                            play_selected_track(&mut state)?;
                        }
                        KeyCode::Char('n') => next_track(&mut state)?,
                        KeyCode::Char('p') => prev_track(&mut state)?,
                        KeyCode::Char('t') => resort_playlist(&mut state, SortKey::Title),
                        KeyCode::Char('a') => resort_playlist(&mut state, SortKey::Artist),
                        KeyCode::Char('L') => resort_playlist(&mut state, SortKey::Album),
                        KeyCode::Char('s') => resort_playlist(&mut state, SortKey::Path),
                        KeyCode::Char('r') => toggle_reverse(&mut state),
                        KeyCode::Char('R') | KeyCode::F(5) => {
                            refresh_playlist(&mut state, terminal)?;
                            clear_kitty_image(terminal.backend_mut()).ok();
                            kitty_drawn = false;
                            last_art_sig = None;
                        }
                        KeyCode::Char('k') => state.force_kitty = !state.force_kitty,
                        KeyCode::Char('b') if state.ui_mode == UiMode::NowPlaying => {
                            state.art_view_mode = match state.art_view_mode {
                                ArtViewMode::AlbumArt => ArtViewMode::AsciiArt,
                                ArtViewMode::AsciiArt => ArtViewMode::AlbumArt,
                            };
                        }
                        KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.ui_mode = UiMode::Playlist;
                        }
                        KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.ui_mode = UiMode::NowPlaying;
                        }
                        KeyCode::F(1) => state.ui_mode = UiMode::Playlist,
                        KeyCode::F(2) => state.ui_mode = UiMode::NowPlaying,
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }

        if state.sink.empty() && !advance_if_possible(&mut state)? {
            break;
        }
    }

    Ok(())
}
