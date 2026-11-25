//! Classic skin support: `.wsz`/`.zip` archives or unpacked
//! directories, with per-file fallback to Linamp's own default skin.

pub mod bitmap;
pub mod default_skin;
pub mod sprites;
pub mod textcfg;

pub use bitmap::Bitmap;
pub use sprites::{Rect, Sheet};
pub use textcfg::{PlColors, Regions};

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

pub struct Skin {
    pub name: String,
    sheets: HashMap<Sheet, Bitmap>,
    /// The skin provides `nums_ex.bmp` (which has its own minus sign).
    pub has_nums_ex: bool,
    pub viscolor: [u32; 24],
    pub pl: PlColors,
    pub regions: Regions,
}

impl Skin {
    pub fn builtin() -> Skin {
        let sheets = Sheet::ALL.iter().map(|&s| (s, default_skin::sheet(s))).collect();
        Skin {
            name: "Linamp Default".into(),
            sheets,
            has_nums_ex: false,
            viscolor: default_skin::viscolors(),
            pl: default_skin::pl_colors(),
            regions: Regions::default(),
        }
    }

    pub fn sheet(&self, s: Sheet) -> &Bitmap {
        &self.sheets[&s]
    }

    /// Loads a skin archive or directory.
    pub fn load(path: &Path) -> Result<Skin, String> {
        let files = if path.is_dir() { read_dir_files(path)? } else { read_zip_files(path)? };
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Self::from_files(name, &files))
    }

    /// Builds a skin from lower-cased basename → bytes.
    pub fn from_files(name: String, files: &HashMap<String, Vec<u8>>) -> Skin {
        Self::decode_files(name, files).0
    }

    fn decode_files(name: String, files: &HashMap<String, Vec<u8>>) -> (Skin, bool) {
        let mut skin = Skin::builtin();
        skin.name = name;
        let decode = |file: &str| -> Option<Bitmap> {
            let bytes = files.get(file)?;
            match Bitmap::decode(bytes) {
                Ok(b) if b.w > 0 && b.h > 0 => Some(b),
                Ok(_) => None,
                Err(e) => {
                    log::warn!("skin {}: cannot decode {file}: {e}", skin.name);
                    None
                }
            }
        };
        let mut found = 0;
        let mut loaded_balance = false;
        let mut loaded_volume = false;
        for sheet in Sheet::ALL {
            if let Some(b) = decode(sheet.file_name()) {
                skin.sheets.insert(sheet, b);
                found += 1;
                loaded_balance |= sheet == Sheet::Balance;
                loaded_volume |= sheet == Sheet::Volume;
                if sheet == Sheet::NumsEx {
                    skin.has_nums_ex = true;
                }
            }
        }
        // Old skins without balance.bmp reuse their volume bitmap.
        if !loaded_balance && loaded_volume {
            skin.sheets.insert(Sheet::Balance, skin.sheet(Sheet::Volume).clone());
        }
        if let Some(t) = files.get("viscolor.txt") {
            skin.viscolor = textcfg::parse_viscolor(&linamp_core::text::decode_lossy(t), &skin.viscolor);
        }
        if let Some(t) = files.get("pledit.txt") {
            // Skins that only ship pledit.txt start from the classic colours.
            skin.pl = textcfg::parse_pledit(&linamp_core::text::decode_lossy(t), &PlColors::default());
        } else if found > 0 {
            skin.pl = PlColors::default();
        }
        if let Some(t) = files.get("region.txt") {
            skin.regions = textcfg::parse_region(&linamp_core::text::decode_lossy(t));
        }
        if found == 0 {
            log::warn!("skin {}: no recognised bitmaps, using defaults", skin.name);
        }
        let recognized = found > 0 || ["viscolor.txt", "pledit.txt", "region.txt"].iter().any(|name| files.contains_key(*name));
        (skin, recognized)
    }

    /// Validate the exact staged bytes before replacing an existing installed skin.
    pub fn install_archive(source: &Path, dest: &Path) -> Result<Self, String> {
        let dir = dest.parent().ok_or("missing skin destination directory")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut staged = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        let src = std::fs::File::open(source).map_err(|e| e.to_string())?;
        if !src.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("expected a regular skin archive".into());
        }
        let written = std::io::copy(&mut src.take(MAX_ARCHIVE + 1), &mut staged).map_err(|e| e.to_string())?;
        if written > MAX_ARCHIVE {
            return Err("skin archive exceeds 64 MiB".into());
        }
        let files = read_zip_files(staged.path())?;
        let name = dest.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let (skin, recognized) = Self::decode_files(name, &files);
        if !recognized {
            return Err("no usable skin assets".into());
        }
        staged.persist(dest).map_err(|e| e.to_string())?;
        Ok(skin)
    }
}

