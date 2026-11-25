//! Single-instance control socket.
//!
//! Protocol: the client sends one line per remote command (see
//! [`encode`]) and a final empty line; the server answers with one line
//! (status JSON or `ok`) and closes.

use crate::cli::Remote;
use linamp_core::Command;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST: usize = 1 << 20;

fn private_dir(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        result => result?,
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsafe Linamp runtime directory"));
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn socket_path(class: &str) -> io::Result<PathBuf> {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let dir = std::env::temp_dir().join(format!("linamp-{}", rustix::process::geteuid().as_raw()));
            private_dir(&dir)?;
            dir
        }
    }
    .join("linamp");
    private_dir(&dir)?;
    let safe: String = class.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    Ok(dir.join(format!("{safe}.sock")))
}

pub fn absent(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn encode(r: &Remote) -> String {
    match r {
        Remote::Open(files) => format!("open\t{}", files.iter().map(|f| escape(f)).collect::<Vec<_>>().join("\t")),
        Remote::Add(files) => format!("add\t{}", files.iter().map(|f| escape(f)).collect::<Vec<_>>().join("\t")),
        Remote::Cmd(c) => format!("cmd\t{}", c.id()),
        Remote::SetVolumePercent(v) => format!("vol\t{v}"),
        Remote::VolumeStep(v) => format!("volstep\t{v}"),
        Remote::SetPan(v) => format!("pan\t{v}"),
        Remote::PanStep(v) => format!("panstep\t{v}"),
        Remote::Shuffle(v) => format!("shuffle\t{}", v.map(|b| (b as u8).to_string()).unwrap_or_default()),
        Remote::Repeat(v) => format!("repeat\t{}", v.map(|b| (b as u8).to_string()).unwrap_or_default()),
        Remote::SeekMs(ms) => format!("seek\t{ms}"),
        Remote::Clear => "clear".into(),
        Remote::Show => "show".into(),
        Remote::Quit => "quit".into(),
        Remote::Status => "status".into(),
    }
}

pub fn decode(line: &str) -> Option<Remote> {
    let mut parts = line.split('\t');
    let verb = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let arg = rest.first().copied().unwrap_or("");
    let opt_bool = |s: &str| match s {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    };
    Some(match verb {
        "open" => Remote::Open(rest.iter().filter(|s| !s.is_empty()).map(|s| unescape(s)).collect()),
        "add" => Remote::Add(rest.iter().filter(|s| !s.is_empty()).map(|s| unescape(s)).collect()),
        "cmd" => Remote::Cmd(Command::from_id(arg.parse().ok()?)?),
        "vol" => Remote::SetVolumePercent(arg.parse().ok()?),
        "volstep" => Remote::VolumeStep(arg.parse().ok()?),
        "pan" => Remote::SetPan(arg.parse().ok()?),
        "panstep" => Remote::PanStep(arg.parse().ok()?),
        "shuffle" => Remote::Shuffle(opt_bool(arg)),
        "repeat" => Remote::Repeat(opt_bool(arg)),
        "seek" => Remote::SeekMs(arg.parse().ok()?),
        "clear" => Remote::Clear,
        "show" => Remote::Show,
        "quit" => Remote::Quit,
        "status" => Remote::Status,
        _ => return None,
    })
}

/// Sends commands to a running instance and returns its reply.
pub fn send(path: &Path, remotes: &[Remote]) -> io::Result<String> {
    send_connected(UnixStream::connect(path)?, remotes)
}

pub fn forward_on_contention(path: &Path, remotes: &[Remote]) -> io::Result<String> {
    let deadline = Instant::now() + IPC_TIMEOUT;
    loop {
        match UnixStream::connect(path) {
            Err(e) if absent(&e) && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            result => return send_connected(result?, remotes),
        }
    }
}

fn send_connected(mut s: UnixStream, remotes: &[Remote]) -> io::Result<String> {
    s.set_read_timeout(Some(IPC_TIMEOUT))?;
    s.set_write_timeout(Some(IPC_TIMEOUT))?;
    let mut msg = String::new();
    for r in remotes {
        msg.push_str(&encode(r));
        msg.push('\n');
    }
    msg.push('\n');
    s.write_all(msg.as_bytes())?;
    let mut reply = String::new();
    if BufReader::new(s.take(MAX_REQUEST as u64)).read_line(&mut reply)? == 0 || !reply.ends_with('\n') {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete reply from Linamp"));
    }
    Ok(reply.trim_end().to_string())
}

#[derive(Debug)]
pub struct Request {
    pub remotes: Vec<Remote>,
    pub reply: crossbeam_channel::Sender<String>,
}

/// The lock file stays in place so all launchers lock the same inode.
pub struct Endpoint {
    path: PathBuf,
    inode: (u64, u64),
    listener: UnixListener,
    _lock: File,
}

impl Endpoint {
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let lock = File::options().read(true).write(true).create(true).truncate(false).mode(0o600).open(path.with_extension("lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(io::ErrorKind::AddrInUse.into()),
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if !meta.file_type().is_socket() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "control path is not a socket"));
            }
            // A running older version may own this socket without holding a lock.
            match UnixStream::connect(&path) {
                Ok(_) => return Err(io::ErrorKind::AddrInUse.into()),
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(&path)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        let listener = UnixListener::bind(&path)?;
        let meta = fs::symlink_metadata(&path)?;
        Ok(Self { path, inode: (meta.dev(), meta.ino()), listener, _lock: lock })
    }

    pub fn serve(&self, deliver: impl Fn(Request) + Send + 'static) -> io::Result<()> {
        let listener = self.listener.try_clone()?;
        std::thread::Builder::new().name("linamp-ipc".into()).spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let Ok(remotes) = read_request(&mut stream) else { continue };
                let (tx, rx) = crossbeam_channel::bounded(1);
                deliver(Request { remotes, reply: tx });
                if let Ok(reply) = rx.recv_timeout(IPC_TIMEOUT) {
                    let _ = stream.set_write_timeout(Some(IPC_TIMEOUT));
                    let _ = writeln!(stream, "{reply}");
                }
            }
        })?;
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.inode) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn read_request(stream: &mut UnixStream) -> io::Result<Vec<Remote>> {
    let mut data = Vec::new();
    let mut buf = [0; 8192];
    let deadline = Instant::now() + IPC_TIMEOUT;
    while data.len() < MAX_REQUEST {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(remaining))?;
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let start = data.len().saturating_sub(1);
        data.extend_from_slice(&buf[..n]);
        if data.len() > MAX_REQUEST {
            break;
        }
        if let Some(end) = data[start..].windows(2).position(|w| w == b"\n\n") {
            let msg = std::str::from_utf8(&data[..start + end]).map_err(io::Error::other)?;
            return Ok(msg.lines().filter_map(decode).collect());
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidData, "control request exceeds size limit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let all = [
            Remote::Open(vec!["/a b/c\td.mp3".into(), "line\nbreak".into()]),
            Remote::Add(vec!["x".into()]),
            Remote::Cmd(Command::Next),
            Remote::SetVolumePercent(40),
            Remote::VolumeStep(-5),
            Remote::SetPan(-30),
            Remote::PanStep(6),
            Remote::Shuffle(None),
            Remote::Repeat(Some(true)),
            Remote::SeekMs(1234),
            Remote::Clear,
            Remote::Show,
            Remote::Quit,
            Remote::Status,
        ];
        for r in all {
            if let Remote::Open(files) = &r {
                // Tabs inside names are not representable; skip that case.
                if files.iter().any(|f| f.contains('\t')) {
                    continue;
                }
            }
            assert_eq!(decode(&encode(&r)), Some(r.clone()), "{r:?}");
        }
        assert_eq!(decode(&encode(&Remote::Open(vec!["line\nbreak".into()]))), Some(Remote::Open(vec!["line\nbreak".into()])));
    }

    #[test]
    fn owner_cleanup_preserves_replacement_socket() {
        let path = std::env::temp_dir().join(format!("linamp-replaced-{}.sock", std::process::id()));
        let owner = Endpoint::bind(path.clone()).unwrap();
        fs::remove_file(&path).unwrap();
        let replacement = UnixListener::bind(&path).unwrap();
        drop(owner);
        assert!(UnixStream::connect(&path).is_ok());
        drop(replacement);
        // Recover the stale socket and release it on exit.
        drop(Endpoint::bind(path.clone()).unwrap());
        assert!(!path.exists());
        fs::remove_file(path.with_extension("lock")).unwrap();
    }

    #[test]
    fn socket_round_trip() {
        let path = std::env::temp_dir().join(format!("linamp-test-{}.sock", std::process::id()));
        let endpoint = Endpoint::bind(path.clone()).unwrap();
        assert_eq!(Endpoint::bind(path.clone()).err().unwrap().kind(), io::ErrorKind::AddrInUse);
        endpoint
            .serve(|req| {
                let _ = req.reply.send(format!("got {}", req.remotes.len()));
            })
            .unwrap();
        let reply = send(&path, &[Remote::Status, Remote::Cmd(Command::Play)]).unwrap();
        assert_eq!(reply, "got 2");
        drop(endpoint);
        assert!(!path.exists());
        fs::remove_file(path.with_extension("lock")).unwrap();
    }
}
