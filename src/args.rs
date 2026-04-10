use anyhow::{Context, Result};
use std::{env, path::PathBuf};

use crate::models::{Args, SortKey};

pub fn parse_args() -> Result<Args> {
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

    let path = path.context(
        "Usage: bu-rust-mp3 [--sort path|name|mtime|title|artist|album] [--reverse] <file.mp3|directory>",
    )?;
    Ok(Args {
        path,
        sort,
        reverse,
    })
}

pub fn print_usage() {
    println!(
        "Usage: bu-rust-mp3 [--sort path|name|mtime|title|artist|album] [--reverse] <file.mp3|directory>"
    );
    println!("Sort defaults to path. Use --reverse to invert order.");
}
