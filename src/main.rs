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
        .get_matches();

    if matches.get_flag("daemon") {
        println!("Starting clipboard backend daemon...");
        backend::run_backend().await?;
    } else {
        println!("Starting clipboard frontend...");
        frontend::run_frontend().await?;
    }

    Ok(())
}
