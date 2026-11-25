//! Playlist files: M3U / M3U8, PLS and XSPF.

use crate::playlist::{is_url, Entry};
use crate::text::{decode_lossy, path_to_file_url, percent_decode, xml_escape, xml_unescape};
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistFormat {
    M3u,
    M3u8,
    Pls,
    Xspf,
}

impl PlaylistFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "m3u" => Self::M3u,
            "m3u8" => Self::M3u8,
            "pls" => Self::Pls,
            "xspf" => Self::Xspf,
            _ => return None,
        })
    }
}

pub fn is_playlist(path: &str) -> bool {
    PlaylistFormat::from_path(Path::new(path.split(['?', '#']).next().unwrap_or(path))).is_some()
}

/// One item read from a playlist file.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub location: String,
    pub title: Option<String>,
    pub length_ms: Option<u64>,
}

impl Item {
    pub fn into_entry(self) -> Entry {
        let mut e = Entry::new(self.location);
        e.file_title = self.title.filter(|t| !t.is_empty());
        e.length_ms = self.length_ms;
        if let Some(t) = &e.file_title {
            e.display = t.clone();
        }
        e
    }
}

/// Resolves a playlist reference relative to the playlist's directory.
fn resolve(base: Option<&Path>, raw: &str) -> String {
    let raw = raw.trim();
    if is_url(raw) {
        return raw.to_string();
    }
    if let Some(rest) = raw.strip_prefix("file://") {
        // file:///abs/path or file://localhost/abs/path
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        return percent_decode(rest);
    }
    let path = raw.replace('\\', "/");
    let p = Path::new(&path);
    if p.is_absolute() {
        return path;
    }
    match base {
        Some(dir) => normalize(&dir.join(p)).to_string_lossy().into_owned(),
        None => path,
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub fn parse(text: &str, format: PlaylistFormat, base: Option<&Path>) -> Vec<Item> {
    parse_locations(text, format, |raw| Some(resolve(base, raw)))
}

/// A reference from an HTTP playlist must remain a network location.
pub fn parse_remote(text: &str, format: PlaylistFormat, base: &str) -> Vec<Item> {
    let Ok(base) = Url::parse(base) else { return Vec::new() };
    parse_locations(text, format, |raw| {
        let url = base.join(raw.trim()).ok()?;
        matches!(url.scheme(), "http" | "https").then(|| url.into())
    })
}

fn parse_locations(text: &str, format: PlaylistFormat, resolve_location: impl Fn(&str) -> Option<String>) -> Vec<Item> {
    match format {
        PlaylistFormat::M3u | PlaylistFormat::M3u8 => parse_m3u(text, resolve_location),
        PlaylistFormat::Pls => parse_pls(text, resolve_location),
        PlaylistFormat::Xspf => parse_xspf(text, resolve_location),
    }
}

/// Guesses the format from content (for URLs without a useful extension).
pub fn sniff(text: &str) -> Option<PlaylistFormat> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("#EXTM3U") {
        Some(PlaylistFormat::M3u8)
    } else if t.to_ascii_lowercase().starts_with("[playlist]") {
        Some(PlaylistFormat::Pls)
    } else if t.starts_with("<?xml") && t.contains("<playlist") {
        Some(PlaylistFormat::Xspf)
    } else {
        None
    }
}

pub fn read_file(path: &Path) -> io::Result<Vec<Item>> {
    const MAX_BYTES: u64 = 16 << 20;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "playlist exceeds 16 MiB"));
    }
    let text = decode_lossy(&bytes);
    let format = PlaylistFormat::from_path(path).or_else(|| sniff(&text)).unwrap_or(PlaylistFormat::M3u);
    Ok(parse(&text, format, path.parent()))
}

fn parse_m3u(text: &str, resolve_location: impl Fn(&str) -> Option<String>) -> Vec<Item> {
    let mut items = Vec::new();
    let mut pending: (Option<String>, Option<u64>) = (None, None);
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            let (len, title) = info.split_once(',').unwrap_or((info, ""));
            // The duration may be followed by attributes: `#EXTINF:-1 tvg-id="x",Title`.
            let secs: i64 = len.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).map(|f| f as i64).unwrap_or(-1);
            pending = (Some(title.trim().to_string()), u64::try_from(secs).ok().and_then(|s| s.checked_mul(1000)));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (title, length_ms) = std::mem::take(&mut pending);
        if let Some(location) = resolve_location(line) {
            items.push(Item { location, title, length_ms });
        }
    }
    items
}

