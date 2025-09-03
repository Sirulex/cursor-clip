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
            Arg::new("preserve-selection")
                .long("preserve-selection")
                .help("after reading an external selection, it is immediately set by this application, so you can paste even if the original application is closed")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let preserve_selection = matches.get_flag("preserve-selection");
    let run_daemon = matches.get_flag("daemon");

    if preserve_selection && !run_daemon {
        eprintln!("--preserve-selection can only be used together with --daemon");
        std::process::exit(1);
    }

    if run_daemon {
        println!("Starting clipboard backend daemon...");
        backend::run_backend(preserve_selection).await?;
    } else {
        println!("Starting clipboard frontend...");
        frontend::run_frontend().await?;
    }

    Ok(())
}
