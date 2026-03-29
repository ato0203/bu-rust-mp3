use anyhow::{Context, Result};
use base64::Engine;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use id3::{Tag, TagLike};
use image::{GenericImageView, ImageFormat};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph},
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::{
    env, fs,
    fs::File,
    io::{self, BufReader},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};
use walkdir::WalkDir;

const ALBUM_ART_SIZE_PX: u32 = 400;
const CACHE_VERSION: u32 = 1;
const CACHE_APP_DIR: &str = "bu-rust-mp3";

struct Track {
    path: PathBuf,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    art_png: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
enum SortKey {
    Path,
    Name,
    Mtime,
    Title,
    Artist,
    Album,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Playlist,
    NowPlaying,
}

struct Args {
    path: PathBuf,
    sort: SortKey,
    reverse: bool,
}

#[derive(Clone)]
struct ScannedTrack {
    path: PathBuf,
    size: u64,
    modified_unix_secs: u64,
}

#[derive(Serialize, Deserialize)]
struct PlaylistCache {
    version: u32,
    playlist: String,
    entries: Vec<CachedTrack>,
}

#[derive(Serialize, Deserialize)]
struct CachedTrack {
    path: String,
    size: u64,
    modified_unix_secs: u64,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    art_png_base64: Option<String>,
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
    sort_key: SortKey,
    sort_reverse: bool,
    ui_mode: UiMode,
    force_kitty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ArtSig {
    index: usize,
    width: u16,
    height: u16,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("terminal")?;

    let result = run_app(args, &mut terminal);

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

fn run_app(args: Args, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
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
        let tracks = build_tracks_from_scan(&args, &scanned);
        save_playlist_cache(&cache_path, &args.path, &scanned, &tracks).ok();
        tracks
    };

    let (_stream, stream_handle) = OutputStream::try_default().context("audio output")?;
    let sink = Sink::try_new(&stream_handle).context("audio sink")?;

    let mut state = PlayerState {
        tracks,
        current: 0,
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
    };

    load_track(&mut state)?;

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    let supports_kitty = supports_kitty();
    let mut kitty_drawn = false;
    let mut last_art_sig: Option<ArtSig> = None;

    loop {
        let mut draw_info = DrawInfo::default();
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
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char(' ') => toggle_pause(&mut state),
                        KeyCode::Char('n') => next_track(&mut state)?,
                        KeyCode::Char('p') => prev_track(&mut state)?,
                        KeyCode::Char('t') => resort_playlist(&mut state, SortKey::Title),
                        KeyCode::Char('a') => resort_playlist(&mut state, SortKey::Artist),
                        KeyCode::Char('l') => resort_playlist(&mut state, SortKey::Album),
                        KeyCode::Char('s') => resort_playlist(&mut state, SortKey::Path),
                        KeyCode::Char('r') => toggle_reverse(&mut state),
                        KeyCode::Char('k') => state.force_kitty = !state.force_kitty,
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

        if state.sink.empty() {
            if !advance_if_possible(&mut state)? {
                break;
            }
        }
    }

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
                let key = args_iter.next().context("Missing value for --sort")?;
                sort = match key.as_str() {
                    "path" => SortKey::Path,
                    "name" => SortKey::Name,
                    "mtime" => SortKey::Mtime,
                    "title" => SortKey::Title,
                    "artist" => SortKey::Artist,
                    "album" => SortKey::Album,
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

    let path = path.context("Usage: bu-rust-mp3 [--sort path|name|mtime|title|artist|album] [--reverse] <file.mp3|directory>")?;
    Ok(Args {
        path,
        sort,
        reverse,
    })
}

fn scan_playlist(args: &Args) -> Result<Vec<ScannedTrack>> {
    if args.path.is_file() {
        return Ok(vec![scan_track(&args.path)?]);
    }

    let mut scanned = Vec::new();
    for entry in WalkDir::new(&args.path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let p = entry.path();
            if p.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("mp3"))
                .unwrap_or(false)
            {
                scanned.push(scan_track(p)?);
            }
        }
    }
    scanned.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(scanned)
}

fn scan_track(path: &Path) -> Result<ScannedTrack> {
    let meta = path
        .metadata()
        .with_context(|| format!("read metadata for {}", path.display()))?;
    let modified_unix_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(ScannedTrack {
        path: path.to_path_buf(),
        size: meta.len(),
        modified_unix_secs,
    })
}

fn build_tracks_from_scan(args: &Args, scanned: &[ScannedTrack]) -> Vec<Track> {
    let mut tracks = scanned
        .iter()
        .map(|entry| {
            let meta = read_meta(&entry.path);
            Track {
                path: entry.path.clone(),
                title: meta.title,
                artist: meta.artist,
                album: meta.album,
                art_png: meta.art_png,
            }
        })
        .collect::<Vec<_>>();
    sort_tracks(&mut tracks, args.sort, args.reverse);
    tracks
}

fn load_cached_tracks(
    cache_path: &Path,
    args: &Args,
    scanned: &[ScannedTrack],
) -> Result<Option<Vec<Track>>> {
    let text = match fs::read_to_string(cache_path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("read cache {}", cache_path.display()));
        }
    };

    let cache: PlaylistCache = match serde_json::from_str(&text) {
        Ok(cache) => cache,
        Err(_) => return Ok(None),
    };
    if cache.version != CACHE_VERSION {
        return Ok(None);
    }
    if cache.entries.len() != scanned.len() {
        return Ok(None);
    }
    for (entry, scanned_track) in cache.entries.iter().zip(scanned) {
        if entry.path != scanned_track.path.to_string_lossy()
            || entry.size != scanned_track.size
            || entry.modified_unix_secs != scanned_track.modified_unix_secs
        {
            return Ok(None);
        }
    }

    let mut tracks = cache
        .entries
        .into_iter()
        .map(|entry| Track {
            path: PathBuf::from(entry.path),
            title: entry.title,
            artist: entry.artist,
            album: entry.album,
            art_png: entry.art_png_base64.and_then(decode_art_png),
        })
        .collect::<Vec<_>>();
    sort_tracks(&mut tracks, args.sort, args.reverse);
    Ok(Some(tracks))
}

