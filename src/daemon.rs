use clap::{Arg, ArgAction, Command};
use cursor_clip::shared;

mod backend;
#[cfg(test)]
mod test_support;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new(env!("CARGO_BIN_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about("Monitor the Wayland clipboard and serve clipboard history")
        .arg(
            Arg::new("monitor-only")
                .long("monitor-only")
                .help("Record external selections without taking ownership. If the source application closes, select the item in the overlay to paste it again.")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    cursor_clip::init_logging()?;
    backend::run_backend(matches.get_flag("monitor-only")).await
}
