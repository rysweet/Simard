use simard::dispatch_operator_cli;
use tracing_subscriber::prelude::*;

fn main() -> std::process::ExitCode {
    init_tracing();
    simard::update_check::run_update_check();

    let result = match dispatch_operator_cli(std::env::args().skip(1)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "command failed");
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    };

    opentelemetry::global::shutdown_tracer_provider();
    simard::telemetry::shutdown_metrics();
    result
}

/// Initialize structured tracing with optional OTEL export.
///
/// Diagnostic logs are written to **stderr** so that stdout carries only a
/// command's actual output — e.g. `simard memory stats --json` must emit
/// nothing but the JSON document so it stays pipe-safe (`… | jq`). The
/// dependency stack (notably `amplihack-memory`) logs at `info` on store
/// open; routing those to stderr keeps machine-readable stdout clean.
///
/// - `RUST_LOG` controls verbosity (default: info)
/// - `SIMARD_LOG_JSON=1` enables JSON log output
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` enables OTLP span export (e.g. http://localhost:4317)
///
/// Human/JSON log lines are always written to **stderr** so that stdout carries
/// only program output (e.g. the `--json` payloads emitted by `simard memory
/// stats|dump`). Routing logs to stdout would interleave dependency log lines
/// such as `amplihack_memory::graph::lbug_store: effective LadybugDB limits ...`
/// ahead of the JSON, breaking machine-readable parsing (issue #2340).
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

    // Install the unified telemetry MeterProvider alongside the tracer. The
    // provider is ALWAYS installed (so the facade's instruments are real, not
    // the global no-op); OTLP metric export is attached only when the endpoint
    // is set — identical gating to traces — so the default deployment stays
    // fully in-process. The in-process registry read by `simard status` is
    // unaffected either way (issue #2528).
    simard::telemetry::init_metrics(endpoint.as_deref());

    // Logs go to STDERR, not stdout, so they never corrupt a command's stdout
    // result (e.g. `simard memory stats --json`, whose stdout must be parseable
    // JSON). Without this, an INFO log emitted on a dependency's store-open path
    // interleaves with the JSON and breaks downstream parsers.
    if use_json {
        let otel = endpoint
            .as_deref()
            .and_then(|ep| make_otel_tracer(ep).ok())
            .map(|t| tracing_opentelemetry::layer().with_tracer(t));
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_target(true)
                    .with_writer(std::io::stderr),
            )
            .with(otel)
            .with(simard::trace_collector::SpanCollectorLayer)
            .init();
    } else {
        let otel = endpoint
            .as_deref()
            .and_then(|ep| make_otel_tracer(ep).ok())
            .map(|t| tracing_opentelemetry::layer().with_tracer(t));
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_writer(std::io::stderr),
            )
            .with(otel)
            .with(simard::trace_collector::SpanCollectorLayer)
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