fn save_playlist_cache(
    cache_path: &Path,
    playlist_path: &Path,
    scanned: &[ScannedTrack],
    tracks: &[Track],
) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create cache dir {}", parent.display()))?;
    }

    let entries = scanned
        .iter()
        .map(|entry| {
            let track = tracks
                .iter()
                .find(|track| track.path == entry.path)
                .expect("scan and tracks should contain the same files");
            CachedTrack {
                path: entry.path.to_string_lossy().into_owned(),
                size: entry.size,
                modified_unix_secs: entry.modified_unix_secs,
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                art_png_base64: track.art_png.as_deref().map(encode_art_png),
            }
        })
        .collect();

    let cache = PlaylistCache {
        version: CACHE_VERSION,
        playlist: playlist_path.to_string_lossy().into_owned(),
        entries,
    };
    let json = serde_json::to_string(&cache).context("serialize playlist cache")?;
    fs::write(cache_path, json).with_context(|| format!("write cache {}", cache_path.display()))
}

fn playlist_cache_path(path: &Path) -> PathBuf {
    cache_root()
        .join(CACHE_APP_DIR)
        .join(format!("{}.json", encode_cache_key(path)))
}

fn cache_root() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".cache");
    }
    env::temp_dir()
}

fn encode_cache_key(path: &Path) -> String {
    let key = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut out = String::with_capacity(key.len() * 2);
    for byte in key.bytes() {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{:02x}", byte));
    }
    out
}

fn encode_art_png(png: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(png)
}

fn decode_art_png(encoded: String) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
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
        SortKey::Title => tracks.sort_by_key(|t| meta_key(t.title.as_deref(), &t.path)),
        SortKey::Artist => tracks.sort_by_key(|t| meta_key(t.artist.as_deref(), &t.path)),
        SortKey::Album => tracks.sort_by_key(|t| meta_key(t.album.as_deref(), &t.path)),
    }
    if reverse {
        tracks.reverse();
    }
}

struct TrackMeta {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    art_png: Option<Vec<u8>>,
}

