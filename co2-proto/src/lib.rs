//! Shared irpc protocol for the iroh CO2 monitor.
//!
//! Defines the wire types and the irpc service surface used by both the firmware
//! (server) and the wasm GUI (client). Deliberately board-agnostic: no esp-idf deps
//! and no `[patch]`, so it builds on the host as well as on `riscv32imac-esp-espidf`.
//!
//! The CO2 monitor is read-only — an SCD30 reports CO2 / temperature / humidity and
//! there's nothing to actuate — so the protocol is just "give me the latest reading".

use irpc::channel::oneshot;
use irpc::rpc_requests;
use serde::{Deserialize, Serialize};

/// The ALPN for the CO2 sensor RPC protocol.
pub const SENSOR_ALPN: &[u8] = b"iroh-co2/sensor/0";

/// A single sensor reading from the SCD30.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Reading {
    /// CO2 concentration, ppm.
    pub co2: f32,
    /// Temperature, °C (raw sensor value — the SCD30 self-heats a few °C).
    pub temperature: f32,
    /// Relative humidity, %.
    pub humidity: f32,
}

/// Request the most recent reading. Returns `None` until the first successful read
/// (e.g. while the sensor is still warming up or if it isn't wired yet).
#[derive(Debug, Serialize, Deserialize)]
pub struct GetLatest;

/// The sensor RPC service. `rpc_requests` generates the [`SensorMessage`] enum
/// (the channel-carrying form) consumed by the server handler.
///
/// Variants are append-only: each new method goes at the end so existing
/// discriminants are undisturbed and older clients keep working.
#[rpc_requests(message = SensorMessage)]
#[derive(Debug, Serialize, Deserialize)]
pub enum SensorProtocol {
    #[rpc(tx = oneshot::Sender<Option<Reading>>)]
    GetLatest(GetLatest),
}
