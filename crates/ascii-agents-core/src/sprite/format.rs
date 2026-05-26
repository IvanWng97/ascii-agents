use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::sprite::{Frame, Palette, Pixel, Rgb, Sprite};

/// Parse a `.sprite` text file. Returns one Frame per `@frame N` block.
pub fn parse_sprite_file(src: &str, palette: &Palette) -> Result<Vec<Frame>> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut current: Option<Vec<Vec<Pixel>>> = None;

    for (lineno, raw) in src.lines().enumerate() {
        let line = strip_comment_and_trim(raw);
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("@frame") {
            if let Some(rows) = current.take() {
                frames.push(rows_to_frame(rows).map_err(|e| anyhow!("{e} (line {})", lineno + 1))?);
            }
            let _ = rest
                .trim()
                .parse::<u32>()
                .map_err(|_| anyhow!("@frame requires a number (line {})", lineno + 1))?;
            current = Some(Vec::new());
            continue;
        }

        let rows = current
            .as_mut()
            .ok_or_else(|| anyhow!("pixel data before any @frame (line {})", lineno + 1))?;

        let row = parse_row(line, palette).map_err(|e| anyhow!("{e} (line {})", lineno + 1))?;
        rows.push(row);
    }

    if let Some(rows) = current.take() {
        frames.push(rows_to_frame(rows)?);
    }

    if frames.is_empty() {
        bail!("sprite file contains no frames");
    }
    Ok(frames)
}

fn strip_comment_and_trim(line: &str) -> &str {
    let line = match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    };
    line.trim()
}

fn parse_row(line: &str, palette: &Palette) -> Result<Vec<Pixel>> {
    let mut out = Vec::new();
    for tok in line.split_whitespace() {
        let mut chars = tok.chars();
        let key = chars.next().ok_or_else(|| anyhow!("empty token"))?;
        if chars.next().is_some() {
            bail!("each pixel must be a single character (got {tok:?})");
        }
        let px = palette
            .get(key)
            .ok_or_else(|| anyhow!("unknown palette key '{key}'"))?;
        out.push(px);
    }
    Ok(out)
}

fn rows_to_frame(rows: Vec<Vec<Pixel>>) -> Result<Frame> {
    if rows.is_empty() {
        bail!("frame has no rows");
    }
    let w = rows[0].len();
    for (i, r) in rows.iter().enumerate() {
        if r.len() != w {
            bail!(
                "inconsistent row width at row {i} (expected {w}, got {})",
                r.len()
            );
        }
    }
    let height = rows.len() as u16;
    let width = w as u16;
    let pixels = rows.into_iter().flatten().collect();
    Ok(Frame {
        width,
        height,
        pixels,
    })
}

#[derive(Debug, Deserialize)]
struct PackToml {
    pack: PackMeta,
    palette: HashMap<String, String>,
    animations: HashMap<String, AnimationToml>,
}

#[derive(Debug, Deserialize)]
struct PackMeta {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct AnimationToml {
    frames: Vec<String>,
    frame_ms: u32,
}

#[derive(Debug, Clone)]
pub struct Pack {
    pub name: String,
    pub version: String,
    pub palette: Palette,
    animations: HashMap<String, Sprite>,
}

impl Pack {
    pub fn animation(&self, key: &str) -> Option<&Sprite> {
        self.animations.get(key)
    }
}

pub fn load_pack(dir: &Path) -> Result<Pack> {
    let toml_path = dir.join("pack.toml");
    let toml_src = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let parsed: PackToml =
        toml::from_str(&toml_src).with_context(|| format!("parsing {}", toml_path.display()))?;

    let palette = build_palette(&parsed.palette)?;

    let mut animations = HashMap::new();
    for (anim_name, anim) in parsed.animations {
        let mut frames = Vec::new();
        for fname in &anim.frames {
            let path = dir.join(fname);
            let src = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let mut decoded = parse_sprite_file(&src, &palette)
                .with_context(|| format!("decoding {}", path.display()))?;
            frames.append(&mut decoded);
        }
        animations.insert(
            anim_name,
            Sprite {
                frames,
                frame_ms: anim.frame_ms,
            },
        );
    }

    Ok(Pack {
        name: parsed.pack.name,
        version: parsed.pack.version,
        palette,
        animations,
    })
}

/// Same as `load_pack` but takes in-memory strings — used by binaries that
/// `include_str!` their assets at compile time.
pub fn load_pack_from_strings(pack_toml: &str, frames: &[(&str, &str)]) -> Result<Pack> {
    let parsed: PackToml = toml::from_str(pack_toml).context("parsing pack.toml")?;
    let palette = build_palette(&parsed.palette)?;

    let frame_lookup: HashMap<&str, &str> = frames.iter().copied().collect();
    let mut animations = HashMap::new();
    for (anim_name, anim) in parsed.animations {
        let mut frames_vec = Vec::new();
        for fname in &anim.frames {
            let src = frame_lookup
                .get(fname.as_str())
                .ok_or_else(|| anyhow!("missing embedded frame {fname}"))?;
            let mut decoded =
                parse_sprite_file(src, &palette).with_context(|| format!("decoding {fname}"))?;
            frames_vec.append(&mut decoded);
        }
        animations.insert(
            anim_name,
            Sprite {
                frames: frames_vec,
                frame_ms: anim.frame_ms,
            },
        );
    }

    Ok(Pack {
        name: parsed.pack.name,
        version: parsed.pack.version,
        palette,
        animations,
    })
}

fn build_palette(map: &HashMap<String, String>) -> Result<Palette> {
    let mut palette = Palette::new();
    for (k, v) in map {
        if k.chars().count() != 1 {
            bail!("palette key {k:?} must be exactly one character");
        }
        let key = k
            .chars()
            .next()
            .expect("palette key validated as single char");
        let pixel = parse_palette_value(v).with_context(|| format!("palette key '{k}'"))?;
        palette.insert(key, pixel);
    }
    Ok(palette)
}

fn parse_palette_value(v: &str) -> Result<Pixel> {
    if v.eq_ignore_ascii_case("transparent") {
        return Ok(None);
    }
    let hex = v
        .strip_prefix('#')
        .ok_or_else(|| anyhow!("color must start with '#' or be 'transparent', got {v:?}"))?;
    if hex.len() != 6 {
        bail!("color {v:?} must be 6 hex digits");
    }
    let r = u8::from_str_radix(&hex[0..2], 16)?;
    let g = u8::from_str_radix(&hex[2..4], 16)?;
    let b = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Some(Rgb(r, g, b)))
}
