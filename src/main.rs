// Apply the rule to the whole module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{Parser, Subcommand};
use url::Url;

// Define that crate htmx exists. The code can be found in the htmx file.
mod conversion;
pub(crate) mod editor;
pub(crate) mod htmx;
pub(crate) mod path_utils;
pub(crate) mod store;

#[tokio::main]
async fn main() {
    if let Err(e) = main_err().await {
        eprintln!("{}", e);
    }
}

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(short, long, env = "BLOOG_STORE_URL", required = true)]
    store_url: Url,

    #[arg(short, long, env = "BLOOG_PORT", default_value = "8080")]
    port: usize,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Viewer,
    Editor,
}

async fn main_err() -> Result<(), anyhow::Error> {
    let args = Args::try_parse()?;
    let store = store::Store::from_url(&args.store_url)?;
    match args.command {
        Command::Viewer => {}
        Command::Editor => {
            editor::run(
                editor::Config {
                    port: args.port as u16,
                },
                store,
            )
            .await?
        }
    }
    Ok(())
}