fn read_meta(path: &Path) -> TrackMeta {
    let tag = match Tag::read_from_path(path) {
        Ok(tag) => tag,
        Err(_) => {
            return TrackMeta {
                title: None,
                artist: None,
                album: None,
                art_png: None,
            };
        }
    };
    let art_png = extract_cover_png(&tag);
    TrackMeta {
        title: clean_opt(tag.title()),
        artist: clean_opt(tag.artist()),
        album: clean_opt(tag.album()),
        art_png,
    }
}

fn clean_opt(value: Option<&str>) -> Option<String> {
    let v = value?.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn meta_key(value: Option<&str>, path: &Path) -> String {
    let v = value.unwrap_or("").trim();
    if v.is_empty() {
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    } else {
        v.to_ascii_lowercase()
    }
}

fn extract_cover_png(tag: &Tag) -> Option<Vec<u8>> {
    let pic = tag
        .pictures()
        .find(|p| p.picture_type == id3::frame::PictureType::CoverFront)
        .or_else(|| tag.pictures().next())?;

    let img = image::load_from_memory(&pic.data).ok()?;
    let (w, h) = img.dimensions();
    let size = w.min(h);
    let x = (w - size) / 2;
    let y = (h - size) / 2;
    let img = img.crop_imm(x, y, size, size).resize_exact(
        ALBUM_ART_SIZE_PX,
        ALBUM_ART_SIZE_PX,
        image::imageops::FilterType::Lanczos3,
    );
    let mut png = Vec::new();
    if img
        .write_to(&mut io::Cursor::new(&mut png), ImageFormat::Png)
        .is_ok()
    {
        Some(png)
    } else {
        None
    }
}

fn print_usage() {
    println!(
        "Usage: bu-rust-mp3 [--sort path|name|mtime|title|artist|album] [--reverse] <file.mp3|directory>"
    );
    println!("Sort defaults to path. Use --reverse to invert order.");
}

fn draw_loading_ui(f: &mut Frame, playlist_path: &Path, track_count: usize) {
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

#[derive(Default)]
struct DrawInfo {
    art_rect: Option<Rect>,
    use_kitty: bool,
}

fn draw_ui(f: &mut Frame, state: &PlayerState, supports_kitty: bool) -> DrawInfo {
    match state.ui_mode {
        UiMode::Playlist => draw_playlist(f, state),
        UiMode::NowPlaying => draw_now_playing(f, state, supports_kitty),
    }
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
            .label(format!("{}", fmt_duration(elapsed)))
    };
    f.render_widget(gauge, chunks[2]);

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
            ListItem::new(format!("{}{}", prefix, name))
        })
        .collect::<Vec<_>>();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Playlist"));
    f.render_widget(list, chunks[3]);

    let help = Paragraph::new(
        "Controls: space play/pause | n next | p previous | t title | a artist | l album | s path | r reverse | ctrl+1/F1 playlist | ctrl+2/F2 now playing | q quit",
    );
    f.render_widget(help, chunks[4]);
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
    let info = format!(
        "{}\nTitle : {}\nArtist: {}\nAlbum : {}\nArt   : {} | Kitty: {}",
        status, display_title, artist, album, art_status, kitty_status
    );
    let info = Paragraph::new(info).block(Block::default().borders(Borders::ALL));
    f.render_widget(info, chunks[1]);

    let art_block = Block::default().borders(Borders::ALL).title("Album Art");
    let art_inner = art_block.inner(chunks[2]);
    f.render_widget(art_block, chunks[2]);

    let use_kitty = (supports_kitty || state.force_kitty) && track.art_png.is_some();
    let square = art_inner.width.min(art_inner.height);
    let art_rect = center_rect(art_inner, square, square);
    if !use_kitty {
        if let Some(png) = track.art_png.as_deref() {
            if let Some(lines) = ascii_art_lines(png, art_rect.width, art_rect.height) {
                let art = Paragraph::new(lines.join("\n"));
                f.render_widget(art, art_rect);
            } else {
                let art = Paragraph::new("No art").alignment(Alignment::Center);
                f.render_widget(art, art_rect);
            }
        } else {
            let art = Paragraph::new("No art").alignment(Alignment::Center);
            f.render_widget(art, art_rect);
        }
    }

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
            .label(format!("{}", fmt_duration(elapsed)))
    };
    f.render_widget(gauge, chunks[3]);

    let help = Paragraph::new(
        "Controls: space play/pause | n next | p previous | t title | a artist | l album | s path | r reverse | k kitty | ctrl+1/F1 playlist | ctrl+2/F2 now playing | q quit",
    );
    f.render_widget(help, chunks[4]);

    DrawInfo {
        art_rect: Some(art_rect),
        use_kitty,
    }
}

