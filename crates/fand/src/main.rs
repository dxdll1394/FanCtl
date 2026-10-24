//! `fand` entry point.

use std::path::PathBuf;

use fancore::config::Paths;
use fancore::smc;
use fand::{restore_auto_standalone, serve, Daemon};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dry_run = false;
    let mut socket_override: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => dry_run = true,
            "--restore-auto" => {
                restore_auto_standalone();
                return;
            }
            "--socket" => {
                i += 1;
                socket_override = args.get(i).map(PathBuf::from);
            }
            "--version" | "-V" => {
                println!("fand {}", fancore::VERSION);
                return;
            }
            "--help" | "-h" => {
                println!("用法: fand [--dry-run] [--socket PATH]");
                println!("      fand --restore-auto    一次性把风扇交还系统（需要 root）");
                println!("  以 root 运行才能写入风扇；--dry-run 只读，不写 SMC。");
                return;
            }
            other => {
                eprintln!("未知参数 {other}，用 --help 查看用法");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // `--dry-run` keeps everything inside a scratch directory so it can run as
    // an unprivileged user without touching the real config.
    let paths = if dry_run { Paths::user() } else { Paths::system() };
    let socket = socket_override.unwrap_or_else(|| paths.socket.clone());
    if !dry_run && !smc::is_root() {
        eprintln!("fand 需要 root 才能写风扇；请用 sudo 运行，或加 --dry-run 只观察。");
        std::process::exit(1);
    }
    let daemon = match Daemon::new(paths, dry_run) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("初始化失败：{e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = serve(daemon, &socket) {
        eprintln!("daemon 退出：{e}");
        std::process::exit(1);
    }
}
