// Apply the rule to the whole module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{Parser, Subcommand};
use log::info;
use url::Url;

// Define that crate htmx exists. The code can be found in the htmx file.
mod conversion;
pub(crate) mod editor;
pub(crate) mod htmx;
pub(crate) mod path_utils;
pub(crate) mod store;
mod viewer;
mod viewhelpers;

#[tokio::main]
async fn main() {
    if let Err(e) = main_err().await {
        eprintln!("{}", e);
    }
}

#[derive(Parser, Debug, Clone)]
#[command(version, about)]
struct Args {
    #[arg(
        short,
        long,
        env = "BLOOG_STORE_URL",
        required = true,
        help = "The arrow/object_store url schema with config options as query args."
    )]
    store_url: Url,

    #[arg(short, long, env = "BLOOG_PORT", default_value = "8080", help = "The HTTP port to listen on.")]
    port: usize,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Launch the read-only viewer process.
    Viewer,
    /// Launch the read-write editor process.
    Editor,
}

async fn main_err() -> Result<(), anyhow::Error> {
    let args = Args::try_parse()?;
    env_logger::init();
    let mut anonymous_url = args.store_url.clone();
    anonymous_url.set_query(None);
    let _ = anonymous_url.set_password(None);
    info!(
        "Parsed args {:?}, creating store..",
        Args {
            store_url: anonymous_url,
            ..args.clone()
        }
    );
    let store = store::Store::from_url(&args.store_url)?;
    info!("Starting {:?}..", args.command);
    match args.command {
        Command::Viewer => viewer::run(viewer::Config { port: args.port as u16 }, store).await?,
        Command::Editor => editor::run(editor::Config { port: args.port as u16 }, store).await?,
    }
    Ok(())
}
