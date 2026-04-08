//! Local network discovery helpers for FastTransfer receivers.

use std::{
    collections::BTreeMap,
    env,
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

/// mDNS service type advertised by FastTransfer receivers.
pub const FASTTRANSFER_SERVICE_TYPE: &str = "_fasttransfer._tcp.local.";
const TXT_KEY_DEVICE_NAME: &str = "device_name";
const TXT_KEY_PEER_ID: &str = "peer_id";
const TXT_KEY_TRANSPORT: &str = "transport";
const TXT_KEY_VERSION: &str = "version";
const DISCOVERY_VERSION: &str = "1";
const DEFAULT_DEVICE_NAME: &str = "FastTransfer Device";

/// Peer advertisement shared over discovery channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAdvertisement {
    /// Stable peer identifier.
    pub peer_id: String,
    /// User-facing device name.
    pub device_name: String,
    /// Reachable addresses published by the peer.
    pub addresses: Vec<String>,
    /// Preferred transport protocol.
    pub transport: String,
}

/// Resolved FastTransfer receiver discovered on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyReceiver {
    /// Stable peer identifier.
    pub peer_id: String,
    /// User-facing device name.
    pub device_name: String,
    /// Reachable receiver socket addresses.
    pub addresses: Vec<SocketAddr>,
    /// Preferred transport protocol.
    pub transport: String,
    /// mDNS service fullname.
    pub service_name: String,
    /// Advertised hostname.
    pub host_name: String,
}

impl NearbyReceiver {
    /// Returns the first reachable address when one is available.
    pub fn primary_address(&self) -> Option<SocketAddr> {
        self.addresses.first().copied()
    }

    /// Converts the resolved receiver into a generic peer advertisement.
    pub fn to_advertisement(&self) -> PeerAdvertisement {
        PeerAdvertisement {
            peer_id: self.peer_id.clone(),
            device_name: self.device_name.clone(),
            addresses: self.addresses.iter().map(SocketAddr::to_string).collect(),
            transport: self.transport.clone(),
        }
    }
}

/// Keeps a receiver mDNS advertisement alive for the lifetime of the process.
pub struct ReceiverAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for ReceiverAdvertiser {
    fn drop(&mut self) {
        if let Ok(receiver) = self.daemon.unregister(&self.fullname) {
            let _ = receiver.recv_timeout(Duration::from_secs(1));
        }

        let _ = self.daemon.shutdown();
    }
}

/// Creates a loopback-only advertisement for local development.
pub fn local_loopback_advertisement(device_name: impl Into<String>) -> PeerAdvertisement {
    PeerAdvertisement {
        peer_id: "local-dev-peer".to_owned(),
        device_name: device_name.into(),
        addresses: vec!["127.0.0.1:7000".to_owned()],
        transport: "quic".to_owned(),
    }
}

/// Returns a best-effort user-facing device name for local discovery.
pub fn default_device_name() -> String {
    env::var("FASTTRANSFER_DEVICE_NAME")
        .ok()
        .or_else(|| env::var("COMPUTERNAME").ok())
        .or_else(|| env::var("HOSTNAME").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DEVICE_NAME.to_owned())
}

/// Starts advertising a receiver on the local network over mDNS.
pub fn advertise_receiver(bind_addr: SocketAddr, device_name: impl Into<String>) -> Result<ReceiverAdvertiser> {
    let daemon = ServiceDaemon::new().context("failed to start the mDNS responder")?;
    let device_name = normalize_device_name(device_name.into());
    let peer_id = default_peer_id(&device_name, bind_addr.port());
    let instance_name = format!("{device_name}-{}", bind_addr.port());
    let host_name = format!("{}.local.", dns_safe_label(&default_host_label()));
    let properties = [
        (TXT_KEY_DEVICE_NAME, device_name.as_str()),
        (TXT_KEY_PEER_ID, peer_id.as_str()),
        (TXT_KEY_TRANSPORT, "quic"),
        (TXT_KEY_VERSION, DISCOVERY_VERSION),
    ];

    let service = ServiceInfo::new(
        FASTTRANSFER_SERVICE_TYPE,
        &instance_name,
        &host_name,
        "",
        bind_addr.port(),
        &properties[..],
    )
    .context("failed to build the FastTransfer mDNS service info")?
    .enable_addr_auto();

    let fullname = service.get_fullname().to_owned();
    daemon
        .register(service)
        .with_context(|| format!("failed to advertise receiver {device_name} over mDNS"))?;

    Ok(ReceiverAdvertiser { daemon, fullname })
}

