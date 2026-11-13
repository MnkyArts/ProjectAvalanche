use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_content::texture::{BlockChain, BlockFormat, Shape, Texture};
use ssx_content::{ContentHash, ContentId, Kind};

use super::textures::{DiscTextures, decode_path};

pub const EXPORTS_FILE: &str = "exports.tsv";
const HEADER: &str = "blake3\tsize\tid\tfile\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRow {
    pub hash: ContentHash,
    pub size: u64,
    pub id: String,
    pub file: String,
}

pub fn parse_exports(text: &str) -> Result<Vec<ExportRow>, String> {
    let mut lines = text.split('\n');
    if lines.next() != Some(HEADER.trim_end()) {
        return Err(format!("{EXPORTS_FILE}: not an export index (header {:?})", HEADER.trim_end()));
    }
    let mut rows = Vec::new();
    for (n, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let bad = |what: &str| format!("{EXPORTS_FILE}:{}: {what}", n + 2);
        let f: Vec<&str> = line.split('\t').collect();
        let [hash, size, id, file] = f[..] else { return Err(bad("not four columns")) };
        let size_ok =
            !size.is_empty() && size.bytes().all(|b| b.is_ascii_digit()) && (size == "0" || !size.starts_with('0'));
        if !size_ok {
            return Err(bad("a size that is not a plain number"));
        }
        let hash = hash.parse().map_err(|e| bad(&format!("{e}")))?;
        let size = size.parse().map_err(|_| bad("a size out of range"))?;
        rows.push(ExportRow { hash, size, id: id.to_string(), file: file.to_string() });
    }
    Ok(rows)
}

// PORT(choice): 256 MiB, about 1.3 million exported files at 200 bytes a row (an export command records 1 to a few
// hundred); a larger file is refused with its path, never trimmed; tuning: the export counts of mod authors; the
// original has no mod tools.
const MAX_EXPORTS_BYTES: u64 = 256 << 20;

pub fn record_exports(app_dir: &Path, rows: &[ExportRow]) -> Result<PathBuf> {
    ssx_files::create_dir_all(app_dir).with_context(|| format!("creating {}", app_dir.display()))?;
    let path = app_dir.join(EXPORTS_FILE);
    let mut text = String::new();
    for r in rows {
        if r.file.contains(['\t', '\n', '\r']) || r.id.contains(['\t', '\n', '\r']) {
            bail!("{:?}: a tab or line break in the path", r.file);
        }
        text += &format!("{}\t{}\t{}\t{}\n", r.hash, r.size, r.id, r.file);
    }
    ssx_files::update(&path, &ssx_files::Options::new(), MAX_EXPORTS_BYTES, |old| -> Result<Vec<u8>> {
        let mut index = match old {
            Some(bytes) => {
                String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{}: not UTF-8 text", path.display()))?
            }
            None => HEADER.to_string(),
        };
        parse_exports(&index).map_err(|e| anyhow::anyhow!("{e} ({})", path.display()))?;
        index += &text;
        Ok(index.into_bytes())
    })?;
    Ok(path)
}

fn dds_blocks(chain: &BlockChain) -> Result<Vec<u8>> {
    let fourcc: &[u8; 4] = match chain.format {
        BlockFormat::Bc1 => b"DXT1",
        BlockFormat::Bc2 => b"DXT3",
        BlockFormat::Bc3 => b"DXT5",
        BlockFormat::Bc5 => b"ATI2",
        f => bail!("{} blocks are not exported", f.name()),
    };
    let first = chain.levels.first().context("no levels")?;
    let pf = [32, 0x4, u32::from_le_bytes(*fourcc), 0, 0, 0, 0, 0];
    let mut out = dds_header(first.width, first.height, chain.levels.len() as u32, pf, 0);
    chain.levels.iter().for_each(|l| out.extend_from_slice(&l.data));
    Ok(out)
}

