use anyhow::{Context, Result};
use std::{path::Path, time::UNIX_EPOCH};
use walkdir::WalkDir;

use crate::models::{Args, ScannedTrack};

pub fn scan_playlist(args: &Args) -> Result<Vec<ScannedTrack>> {
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
