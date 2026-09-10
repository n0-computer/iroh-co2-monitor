//! Terminal GUI for the iroh CO2 monitor: the same "big CO2 number" display as the
//! browser GUI (`co2-wasm`), rendered with ratatui, plus a connection panel showing
//! whether the QUIC connection is running over a direct path or a relay, and its RTT.
//!
//! Unlike the browser this is a full native iroh endpoint (UDP + relay + discovery),
//! so it can hole-punch to the device and you can watch the path flip from relay to
//! direct — the panel exists to make that visible.
//!
//! Single command: `co2-cli <ENDPOINT_ID>` (an endpoint ticket also works).

mod net;
mod ui;

use std::time::Duration;

use clap::Parser;
use iroh::{EndpointAddr, EndpointId};
use iroh_tickets::endpoint::EndpointTicket;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use crate::net::NetEvent;
use crate::ui::App;

/// Terminal monitor for the iroh CO2 sensor.
#[derive(Debug, Parser)]
#[command(name = "co2-cli", version, about)]
struct Args {
    /// Endpoint ID of the CO2 monitor (as printed on the firmware's serial console).
    /// A full endpoint ticket is accepted too.
    target: String,

    /// Seconds between reading polls. The SCD30 updates every ~2 s.
    #[arg(short, long, default_value_t = 5, value_name = "SECS")]
    interval: u64,
}

/// Parse the positional target: an endpoint ticket if it looks like one, otherwise a
/// bare endpoint ID (which iroh resolves through discovery, like the short ticket).
fn parse_target(s: &str) -> anyhow::Result<EndpointAddr> {
    let s = s.trim();
    if let Ok(ticket) = s.parse::<EndpointTicket>() {
        return Ok(ticket.into());
    }
    match s.parse::<EndpointId>() {
        Ok(id) => Ok(EndpointAddr::new(id)),
        Err(e) => anyhow::bail!("`{s}` is neither an endpoint ID nor an endpoint ticket: {e}"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let addr = parse_target(&args.target)?;

    // The TUI owns stdout, so logs go to stderr and only when asked for:
    //   RUST_LOG=debug co2-cli <id> 2> co2-cli.log
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let me = endpoint.id();
    let remote = addr.id;

    // Network task → UI: readings, status text, and connection snapshots.
    let (net_tx, net_rx) = mpsc::channel::<NetEvent>(64);
    let net_task = tokio::spawn(net::run(
        endpoint.clone(),
        addr,
        Duration::from_secs(args.interval.max(1)),
        net_tx,
    ));

    let app = App::new(me, remote);
    let result = ui::run(app, net_rx).await;

    net_task.abort();
    endpoint.close().await;
    result
}
