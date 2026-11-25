# Desktop player review

This checklist tracks findings from the review of Linamp 0.1.0. It covers opening files from the desktop, importing playlists and skins, saving user data, audio playback, and starting multiple instances.

| Area | Work item |
| --- | --- |
| Remote imports | Validate YouTube URLs at the helper boundary and terminate option parsing before the URL. |
| Playlists | Resolve remote entries against the fetched URL and restrict them to HTTP and HTTPS. Bound PLS parsing by the input actually present. |
| Skins | Bound archive and bitmap memory use. Validate a replacement skin before installing it over an existing one. |
| File operations | Confirm the final export filename. Delete only confirmed tracks and remove only successfully deleted entries. Give each atomic save its own staging file. |
| Playlist UI | Preserve request order, discard superseded imports, index metadata updates by entry ID and coalesce playlist redraws. |
| Audio | Keep ALSA channel configuration immutable. Correct time-base conversions and failed seeks. Drain and cancel external audio helpers. |
| Instance management | Give the control socket one owner and distinguish a busy player from an absent player. |
| Window management | Fall back to individual panel requests when grouping fails, tracked in upstream PR #2. |
| Dependencies and checks | Update the affected url and rustix patch versions and restore the strict Clippy check. |

## Importing playlists and skins

A downloaded playlist used to lose its source URL during parsing. A relative track name could reach a local metadata reader. The importer now resolves M3U, PLS and XSPF entries against the final HTTP response URL, including after redirects, and accepts HTTP(S) destinations. Local playlists retain filesystem-relative lookup. The PLS parser indexes the numbered fields actually present, supports sparse numbering and caps the index at 65,536 rows. An inaccurate NumberOfEntries can no longer drive an unbounded search.

The YouTube helper now parses and checks the scheme and hostname at invocation time, then passes the serialized URL after the option terminator. Background metadata and playlist lookups use the same boundary as playback.

The skin reader previously collected every archive member with a matching extension. It now reads only named skin assets, with limits of 4,096 entries, 64 MiB per archive, 32 MiB of retained asset bytes and 1,024 pixels per bitmap dimension. Classic skins with missing sheets still use the built-in fallback. Installation decodes a staged copy before replacing the installed file, so a failed import preserves the previous skin.

## Saving files and updating the queue

The filename entered in a save dialog can differ from the filename Linamp generates by adding an extension. Playlist and EQ exports now use an exclusive create for the generated destination. If that file exists, the user sees its actual name before replacement. Each save uses a unique staging file in its destination directory so settings, playlists and EQ files can be saved concurrently without sharing temporary filenames.

The asynchronous deletion dialog now carries both an entry ID and its path. Confirmation checks that each entry still has that path, deletes each distinct file once and removes only matching entries after success. A changed selection or failed deletion no longer removes unrelated playlist rows.

Open and enqueue requests receive sequence numbers. A new replacement discards earlier pending requests, and appends commit in submission order even when workers finish out of order. Metadata replies carry both the entry ID and location, so an edited entry can reject obsolete tags. An ID index speeds up lookup and window redraw requests coalesce bursts of metadata updates.

## Audio and desktop behavior

ALSA writes now use the private channel count configured when the device opened. Decoder seek offsets convert timestamp ticks to PCM frames using the track time base, and bitrate estimation counts decoded frames. A failed seek preserves the clock and queued output. External audio helpers use nonblocking reads for both output pipes, retain a bounded diagnostic tail and release child processes on cancellation. YouTube metadata lookups also have a time and output-size limit.

Each named player takes a filesystem lock before binding its control socket. A live instance keeps ownership when its reply is slow. Concurrent launchers can forward to the winner, stale endpoints can be recovered, and shutdown checks the socket identity before removing it. IPC reads also have a total size and time limit.

The separate taskbar patch records successful window grouping for each panel, allowing the player's minimize and raise commands to handle ungrouped panels individually if X11 hint setup fails.

## Dependencies

The lockfile upgrades url to 2.5.9 and rustix 1.x to 1.1.6 for RUSTSEC-2026-0592 and RUSTSEC-2026-0486. The skin reader uses indexed in-memory ZIP reads. The ZIP filesystem extraction APIs cited during review are outside that path.
