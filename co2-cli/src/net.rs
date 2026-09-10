//! The network side: one task that keeps a QUIC connection to the device, polls
//! `GetLatest` on an interval, and samples the connection's paths (direct vs relay,
//! RTT) for the connection panel. Everything reaches the UI as [`NetEvent`]s.
//!
//! The browser GUI uses `IrohLazyRemoteConnection`, which hides the connection and
//! reconnects internally. Here we hold the [`Connection`] ourselves — that's the
//! only way to read its path list — and do the reconnecting by hand.

use std::time::Duration;

use co2_proto::{GetLatest, Reading, SensorProtocol, SENSOR_ALPN};
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use irpc::Client;
use irpc_iroh::IrohRemoteConnection;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, timeout, MissedTickBehavior};
use tracing::{debug, warn};

/// How often to re-sample the path list / RTT for the connection panel.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
/// Give a single `GetLatest` this long before declaring the connection wedged.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// Pause between reconnect attempts.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Messages from the network task to the UI.
#[derive(Debug)]
pub enum NetEvent {
    /// A successful read from the sensor.
    Reading(Reading),
    /// Human-readable state, shown in the status line until the first reading lands
    /// (after that the UI shows the reading's age instead, like the web GUI).
    Status(String),
    /// Snapshot of the connection for the stats panel.
    Conn(ConnInfo),
}

/// Lifecycle of the QUIC connection to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    Reconnecting,
}

impl ConnState {
    pub fn label(self) -> &'static str {
        match self {
            ConnState::Connecting => "connecting",
            ConnState::Connected => "connected",
            ConnState::Reconnecting => "reconnecting",
        }
    }
}

/// Which transport a path runs over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Direct,
    Relay,
}

impl PathKind {
    pub fn label(self) -> &'static str {
        match self {
            PathKind::Direct => "direct",
            PathKind::Relay => "relay",
        }
    }
}

/// One open QUIC path, owned (the iroh `Path` borrows the connection).
#[derive(Debug, Clone)]
pub struct PathInfo {
    pub kind: PathKind,
    /// Remote address: `ip:port` for a direct path, the relay URL for a relay path.
    pub remote: String,
    pub rtt: Duration,
    /// Whether this is the path QUIC currently sends application data on.
    pub selected: bool,
}

/// Snapshot of the connection for the stats panel.
#[derive(Debug, Clone, Default)]
pub struct ConnInfo {
    pub state: Option<ConnState>,
    /// Open paths, selected one first.
    pub paths: Vec<PathInfo>,
}

impl ConnInfo {
    fn state(state: ConnState) -> Self {
        Self {
            state: Some(state),
            paths: Vec::new(),
        }
    }

    /// The path application data is going over right now.
    pub fn selected(&self) -> Option<&PathInfo> {
        self.paths.iter().find(|p| p.selected)
    }
}

/// Read the connection's current paths into an owned snapshot.
fn snapshot(conn: &Connection, state: ConnState) -> ConnInfo {
    let mut paths: Vec<PathInfo> = conn
        .paths()
        .iter()
        .map(|p| {
            let (kind, remote) = match p.remote_addr() {
                TransportAddr::Relay(url) => {
                    // Relay URLs come back as a FQDN with a root dot and a trailing slash.
                    let url = url.to_string();
                    let url = url.trim_end_matches('/').trim_end_matches('.');
                    (PathKind::Relay, url.to_string())
                }
                TransportAddr::Ip(addr) => (PathKind::Direct, addr.to_string()),
                other => (PathKind::Direct, format!("{other:?}")),
            };
            PathInfo {
                kind,
                remote,
                rtt: p.rtt(),
                selected: p.is_selected(),
            }
        })
        .collect();
    // Selected first, then direct before relay, then by RTT — stable for the panel.
    paths.sort_by(|a, b| {
        b.selected
            .cmp(&a.selected)
            .then_with(|| (a.kind == PathKind::Relay).cmp(&(b.kind == PathKind::Relay)))
            .then_with(|| a.rtt.cmp(&b.rtt))
    });
    ConnInfo {
        state: Some(state),
        paths,
    }
}

/// Connect, poll, sample, reconnect — forever, until the UI drops the receiver.
pub async fn run(
    endpoint: Endpoint,
    addr: EndpointAddr,
    poll_interval: Duration,
    tx: mpsc::Sender<NetEvent>,
) {
    let mut state = ConnState::Connecting;
    loop {
        if tx
            .send(NetEvent::Conn(ConnInfo::state(state)))
            .await
            .is_err()
        {
            return;
        }
        let _ = tx
            .send(NetEvent::Status(format!("{}…", state.label())))
            .await;

        let conn = match endpoint.connect(addr.clone(), SENSOR_ALPN).await {
            Ok(conn) => conn,
            Err(err) => {
                warn!("connect failed: {err:#}");
                let _ = tx
                    .send(NetEvent::Status(format!("connect error: {err}")))
                    .await;
                state = ConnState::Reconnecting;
                sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        debug!("connected to {}", conn.remote_id().fmt_short());

        if serve(&conn, poll_interval, &tx).await.is_err() {
            return; // UI is gone
        }

        conn.close(0u32.into(), b"reconnect");
        state = ConnState::Reconnecting;
        sleep(RECONNECT_DELAY).await;
    }
}

/// Drive one connection until it fails. `Err` means the UI hung up.
async fn serve(
    conn: &Connection,
    poll_interval: Duration,
    tx: &mpsc::Sender<NetEvent>,
) -> Result<(), ()> {
    let client: Client<SensorProtocol> = Client::boxed(IrohRemoteConnection::new(conn.clone()));

    let mut poll = interval(poll_interval);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sample = interval(SAMPLE_INTERVAL);
    sample.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let closed = conn.closed();
    tokio::pin!(closed);

    let send = |ev: NetEvent| async move { tx.send(ev).await.map_err(|_| ()) };

    loop {
        tokio::select! {
            _ = sample.tick() => {
                send(NetEvent::Conn(snapshot(conn, ConnState::Connected))).await?;
            }
            _ = poll.tick() => {
                match timeout(RPC_TIMEOUT, client.rpc(GetLatest)).await {
                    Ok(Ok(Some(reading))) => send(NetEvent::Reading(reading)).await?,
                    Ok(Ok(None)) => send(NetEvent::Status("no reading yet — sensor warming up…".into())).await?,
                    Ok(Err(err)) => {
                        warn!("rpc failed: {err:#}");
                        send(NetEvent::Status(format!("rpc error: {err}"))).await?;
                        return Ok(());
                    }
                    Err(_) => {
                        warn!("rpc timed out");
                        send(NetEvent::Status("rpc timed out".into())).await?;
                        return Ok(());
                    }
                }
            }
            reason = &mut closed => {
                warn!("connection closed: {reason}");
                send(NetEvent::Status(format!("connection closed: {reason}"))).await?;
                return Ok(());
            }
        }
    }
}
