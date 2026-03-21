use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    env,
    fs::File,
    io::{self, BufReader},
    path::PathBuf,
    time::{Duration, Instant, UNIX_EPOCH},
};
use walkdir::WalkDir;

struct Track {
    path: PathBuf,
}

#[derive(Clone, Copy)]
enum SortKey {
    Path,
    Name,
    Mtime,
}

struct Args {
    path: PathBuf,
    sort: SortKey,
    reverse: bool,
}

struct PlayerState {
    tracks: Vec<Track>,
    current: usize,
    paused: bool,
    started_at: Option<Instant>,
    elapsed_before_pause: Duration,
    total_duration: Option<Duration>,
    sink: Sink,
    stream_handle: OutputStreamHandle,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let tracks = collect_tracks(&args)?;
    if tracks.is_empty() {
        anyhow::bail!("No .mp3 files found in {}", args.path.display());
    }

    let (_stream, stream_handle) = OutputStream::try_default().context("audio output")?;
    let sink = Sink::try_new(&stream_handle).context("audio sink")?;

    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("terminal")?;

    let mut state = PlayerState {
        tracks,
        current: 0,
        paused: false,
        started_at: None,
        elapsed_before_pause: Duration::ZERO,
        total_duration: None,
        sink,
        stream_handle,
    };

    load_track(&mut state)?;

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        terminal
            .draw(|f| draw_ui(f, &state))
            .context("draw UI")?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).context("poll event")? {
            if let Event::Key(key) = event::read().context("read event")? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char(' ') => toggle_pause(&mut state),
                        KeyCode::Char('n') => next_track(&mut state)?,
                        KeyCode::Char('p') => prev_track(&mut state)?,
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }

        if state.sink.empty() {
            if !advance_if_possible(&mut state)? {
                break;
            }
        }
    }

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args_iter = env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    let mut sort = SortKey::Path;
    let mut reverse = false;

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--reverse" => reverse = true,
            "--sort" => {
                let key = args_iter
                    .next()
                    .context("Missing value for --sort")?;
                sort = match key.as_str() {
                    "path" => SortKey::Path,
                    "name" => SortKey::Name,
                    "mtime" => SortKey::Mtime,
                    _ => anyhow::bail!("Unknown sort key '{}'", key),
                };
            }
            _ => {
                if path.is_some() {
                    anyhow::bail!("Only one path is allowed");
                }
                path = Some(PathBuf::from(arg));
            }
        }
    }

    let path = path.context("Usage: bu-rust-mp3 [--sort path|name|mtime] [--reverse] <file.mp3|directory>")?;
    Ok(Args { path, sort, reverse })
}

fn collect_tracks(args: &Args) -> Result<Vec<Track>> {
    if args.path.is_file() {
        return Ok(vec![Track {
            path: args.path.to_path_buf(),
        }]);
    }

    let mut tracks = Vec::new();
    for entry in WalkDir::new(&args.path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let p = entry.path();
            if p.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("mp3"))
                .unwrap_or(false)
            {
                tracks.push(Track { path: p.into() });
            }
        }
    }
    sort_tracks(&mut tracks, args.sort, args.reverse);
    Ok(tracks)
}

fn sort_tracks(tracks: &mut [Track], sort: SortKey, reverse: bool) {
    match sort {
        SortKey::Path => tracks.sort_by_key(|t| t.path.clone()),
        SortKey::Name => tracks.sort_by_key(|t| {
            t.path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
        }),
        SortKey::Mtime => tracks.sort_by_key(|t| {
            t.path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH)
        }),
    }
    if reverse {
        tracks.reverse();
    }
}

fn print_usage() {
    println!("Usage: bu-rust-mp3 [--sort path|name|mtime] [--reverse] <file.mp3|directory>");
    println!("Sort defaults to path. Use --reverse to invert order.");
}

fn load_track(state: &mut PlayerState) -> Result<()> {
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
    Ok(())
}

fn toggle_pause(state: &mut PlayerState) {
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

fn next_track(state: &mut PlayerState) -> Result<()> {
    if state.current + 1 < state.tracks.len() {
        state.current += 1;
        load_track(state)?;
    }
    Ok(())
}

fn prev_track(state: &mut PlayerState) -> Result<()> {
    if state.current > 0 {
        state.current -= 1;
        load_track(state)?;
    }
    Ok(())
}

fn advance_if_possible(state: &mut PlayerState) -> Result<bool> {
    if state.current + 1 < state.tracks.len() {
        state.current += 1;
        load_track(state)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn current_elapsed(state: &PlayerState) -> Duration {
    if state.paused {
        state.elapsed_before_pause
    } else if let Some(started) = state.started_at {
        state.elapsed_before_pause + started.elapsed()
    } else {
        state.elapsed_before_pause
    }
}

fn draw_ui(f: &mut Frame, state: &PlayerState) {
    let size = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
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
        "bu-rust-mp3  |  {} / {}",
        state.current + 1,
        state.tracks.len()
    );
    let header = Paragraph::new(title).block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    let track = &state.tracks[state.current];
    let status = if state.paused { "Paused" } else { "Playing" };
    let now_playing = format!(
        "{}: {}",
        status,
        track.path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
    );
    let now = Paragraph::new(now_playing).block(Block::default().borders(Borders::ALL));
    f.render_widget(now, chunks[1]);

    let elapsed = current_elapsed(state);
    let gauge = if let Some(total) = state.total_duration {
        let ratio = (elapsed.as_secs_f64() / total.as_secs_f64()).min(1.0);
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Progress"))
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(ratio)
            .label(format!(
                "{} / {}",
                fmt_duration(elapsed),
                fmt_duration(total)
            ))
    } else {
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Progress"))
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(0.0)
            .label(format!("{}", fmt_duration(elapsed)))
    };
    f.render_widget(gauge, chunks[2]);

    let items = state
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let name = t.path.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            let prefix = if i == state.current { "▶ " } else { "  " };
            ListItem::new(format!("{}{}", prefix, name))
        })
        .collect::<Vec<_>>();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Playlist"));
    f.render_widget(list, chunks[3]);

    let help = Paragraph::new("Controls: space play/pause | n next | p previous | q quit");
    f.render_widget(help, chunks[4]);
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}