fn resort_playlist(state: &mut PlayerState, sort: SortKey) {
    let current_path = state.tracks[state.current].path.clone();
    state.sort_key = sort;
    sort_tracks(&mut state.tracks, state.sort_key, state.sort_reverse);
    if let Some(idx) = state.tracks.iter().position(|t| t.path == current_path) {
        state.current = idx;
    }
}

fn toggle_reverse(state: &mut PlayerState) {
    let current_path = state.tracks[state.current].path.clone();
    state.sort_reverse = !state.sort_reverse;
    sort_tracks(&mut state.tracks, state.sort_key, state.sort_reverse);
    if let Some(idx) = state.tracks.iter().position(|t| t.path == current_path) {
        state.current = idx;
    }
}

fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Path => "path",
        SortKey::Name => "name",
        SortKey::Mtime => "mtime",
        SortKey::Title => "title",
        SortKey::Artist => "artist",
        SortKey::Album => "album",
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

fn supports_kitty() -> bool {
    if let Ok(v) = env::var("BU_MP3_KITTY") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    if env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }
    if let Ok(term) = env::var("TERM") {
        if term.contains("kitty") || term.contains("ghostty") {
            return true;
        }
    }
    if env::var("GHOSTTY").is_ok() {
        return true;
    }
    if let Ok(tp) = env::var("TERM_PROGRAM") {
        if tp.to_ascii_lowercase().contains("ghostty") {
            return true;
        }
    }
    false
}

fn env_force_kitty() -> bool {
    if let Ok(v) = env::var("BU_MP3_KITTY") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    false
}

fn ascii_art_lines(png: &[u8], width: u16, height: u16) -> Option<Vec<String>> {
    if width == 0 || height == 0 {
        return None;
    }
    let img = image::load_from_memory(png).ok()?;
    let img = img.resize_exact(
        width as u32,
        height as u32,
        image::imageops::FilterType::Nearest,
    );
    let luma = img.to_luma8();
    let ramp = b" .:-=+*#%@";
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height as u32 {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width as u32 {
            let v = luma.get_pixel(x, y).0[0] as usize;
            let idx = v * (ramp.len() - 1) / 255;
            line.push(ramp[idx] as char);
        }
        lines.push(line);
    }
    Some(lines)
}

fn render_kitty_image<W: Write>(out: &mut W, rect: Rect, png: &[u8]) -> io::Result<()> {
    execute!(out, cursor::MoveTo(rect.x, rect.y))?;

    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let mut first = true;
    let mut remaining = encoded.as_str();
    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.len() > 4096 {
            remaining.split_at(4096)
        } else {
            (remaining, "")
        };
        remaining = rest;
        let more = if remaining.is_empty() { 0 } else { 1 };
        if first {
            let seq = format!(
                "\x1b_Gf=100,a=T,c={},r={},i=1,m={};{}\x1b\\",
                rect.width, rect.height, more, chunk
            );
            write_kitty_seq(out, &seq)?;
            first = false;
        } else {
            let seq = format!("\x1b_Gm={};{}\x1b\\", more, chunk);
            write_kitty_seq(out, &seq)?;
        }
    }
    out.flush()
}

fn clear_kitty_image<W: Write>(out: &mut W) -> io::Result<()> {
    write_kitty_seq(out, "\x1b_Ga=d,i=1\x1b\\")?;
    out.flush()
}

fn write_kitty_seq<W: Write>(out: &mut W, seq: &str) -> io::Result<()> {
    if env::var("TMUX").is_ok() {
        let escaped = seq.replace('\x1b', "\x1b\x1b");
        write!(out, "\x1bPtmux;{}\x1b\\", escaped)
    } else {
        write!(out, "{}", seq)
    }
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}
