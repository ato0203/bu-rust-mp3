use base64::Engine;
use crossterm::{cursor, execute};
use ratatui::prelude::Rect;
use std::{
    env,
    io::{self, Write},
};

pub fn supports_kitty() -> bool {
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

pub fn env_force_kitty() -> bool {
    if let Ok(v) = env::var("BU_MP3_KITTY") {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    false
}

pub fn ascii_art_lines(png: &[u8], width: u16, height: u16) -> Option<Vec<String>> {
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

pub fn render_kitty_image<W: Write>(out: &mut W, rect: Rect, png: &[u8]) -> io::Result<()> {
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

pub fn clear_kitty_image<W: Write>(out: &mut W) -> io::Result<()> {
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
