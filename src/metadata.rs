use id3::{Tag, TagLike};
use image::ImageFormat;
use std::{io, path::Path, time::UNIX_EPOCH};

use crate::models::{ScannedTrack, SortKey, Track, TrackMeta};

pub fn build_tracks_from_scan(
    scanned: &[ScannedTrack],
    sort: SortKey,
    reverse: bool,
) -> Vec<Track> {
    let mut tracks = scanned
        .iter()
        .map(|entry| {
            let meta = read_meta(&entry.path);
            Track {
                path: entry.path.clone(),
                title: meta.title,
                artist: meta.artist,
                album: meta.album,
                lyrics: meta.lyrics,
                art_png: meta.art_png,
            }
        })
        .collect::<Vec<_>>();
    sort_tracks(&mut tracks, sort, reverse);
    tracks
}

pub fn sort_tracks(tracks: &mut [Track], sort: SortKey, reverse: bool) {
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

pub fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Path => "path",
        SortKey::Name => "name",
        SortKey::Mtime => "mtime",
        SortKey::Title => "title",
        SortKey::Artist => "artist",
        SortKey::Album => "album",
    }
}

fn read_meta(path: &Path) -> TrackMeta {
    let tag = match Tag::read_from_path(path) {
        Ok(tag) => tag,
        Err(_) => {
            return TrackMeta {
                title: None,
                artist: None,
                album: None,
                lyrics: None,
                art_png: None,
            };
        }
    };
    let art_png = extract_cover_png(&tag);
    TrackMeta {
        title: clean_opt(tag.title()),
        artist: clean_opt(tag.artist()),
        album: clean_opt(tag.album()),
        lyrics: extract_lyrics(&tag),
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

fn extract_lyrics(tag: &Tag) -> Option<String> {
    tag.lyrics()
        .find_map(|lyrics| clean_opt(Some(lyrics.text.as_str())))
}
