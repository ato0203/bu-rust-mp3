use anyhow::{Context, Result};
use prost::Message;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use crate::metadata::sort_tracks;
use crate::models::{Args, PlayerState, ScannedTrack, Track};

const CACHE_VERSION: u32 = 3;
const CACHE_APP_DIR: &str = "bu-rust-mp3";
const RESUME_STATE_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Message)]
struct PlaylistCache {
    #[prost(uint32, tag = "1")]
    version: u32,
    #[prost(string, tag = "2")]
    playlist: String,
    #[prost(message, repeated, tag = "3")]
    entries: Vec<CachedTrack>,
}

#[derive(Clone, PartialEq, Message)]
struct CachedTrack {
    #[prost(string, tag = "1")]
    path: String,
    #[prost(uint64, tag = "2")]
    size: u64,
    #[prost(uint64, tag = "3")]
    modified_unix_secs: u64,
    #[prost(string, optional, tag = "4")]
    title: Option<String>,
    #[prost(string, optional, tag = "5")]
    artist: Option<String>,
    #[prost(string, optional, tag = "6")]
    album: Option<String>,
    #[prost(string, optional, tag = "7")]
    lyrics: Option<String>,
    #[prost(bytes, optional, tag = "8")]
    art_png: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct ResumeState {
    #[prost(uint32, tag = "1")]
    version: u32,
    #[prost(string, tag = "2")]
    playlist: String,
    #[prost(string, tag = "3")]
    last_track_path: String,
}

pub fn load_cached_tracks(
    cache_path: &Path,
    args: &Args,
    scanned: &[ScannedTrack],
) -> Result<Option<Vec<Track>>> {
    let bytes = match fs::read(cache_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("read cache {}", cache_path.display()));
        }
    };

    let cache = match PlaylistCache::decode(bytes.as_slice()) {
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
            lyrics: entry.lyrics,
            art_png: entry.art_png,
        })
        .collect::<Vec<_>>();
    sort_tracks(&mut tracks, args.sort, args.reverse);
    Ok(Some(tracks))
}

pub fn save_playlist_cache(
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
                lyrics: track.lyrics.clone(),
                art_png: track.art_png.clone(),
            }
        })
        .collect();

    let cache = PlaylistCache {
        version: CACHE_VERSION,
        playlist: playlist_path.to_string_lossy().into_owned(),
        entries,
    };
    let bytes = cache.encode_to_vec();
    fs::write(cache_path, bytes).with_context(|| format!("write cache {}", cache_path.display()))
}

pub fn playlist_cache_path(path: &Path) -> PathBuf {
    cache_root()
        .join(CACHE_APP_DIR)
        .join(format!("{}.pb", encode_cache_key(path)))
}

fn resume_state_path(path: &Path) -> PathBuf {
    cache_root()
        .join(CACHE_APP_DIR)
        .join(format!("{}.resume.pb", encode_cache_key(path)))
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

pub fn restore_last_played_index(state: &mut PlayerState, playlist_path: &Path) {
    let path = resume_state_path(playlist_path);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let resume: ResumeState = match ResumeState::decode(bytes.as_slice()) {
        Ok(resume) => resume,
        Err(_) => return,
    };
    if resume.version != RESUME_STATE_VERSION {
        return;
    }
    let last_track_path = Path::new(&resume.last_track_path);
    if let Some(index) = state
        .tracks
        .iter()
        .position(|track| track.path.as_path() == last_track_path)
    {
        state.current = index;
        state.selected = index;
    }
}

pub fn save_resume_state(state: &PlayerState) -> Result<()> {
    let path = resume_state_path(&state.playlist_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create cache dir {}", parent.display()))?;
    }
    let resume = ResumeState {
        version: RESUME_STATE_VERSION,
        playlist: state.playlist_path.to_string_lossy().into_owned(),
        last_track_path: state.tracks[state.current]
            .path
            .to_string_lossy()
            .into_owned(),
    };
    let bytes = resume.encode_to_vec();
    fs::write(&path, bytes).with_context(|| format!("write resume state {}", path.display()))
}
