use simard::dispatch_operator_cli;
use tracing_subscriber::prelude::*;

fn main() -> std::process::ExitCode {
    init_tracing();

    let result = match dispatch_operator_cli(std::env::args().skip(1)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "command failed");
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    };

    opentelemetry::global::shutdown_tracer_provider();
    result
}

/// Initialize structured tracing with optional OTEL export.
///
/// - `RUST_LOG` controls verbosity (default: info)
/// - `SIMARD_LOG_JSON=1` enables JSON log output
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` enables OTLP span export (e.g. http://localhost:4317)
fn init_tracing() {
    let use_json = std::env::var("SIMARD_LOG_JSON")
        .map(|v| matches!(v.as_str(), "1" | "true"))
        .unwrap_or(false);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();

    if let Some(ref ep) = endpoint {
        eprintln!("[simard] OTEL tracing enabled → {ep}");
    }

    // Each branch creates the otel layer inline so Rust infers the subscriber
    // type parameter correctly for the layered stack.
    if use_json {
        let otel = endpoint
            .as_deref()
            .and_then(|ep| make_otel_tracer(ep).ok())
            .map(|t| tracing_opentelemetry::layer().with_tracer(t));
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json().with_target(true))
            .with(otel)
            .init();
    } else {
        let otel = endpoint
            .as_deref()
            .and_then(|ep| make_otel_tracer(ep).ok())
            .map(|t| tracing_opentelemetry::layer().with_tracer(t));
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(true))
            .with(otel)
            .init();
    }
}

fn make_otel_tracer(
    endpoint: &str,
) -> Result<opentelemetry_sdk::trace::Tracer, Box<dyn std::error::Error + Send + Sync>> {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::TracerProvider;

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let resource = Resource::new(vec![
        KeyValue::new("service.name", "simard"),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("simard");
    opentelemetry::global::set_tracer_provider(provider);

    Ok(tracer)
}