fn parse_pls(text: &str, resolve_location: impl Fn(&str) -> Option<String>) -> Vec<Item> {
    // Sparse PLS files are valid. Index only actual fields and ignore the advisory entry count.
    #[derive(Default)]
    struct Row<'a> {
        file: Option<&'a str>,
        title: Option<&'a str>,
        length_ms: Option<u64>,
    }
    const MAX_ROWS: usize = 65_536;
    let mut rows = BTreeMap::<u64, Row>::new();
    let mut in_playlist = false;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.split_once(']')) {
            in_playlist = section.0.trim().eq_ignore_ascii_case("playlist");
            continue;
        }
        if !in_playlist {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let Some(pos) = key.find(|c: char| c.is_ascii_digit()) else { continue };
        let (field, number) = key.split_at(pos);
        if !["File", "Title", "Length"].iter().any(|kind| field.eq_ignore_ascii_case(kind)) {
            continue;
        }
        let Ok(index) = number.parse::<u64>() else { continue };
        if index == 0 || (rows.len() == MAX_ROWS && !rows.contains_key(&index)) {
            continue;
        }
        let row = rows.entry(index).or_default();
        let value = value.trim();
        if field.eq_ignore_ascii_case("file") {
            row.file = Some(value);
        } else if field.eq_ignore_ascii_case("title") {
            row.title = Some(value);
        } else {
            row.length_ms = value.parse::<u64>().ok().and_then(|s| s.checked_mul(1000));
        }
    }
    rows.into_values()
        .filter_map(|row| {
            Some(Item { location: resolve_location(row.file?)?, title: row.title.map(str::to_owned), length_ms: row.length_ms })
        })
        .collect()
}

fn tag_content<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = block.find(&open)?;
    let after = &block[start + open.len()..];
    let gt = after.find('>')?;
    if after[..gt].ends_with('/') {
        return Some("");
    }
    let body = &after[gt + 1..];
    let end = body.find(&format!("</{tag}>"))?;
    Some(body[..end].trim())
}

fn parse_xspf(text: &str, resolve_location: impl Fn(&str) -> Option<String>) -> Vec<Item> {
    let mut items = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<track") {
        let block_start = &rest[start..];
        let Some(end) = block_start.find("</track>") else { break };
        let block = &block_start[..end];
        if let Some(loc) = tag_content(block, "location") {
            let loc = xml_unescape(loc);
            let title = tag_content(block, "title").map(xml_unescape);
            let creator = tag_content(block, "creator").map(xml_unescape);
            let title = match (creator, title) {
                (Some(c), Some(t)) if !c.is_empty() => Some(format!("{c} - {t}")),
                (_, t) => t,
            };
            let length_ms = tag_content(block, "duration").and_then(|d| d.parse().ok());
            if let Some(location) = resolve_location(&loc) {
                items.push(Item { location, title, length_ms });
            }
        }
        rest = &block_start[end + "</track>".len()..];
    }
    items
}

fn relative_or_absolute(location: &str, base: Option<&Path>) -> String {
    if is_url(location) {
        return location.to_string();
    }
    if let Some(base) = base {
        if let Ok(rel) = Path::new(location).strip_prefix(base) {
            return rel.to_string_lossy().into_owned();
        }
    }
    location.to_string()
}

pub fn write(items: &[Item], format: PlaylistFormat, base: Option<&Path>) -> String {
    let mut out = String::new();
    match format {
        PlaylistFormat::M3u | PlaylistFormat::M3u8 => {
            out.push_str("#EXTM3U\n");
            for it in items {
                if it.title.is_some() || it.length_ms.is_some() {
                    let secs = it.length_ms.map(|l| (l / 1000) as i64).unwrap_or(-1);
                    out.push_str(&format!("#EXTINF:{secs},{}\n", it.title.as_deref().unwrap_or("")));
                }
                out.push_str(&relative_or_absolute(&it.location, base));
                out.push('\n');
            }
        }
        PlaylistFormat::Pls => {
            out.push_str("[playlist]\n");
            for (i, it) in items.iter().enumerate() {
                let n = i + 1;
                out.push_str(&format!("File{n}={}\n", relative_or_absolute(&it.location, base)));
                if let Some(t) = &it.title {
                    out.push_str(&format!("Title{n}={t}\n"));
                }
                let secs = it.length_ms.map(|l| (l / 1000) as i64).unwrap_or(-1);
                out.push_str(&format!("Length{n}={secs}\n"));
            }
            out.push_str(&format!("NumberOfEntries={}\nVersion=2\n", items.len()));
        }
        PlaylistFormat::Xspf => {
            out.push_str(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n  <trackList>\n",
            );
            for it in items {
                let loc = if is_url(&it.location) { it.location.clone() } else { path_to_file_url(&it.location) };
                out.push_str("    <track>\n");
                out.push_str(&format!("      <location>{}</location>\n", xml_escape(&loc)));
                if let Some(t) = &it.title {
                    out.push_str(&format!("      <title>{}</title>\n", xml_escape(t)));
                }
                if let Some(l) = it.length_ms {
                    out.push_str(&format!("      <duration>{l}</duration>\n"));
                }
                out.push_str("    </track>\n");
            }
            out.push_str("  </trackList>\n</playlist>\n");
        }
    }
    out
}

