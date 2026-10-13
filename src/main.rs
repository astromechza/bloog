// Apply the rule to the whole module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{Parser, Subcommand, crate_name, crate_version};
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::resource::{EnvResourceDetector, SdkProvidedResourceDetector, TelemetryResourceDetector};
use std::collections::HashMap;
use std::process::ExitCode;
use tokio::task::spawn_blocking;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt, registry};
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
async fn main() -> ExitCode {
    match main_err().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{:#}", e);
            ExitCode::FAILURE
        }
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
    port: u16,

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
    // parse() prints help, version and usage errors itself and exits with the right code.
    let args = Args::parse();

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
                        .with_service_name(format!("bloog-{:?}", args.command))
                        .build(),
                )
                .build();
            registry()
                .with(EnvFilter::from_default_env())
                .with(fmt::Layer::default().with_filter(EnvFilter::from_default_env()))
                .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(format!("bloog-{:?}", args.command))))
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
            honeycomb_key: args.honeycomb_key.as_ref().map(|_| "<redacted>".to_string()),
            ..args.clone()
        }
    );
    let store = store::Store::from_url(&args.store_url)?;

    info!("Starting {:?}..", args.command);
    let result = match args.command {
        Command::Viewer => viewer::run(viewer::Config { port: args.port }, store).await,
        Command::Editor => editor::run(editor::Config { port: args.port }, store).await,
    };

    // Flush any buffered spans before exiting, regardless of how the server stopped.
    if let Some(tracer_provider) = optional_tracer_provider {
        match spawn_blocking(move || tracer_provider.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("failed to shutdown tracer provider: {}", e),
            Err(e) => eprintln!("failed to join tracer provider shutdown: {}", e),
        }
    }
    result
}

/// Resolves when the process receives Ctrl-C or SIGTERM so that the servers can shut down gracefully.
pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("Shutdown signal received..");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_must_fit_u16() {
        let parse = |port: &str| Args::try_parse_from(["bloog", "--store-url", "file:///tmp/x", "--port", port, "viewer"]);
        assert!(parse("8080").is_ok_and(|a| a.port == 8080));
        assert!(parse("70000").is_err());
    }
}
