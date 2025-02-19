// Apply the rule to the whole module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{crate_name, crate_version, Parser, Subcommand};
use log::info;
use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::resource::{EnvResourceDetector, SdkProvidedResourceDetector, TelemetryResourceDetector};
use opentelemetry_sdk::Resource;
use std::collections::HashMap;
use tokio::task::spawn_blocking;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, registry, EnvFilter, Layer};
use url::Url;

// Define that crate htmx exists. The code can be found in the htmx file.
mod conversion;
mod customhttptrace;
pub(crate) mod editor;
pub(crate) mod htmx;
pub(crate) mod path_utils;
mod statics;
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

    #[arg(env = "BLOOG_HONEYCOMB_KEY")]
    honeycomb_key: Option<String>,

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

    let optional_tracer_provider = match &args.honeycomb_key {
        Some(honeycomb_key) => {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint("https://api.honeycomb.io/v1/traces")
                .with_headers(HashMap::from([("x-honeycomb-team".to_string(), honeycomb_key.to_string())]))
                .with_timeout(std::time::Duration::from_secs(5))
                .build()?;

            let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(
                    Resource::builder()
                        .with_attribute(KeyValue::new("crate.name", crate_name!()))
                        .with_attribute(KeyValue::new("crate.version", crate_version!()))
                        .with_detector(Box::new(TelemetryResourceDetector {}))
                        .with_detector(Box::new(SdkProvidedResourceDetector {}))
                        .with_detector(Box::new(EnvResourceDetector::new()))
                        .with_service_name(format!("bloog-{:?}", &args.command))
                        .build(),
                )
                .build();
            registry()
                .with(EnvFilter::from_default_env())
                .with(fmt::Layer::default().with_filter(EnvFilter::from_default_env()))
                .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(format!("bloog-{:?}", &args.command))))
                .init();

            Some(tracer_provider)
        }
        None => {
            tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();
            None
        }
    };

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

    if let Some(tracer_provider) = optional_tracer_provider {
        spawn_blocking(move || tracer_provider.shutdown());
    }
    Ok(())
}
