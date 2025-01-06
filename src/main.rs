// Apply the rule to the whole module.
#![deny(clippy::unwrap_used,clippy::expect_used,clippy::panic)]

// Define that crate htmx exists. The code can be found in the htmx file.
pub(crate) mod htmx;
pub(crate) mod editor;
pub(crate) mod store;
pub(crate) mod path_utils;

#[tokio::main]
async fn main() {
    if let Err(e) = main_err().await {
        eprintln!("{}", e);
    }
}

async fn main_err() -> Result<(), anyhow::Error> {
    editor::run(editor::Config::default(), store::Store::default()).await?;
    Ok(())
}