const MAX_FILE: u64 = 16 << 20;
const MAX_ARCHIVE: u64 = 64 << 20;
const MAX_ASSETS: u64 = 32 << 20;
const MAX_ENTRIES: usize = 4096;

fn wanted(name: &str) -> bool {
    matches!(name, "viscolor.txt" | "pledit.txt" | "region.txt") || Sheet::ALL.iter().any(|s| s.file_name() == name)
}

fn read_asset(reader: impl Read, remaining: &mut u64) -> Result<Vec<u8>, String> {
    let limit = MAX_FILE.min(*remaining);
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("skin exceeds asset size limit".into());
    }
    *remaining -= bytes.len() as u64;
    Ok(bytes)
}

fn read_zip_files(path: &Path) -> Result<HashMap<String, Vec<u8>>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_ARCHIVE {
        return Err("skin archive exceeds 64 MiB".into());
    }
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{}: not a skin archive: {e}", path.display()))?;
    let mut out = HashMap::new();
    let mut remaining = MAX_ASSETS;
    if zip.len() > MAX_ENTRIES {
        return Err("too many skin entries".into());
    }
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else { continue };
        if entry.is_dir() || entry.size() > MAX_FILE {
            continue;
        }
        let base = entry.name().rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
        if !wanted(&base) || out.contains_key(&base) {
            continue;
        }
        out.insert(base, read_asset(entry, &mut remaining)?);
    }
    Ok(out)
}

fn read_dir_files(dir: &Path) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut out = HashMap::new();
    let mut remaining = MAX_ASSETS;
    let mut visited = 0;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for entry in entries.flatten() {
            let p = entry.path();
            visited += 1;
            if visited > MAX_ENTRIES {
                return Err("too many skin entries".into());
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < 2 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let base = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if kind.is_file() && wanted(&base) && !out.contains_key(&base) {
                let file = std::fs::File::open(p).map_err(|e| e.to_string())?;
                out.insert(base, read_asset(file, &mut remaining)?);
            }
        }
    }
    Ok(out)
}

/// Lists installable skins in `dir` (archives and sub-directories).
pub fn list_skins(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                || p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("wsz") || e.eq_ignore_ascii_case("zip"))
        })
        .collect();
    v.sort_by(|a, b| linamp_core::text::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder as _;

    #[test]
    fn archive_install_preserves_previous_skin_on_error() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wsz");
        let installed = dir.path().join("skins/theme.wsz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
        zip.start_file("theme/pledit.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"[Text]\nNormal=#123456").unwrap();
        zip.finish().unwrap();
        let skin = Skin::install_archive(&source, &installed).unwrap();
        assert_eq!(skin.pl.normal, 0x123456);
        // Some file pickers return the already installed file.
        Skin::install_archive(&installed, &installed).unwrap();
        let before = std::fs::read(&installed).unwrap();
        std::fs::write(&source, b"incomplete").unwrap();
        assert!(Skin::install_archive(&source, &installed).is_err());
        assert_eq!(std::fs::read(installed).unwrap(), before);
    }

    #[test]
    fn assets_share_a_byte_budget_and_image_dimensions_are_checked() {
        let mut budget = 5;
        assert_eq!(read_asset(&b"abc"[..], &mut budget).unwrap(), b"abc");
        assert!(read_asset(&b"def"[..], &mut budget).is_err());
        assert!(!wanted("documentation.txt"));
        let mut img = Vec::new();
        image::codecs::png::PngEncoder::new(&mut img).write_image(&[0; 1025], 1025, 1, image::ExtendedColorType::L8).unwrap();
        assert!(Bitmap::decode(&img).is_err());
    }

    #[test]
    fn builtin_sheets_have_reference_sizes() {
        let skin = Skin::builtin();
        for s in Sheet::ALL {
            assert_eq!((skin.sheet(s).w, skin.sheet(s).h), s.size(), "{s:?}");
        }
    }

    #[test]
    fn loads_from_files_with_fallbacks() {
        let mut files = HashMap::new();
        let mut img = Vec::new();
        image::codecs::png::PngEncoder::new(&mut img).write_image(&[255u8; 68 * 433 * 3], 68, 433, image::ExtendedColorType::Rgb8).unwrap();
        files.insert("volume.bmp".to_string(), img); // PNG data despite the name
        files.insert("main.bmp".to_string(), b"not an image".to_vec());
        files.insert("pledit.txt".to_string(), b"[Text]\nNormal=#112233\n".to_vec());
        let skin = Skin::from_files("t".into(), &files);
        assert_eq!(skin.sheet(Sheet::Volume).px[0], 0xffffff);
        assert_eq!(skin.sheet(Sheet::Balance).px[0], 0xffffff, "balance falls back to volume");
        assert_eq!(skin.sheet(Sheet::Main).w, 275, "bad main.bmp falls back to default");
        assert_eq!(skin.pl.normal, 0x112233);
    }
}