/// Discovers nearby FastTransfer receivers over mDNS.
pub fn discover_receivers(timeout: Duration) -> Result<Vec<NearbyReceiver>> {
    let daemon = ServiceDaemon::new().context("failed to start the mDNS browser")?;
    let receiver = daemon
        .browse(FASTTRANSFER_SERVICE_TYPE)
        .context("failed to browse for FastTransfer receivers over mDNS")?;
    let deadline = Instant::now() + timeout;
    let mut receivers = BTreeMap::<String, NearbyReceiver>::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait_for = remaining.min(Duration::from_millis(250));
        match receiver.recv_timeout(wait_for) {
            Ok(ServiceEvent::ServiceResolved(service)) => {
                upsert_receiver(&mut receivers, &service);
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    let _ = daemon.stop_browse(FASTTRANSFER_SERVICE_TYPE);
    let _ = daemon.shutdown();

    let mut discovered: Vec<_> = receivers.into_values().collect();
    discovered.sort_by(|left, right| {
        left.device_name
            .cmp(&right.device_name)
            .then_with(|| left.service_name.cmp(&right.service_name))
    });

    Ok(discovered)
}

fn upsert_receiver(receivers: &mut BTreeMap<String, NearbyReceiver>, service: &ResolvedService) {
    let key = service.get_fullname().to_owned();
    let candidate = nearby_receiver_from(service);

    receivers
        .entry(key)
        .and_modify(|existing| merge_receiver(existing, &candidate))
        .or_insert(candidate);
}

fn nearby_receiver_from(service: &ResolvedService) -> NearbyReceiver {
    let mut addresses = resolved_addresses(service);
    addresses.sort_unstable();
    addresses.dedup();

    NearbyReceiver {
        peer_id: property_or_else(service, TXT_KEY_PEER_ID, || service.get_fullname().to_owned()),
        device_name: property_or_else(service, TXT_KEY_DEVICE_NAME, default_device_name),
        addresses,
        transport: property_or_else(service, TXT_KEY_TRANSPORT, || "quic".to_owned()),
        service_name: service.get_fullname().to_owned(),
        host_name: service.get_hostname().to_owned(),
    }
}

fn merge_receiver(existing: &mut NearbyReceiver, incoming: &NearbyReceiver) {
    existing.addresses.extend(incoming.addresses.iter().copied());
    existing.addresses.sort_unstable();
    existing.addresses.dedup();

    if existing.device_name == DEFAULT_DEVICE_NAME {
        existing.device_name = incoming.device_name.clone();
    }

    if existing.transport.is_empty() {
        existing.transport = incoming.transport.clone();
    }
}

fn resolved_addresses(service: &ResolvedService) -> Vec<SocketAddr> {
    service
        .get_addresses()
        .iter()
        .map(|address| SocketAddr::new(address.to_ip_addr(), service.get_port()))
        .collect()
}

fn property_or_else(service: &ResolvedService, key: &str, fallback: impl FnOnce() -> String) -> String {
    service
        .get_property_val_str(key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(fallback)
}

fn normalize_device_name(name: String) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        default_device_name()
    } else {
        trimmed.to_owned()
    }
}

fn default_peer_id(device_name: &str, port: u16) -> String {
    format!("{}-{port}", dns_safe_label(device_name))
}

fn default_host_label() -> String {
    env::var("COMPUTERNAME")
        .ok()
        .or_else(|| env::var("HOSTNAME").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "fasttransfer-host".to_owned())
}

fn dns_safe_label(value: &str) -> String {
    let mut label = String::with_capacity(value.len());
    let mut last_was_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            label.push(ch.to_ascii_lowercase());
            last_was_dash = false;
            continue;
        }

        if !last_was_dash {
            label.push('-');
            last_was_dash = true;
        }
    }

    let trimmed = label.trim_matches('-');
    if trimmed.is_empty() {
        "fasttransfer".to_owned()
    } else {
        trimmed.to_owned()
    }
}