/// Writes a playlist file; M3U is written as UTF-8 too (lossless).
pub fn write_file(path: &Path, items: &[Item]) -> io::Result<()> {
    let format = PlaylistFormat::from_path(path).unwrap_or(PlaylistFormat::M3u8);
    let text = write(items, format, path.parent());
    crate::config::write_atomic(path, text.as_bytes())
}

pub fn items_from_entries(entries: &[Entry]) -> Vec<Item> {
    entries
        .iter()
        .map(|e| Item { location: e.location.clone(), title: Some(e.display.clone()).filter(|t| !t.is_empty()), length_ms: e.length_ms })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Item> {
        vec![
            Item { location: "/music/a b/01 – ä.mp3".into(), title: Some("Artist - Ä & <B>".into()), length_ms: Some(125_000) },
            Item { location: "http://radio.example/stream".into(), title: None, length_ms: None },
        ]
    }

    #[test]
    fn round_trip_all_formats() {
        for fmt in [PlaylistFormat::M3u8, PlaylistFormat::Pls, PlaylistFormat::Xspf] {
            let text = write(&sample(), fmt, None);
            let back = parse(&text, fmt, None);
            assert_eq!(back.len(), 2, "{fmt:?}");
            assert_eq!(back[0].location, sample()[0].location, "{fmt:?}");
            assert_eq!(back[0].title, sample()[0].title, "{fmt:?}");
            assert_eq!(back[0].length_ms, Some(125_000), "{fmt:?}");
            assert_eq!(back[1].location, "http://radio.example/stream", "{fmt:?}");
            assert_eq!(sniff(&text), Some(fmt).map(|f| if f == PlaylistFormat::M3u { PlaylistFormat::M3u8 } else { f }));
        }
    }

    #[test]
    fn relative_paths_resolve() {
        let base = Path::new("/home/u/lists");
        let items = parse("#EXTM3U\n../music/x.mp3\nsub\\y.flac\nfile:///abs/z%20z.ogg\n", PlaylistFormat::M3u, Some(base));
        let locs: Vec<_> = items.iter().map(|i| i.location.as_str()).collect();
        assert_eq!(locs, ["/home/u/music/x.mp3", "/home/u/lists/sub/y.flac", "/abs/z z.ogg"]);
        let written = write(&items, PlaylistFormat::M3u8, Some(base));
        assert!(written.contains("\nsub/y.flac\n"));
    }

    #[test]
    fn pls_tolerates_bad_counts() {
        let items = parse("[Playlist]\nNumberOfEntries=1\nFile1=http://a\nFile2=http://b\nTitle2=B\n", PlaylistFormat::Pls, None);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].title.as_deref(), Some("B"));
    }

    #[test]
    fn remote_references_stay_on_the_network() {
        let base = "https://radio.example/lists/stations.m3u";
        let cases = [
            (PlaylistFormat::M3u, "../stream\nfile:///local\n/root-relative\n"),
            (PlaylistFormat::Pls, "[playlist]\nFile1=../stream\nFile2=file:///local\nFile3=/root-relative"),
            (PlaylistFormat::Xspf, "<track><location>../stream</location></track><track><location>file:///local</location></track><track><location>/root-relative</location></track>"),
        ];
        for (format, text) in cases {
            let items = parse_remote(text, format, base);
            assert_eq!(
                items.iter().map(|i| i.location.as_str()).collect::<Vec<_>>(),
                ["https://radio.example/stream", "https://radio.example/root-relative"]
            );
        }
    }

    #[test]
    fn sparse_pls_rows_keep_order_and_metadata() {
        let text = "[playlist]\nTitle3000=last\nFile3000=c.ogg\nFile9=a.ogg\nLength9=8\nNumberOfEntries=2";
        let items = parse(text, PlaylistFormat::Pls, None);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].length_ms, Some(8000));
        assert_eq!(items[1].title.as_deref(), Some("last"));
    }

    #[test]
    fn extinf_with_attributes() {
        let items = parse("#EXTM3U\n#EXTINF:-1 tvg-name=\"x\",Radio One\nhttp://r1\n", PlaylistFormat::M3u, None);
        assert_eq!(items[0].title.as_deref(), Some("Radio One"));
        assert_eq!(items[0].length_ms, None);
    }
}
