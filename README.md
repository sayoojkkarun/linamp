# Linamp

A Winamp-style skinnable audio player for Linux, written in Rust. Linamp contains no Winamp code or artwork. Its default skin
is drawn procedurally, and classic `.wsz` skins load at runtime.

## Features

- **Classic windows:** Main, Equalizer and Playlist Editor, drawn pixel-exact
  from the skin at 1x or double size. Windows dock and snap together, and
  support windowshade mode, EasyMove, always-on-top and `region.txt` shapes.
- **Classic skins:** `.wsz`/`.zip` archives or unpacked directories. Each file
  falls back to the default skin if missing or broken. `viscolor.txt` and
  `pledit.txt` are supported.
- **Playback:**
  - Formats: MP3, AAC/M4A, ALAC, FLAC, Ogg Vorbis, WAV, AIFF and MKV audio,
    decoded by [Symphonia](https://github.com/pdeljanov/Symphonia).
  - Gapless playback, stop with fadeout, stop after current, ReplayGain.
  - HTTP/Icecast/SHOUTcast streams with ICY titles.
- **Equalizer:** 10 bands plus preamp, with either the Winamp or the ISO
  band set and an optional limiter. Reads and writes `.eqf` and `.q1`
  presets, and can auto-load presets per file.
- **Playlist:**
  - M3U, M3U8, PLS and XSPF.
  - Sort, reverse, randomize, crop, remove dead entries.
  - Drag to reorder, jump to file or time.
  - Advanced Title Formatting (`[%artist% - ]$if2(%title%,…)`).
- **Visualization:** classic spectrum analyzer (normal, fire and line styles,
  peaks) and oscilloscope (dots, lines, solid).
- **Control:**
  - Winamp keyboard shortcuts and Winamp-style menus.
  - Command line with `/PLAY /NEXT /VOL=50 /COMMAND=40045 …` sent to the
    running instance.
  - MPRIS2 for media keys, desktop widgets and `playerctl`.

## Building

You need a Rust toolchain (1.82 or newer) and an X11 or XWayland session. No
system development packages are needed. ALSA (`libasound.so.2`) is loaded at
runtime, and on PipeWire/PulseAudio systems the `default` device is used.

```sh
cargo build --release
./target/release/linamp ~/Music
```

To install for the current user, with a desktop entry and `.wsz` MIME type:

```sh
packaging/install.sh
```

Optional tools: `zenity` or `kdialog` for the file and URL dialogs, and
`fc-match` (fontconfig) for the playlist font.

## Using

| Action | How |
|---|---|
| Open files | `L`, the eject button, or drag files onto a window |
| Transport | `Z X C V B` (prev, play, pause, stop, next) |
| Seek / volume | arrow keys, or drag the bars; mouse wheel changes volume |
| Main menu | click the top-left button, right-click, or `Alt+F` |
| Skins | `Alt+S`, or put `.wsz` files in `~/.local/share/linamp/skins/` |
| Equalizer keys | `1`–`0` / `Q`–`P` raise/lower bands, `N` on/off, `S` presets |
| Double size / shade | `Ctrl+D` / `Ctrl+W` (or double-click the title bar) |

Remote control examples:

```sh
linamp /ADD song.flac      # enqueue in the running instance
linamp /PLAYPAUSE
linamp /VOL=40 /SHUFFLE=1
linamp /STATUS             # JSON status of the running instance
linamp /NEW                # start a second, independent instance
```

Settings live in `~/.config/linamp/linamp.ini`. Its key names follow
`winamp.ini`. The playlist is saved to `~/.config/linamp/linamp.m3u8`.

## Layout

| Crate | Contents |
|---|---|
| `crates/linamp-core` | settings, playlist model and formats, title formatting, EQF, command ids |
| `crates/linamp-audio` | decoding, EQ, limiter, ALSA output, engine thread, streams |
| `crates/linamp-skin` | skin loading, sprite geometry, default skin generator |
| `apps/linamp` | windows, menus, input, dialogs, IPC, MPRIS |

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p linamp-audio --example play -- --vol 0 file.mp3   # headless engine test
cargo run -p linamp-skin --example dump_sheets -- skin.wsz out/ # dump sprite sheets
LINAMP_SNAPSHOT=out/ linamp /NEW /INIDIR=/tmp/cfg            # render windows to PNG and exit
```
