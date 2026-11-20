use clap::Command;
use cursor_clip::shared;

mod frontend;
#[cfg(test)]
mod test_support;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Command::new(env!("CARGO_BIN_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about("Open or toggle the clipboard history overlay")
        .get_matches();

    cursor_clip::init_logging()?;
    frontend::run_frontend()
}
