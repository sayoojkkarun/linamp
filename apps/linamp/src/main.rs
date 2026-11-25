//! Linamp: a Winamp-style skinnable audio player for Linux.

mod app;
mod cli;
mod dialogs;
mod font;
mod ipc;
mod menu;
mod model;
mod mpris;
mod render;
mod shape;
mod ui;
mod ui_eq;
mod ui_main;
mod ui_pl;
mod vis;

use cli::Remote;
use linamp_core::config::{Config, Paths};
use model::{AppEvent, Model};
use winit::event_loop::EventLoop;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let opts = match cli::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("linamp: {e}");
            std::process::exit(2);
        }
    };
    if opts.help {
        print!("{}", cli::HELP);
        return;
    }
    if opts.version {
        println!("linamp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let class = opts.instance_class.clone().unwrap_or_else(|| "default".into());
    let socket_class =
        if opts.new_instance && opts.instance_class.is_none() { format!("new-{}", std::process::id()) } else { class.clone() };
    let fail = |err: std::io::Error| -> ! {
        eprintln!("linamp: {err}");
        std::process::exit(1);
    };
    let socket = ipc::socket_path(&socket_class).unwrap_or_else(|e| fail(e));
    let mut forwarded = opts.remotes.clone();
    if opts.kill {
        forwarded = vec![Remote::Quit];
    }
    if forwarded.is_empty() {
        forwarded.push(Remote::Show);
    }
    let print_reply = |reply: String| {
        if forwarded.contains(&Remote::Status) {
            println!("{reply}");
        }
    };
    if !opts.new_instance {
        match ipc::send(&socket, &forwarded) {
            Ok(reply) => return print_reply(reply),
            Err(e) if !ipc::absent(&e) => fail(e),
            Err(_) if forwarded.iter().all(|r| matches!(r, Remote::Quit | Remote::Status)) => {
                eprintln!("linamp: no running instance");
                std::process::exit(1);
            }
            Err(_) => {}
        }
    }
    let endpoint = match ipc::Endpoint::bind(socket.clone()) {
        Ok(endpoint) => endpoint,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && !opts.new_instance => {
            return print_reply(ipc::forward_on_contention(&socket, &forwarded).unwrap_or_else(|e| fail(e)));
        }
        Err(e) => fail(e),
    };

    if let Err(e) = linamp_audio::alsa::available() {
        log::error!("{e} — audio output will not work");
    }
    let paths = Paths::resolve(opts.ini_dir.clone());
    let ini = opts.config_file.clone().unwrap_or_else(|| paths.ini_file());
    let config = Config::load(ini);
    if opts.del_m3u {
        let _ = std::fs::remove_file(paths.playlist_file());
    }

    let event_loop = match EventLoop::<AppEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("linamp: cannot open a display: {e}");
            drop(endpoint);
            std::process::exit(1);
        }
    };
    let proxy = event_loop.create_proxy();
    let mut model = Model::new(paths, config, proxy.clone());
    model.load_session();

    let server_proxy = proxy.clone();
    if let Err(e) = endpoint.serve(move |req| {
        let _ = server_proxy.send_event(AppEvent::Remote(req));
    }) {
        eprintln!("linamp: cannot start control socket: {e}");
        drop(endpoint);
        std::process::exit(1);
    }

    let reply = model.on_remote(opts.remotes.clone());
    if opts.remotes.contains(&Remote::Status) {
        println!("{reply}");
    }

    let mpris = mpris::spawn(proxy.clone(), model.engine.shared().clone(), &class);
    let mut app = app::App::new(model, opts.start_minimized, mpris);
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop: {e}");
    }
    drop(endpoint);
}