fn dds_header(w: u32, h: u32, mips: u32, pixel_format: [u32; 8], caps2: u32) -> Vec<u8> {
    let mut header = [0u32; 31];
    header[0] = 124;
    header[1] = 0x1 | 0x2 | 0x4 | 0x1000 | 0x2_0000;
    (header[2], header[3], header[6]) = (h, w, mips);
    header[18..26].copy_from_slice(&pixel_format);
    (header[26], header[27]) = (0x1000 | 0x8 | 0x40_0000, caps2);
    let mut out = b"DDS ".to_vec();
    header.iter().for_each(|v| out.extend(v.to_le_bytes()));
    out
}

fn dds_cube(t: &Texture) -> Result<Vec<u8>> {
    let size = t.width();
    let mips = t.levels.iter().take_while(|l| l.images.len() == 6 && l.width == l.height).count();
    if mips == 0 {
        bail!("not a cube with six square faces");
    }
    let pf = [32, 0x4, u32::from_le_bytes(*b"DX10"), 0, 0, 0, 0, 0];
    let mut out = dds_header(size, size, mips as u32, pf, 0x200 | 0xFC00);
    [28u32, 3, 0x4, 1, 0].iter().for_each(|v| out.extend(v.to_le_bytes()));
    for face in 0..6 {
        t.levels[..mips].iter().for_each(|l| out.extend_from_slice(&l.images[face]));
    }
    Ok(out)
}

fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    let mut writer = e.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(out)
}

pub fn export_files(t: &Texture, name: &str, blocks: bool) -> Result<Vec<(String, Vec<u8>)>> {
    let normal = t.blocks.as_ref().is_some_and(|b| b.format == BlockFormat::Bc5);
    match t.shape {
        Shape::Volume => bail!("a volume texture (the post-FX colour cube's data) is not exported"),
        Shape::Cube => Ok(vec![(format!("{name}.dds"), dds_cube(t)?)]),
        Shape::Plane if normal || blocks => {
            let chain =
                t.blocks.as_ref().context("this texture's levels are not stored as GPU blocks: export it as PNG")?;
            let mut files = vec![(format!("{name}.dds"), dds_blocks(chain)?)];
            if normal {
                files.push(("textures.toml".into(), format!("[\"{name}.dds\"]\nkind = \"normal\"\n").into_bytes()));
            }
            Ok(files)
        }
        Shape::Plane => {
            let l = t.levels.first().context("no levels")?;
            Ok(vec![(format!("{name}.png"), png_rgba(l.width, l.height, &l.images[0])?)])
        }
    }
}

pub fn export_texture(
    big_root: &Path,
    id: &ContentId,
    out_dir: &Path,
    app_dir: &Path,
    blocks: bool,
) -> Result<Vec<PathBuf>> {
    if id.kind() != Kind::Texture {
        bail!("{id}: only textures are exported in this version");
    }
    let disc = DiscTextures::index(big_root)?;
    let file = disc
        .file(id)
        .with_context(|| format!("{id}: no such texture on the disc (ids: `content ids --kind texture`)"))?;
    let texture = decode_path(file)?;
    let name = id.path().rsplit('/').next().unwrap_or(id.path());
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let mut written = Vec::new();
    let mut rows = Vec::new();
    for (file_name, bytes) in export_files(&texture, name, blocks)? {
        let path = out_dir.join(&file_name);
        if file_name == "textures.toml" && path.exists() {
            let old = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let settings = ssx_content::texture::FolderSettings::parse(&old, &path.display().to_string())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if settings.files().all(|(f, _)| f != format!("{name}.dds")) {
                let mut text = old;
                text.extend_from_slice(format!("\n{}", String::from_utf8_lossy(&bytes)).as_bytes());
                ssx_files::replace(&path, &text, &ssx_files::Options::new())?;
            }
            written.push(path);
            continue;
        }
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        if file_name != "textures.toml" {
            rows.push(ExportRow {
                hash: ContentHash::of(&bytes),
                size: bytes.len() as u64,
                id: id.to_string(),
                file: path.display().to_string(),
            });
        }
        written.push(path);
    }
    record_exports(app_dir, &rows)?;
    Ok(written)
}

#[cfg(test)]
mod tests;
