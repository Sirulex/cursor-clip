use clap::{Arg, Command};

mod backend;
mod frontend;
mod shared;

#[tokio::main(flavor = "multi_thread", worker_threads = 3)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new("cursor-clip")
        .version("0.1.0")
        .about("Clipboard manager with GUI overlay")
        .arg(
            Arg::new("daemon")
                .long("daemon")
                .help("Run as background daemon")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("monitor-only")
                .long("monitor-only")
                .help("Do not take ownership of a newly received external selection; just record it. This does not automatically ensure clipboard persistence if the original application is closed. You can still paste the selection by choosing it in the GUI. If unsure, you probably want to keep the default behaviour and don't use this flag.")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let monitor_only = matches.get_flag("monitor-only");
    let run_daemon = matches.get_flag("daemon");

    if monitor_only && !run_daemon {
        eprintln!("--monitor-only can only be used together with --daemon");
        std::process::exit(1);
    }

    if run_daemon {
        println!("Starting clipboard backend daemon...");
        backend::run_backend(monitor_only).await?;
    } else {
        println!("Starting clipboard frontend...");
        frontend::run_frontend().await?;
    }

    Ok(())
}
