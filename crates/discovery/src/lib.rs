//! Local network discovery helpers for FastTransfer receivers.

use std::{
    collections::BTreeMap,
    env,
    fs,
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use integrity::{format_sha256, sha256_bytes};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

/// mDNS service type advertised by FastTransfer receivers.
pub const FASTTRANSFER_SERVICE_TYPE: &str = "_fasttransfer._tcp.local.";
const TXT_KEY_DEVICE_NAME: &str = "device_name";
const TXT_KEY_PEER_ID: &str = "peer_id";
const TXT_KEY_TRANSPORT: &str = "transport";
const TXT_KEY_VERSION: &str = "version";
const TXT_KEY_CERT_SHA256: &str = "cert_sha256";
const TXT_KEY_CERT_PARTS: &str = "cert_parts";
const TXT_KEY_CERT_PART_PREFIX: &str = "cert";
const DISCOVERY_VERSION: &str = "2";
const DEFAULT_DEVICE_NAME: &str = "FastTransfer Device";
const CERT_PART_VALUE_LEN: usize = 200;
const SHORT_FINGERPRINT_LEN: usize = 12;

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
    /// DER-encoded receiver certificate advertised over LAN discovery.
    pub certificate_der: Vec<u8>,
    /// SHA-256 fingerprint for the advertised receiver certificate.
    pub certificate_sha256_hex: String,
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

    /// Returns a short human-readable fingerprint for UI display.
    pub fn short_fingerprint(&self) -> String {
        short_fingerprint(&self.certificate_sha256_hex)
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

/// Returns a short display fingerprint from a full certificate fingerprint.
pub fn short_fingerprint(fingerprint: &str) -> String {
    fingerprint.chars().take(SHORT_FINGERPRINT_LEN).collect()
}

/// Starts advertising a receiver on the local network over mDNS.
pub fn advertise_receiver(
    bind_addr: SocketAddr,
    device_name: impl Into<String>,
    cert_path: &Path,
) -> Result<ReceiverAdvertiser> {
    let daemon = ServiceDaemon::new().context("failed to start the mDNS responder")?;
    let device_name = normalize_device_name(device_name.into());
    let peer_id = default_peer_id(&device_name, bind_addr.port());
    let instance_name = format!("{device_name}-{}", bind_addr.port());
    let host_name = format!("{}.local.", dns_safe_label(&default_host_label()));
    let cert_bytes = fs::read(cert_path)
        .with_context(|| format!("failed to read receiver certificate at {}", cert_path.display()))?;
    let cert_sha256_hex = format_sha256(&sha256_bytes(&cert_bytes));
    let cert_b64 = BASE64_STANDARD.encode(cert_bytes);
    let cert_parts = split_string(&cert_b64, CERT_PART_VALUE_LEN);
    let mut properties = vec![
        (TXT_KEY_DEVICE_NAME.to_owned(), device_name.clone()),
        (TXT_KEY_PEER_ID.to_owned(), peer_id),
        (TXT_KEY_TRANSPORT.to_owned(), "quic".to_owned()),
        (TXT_KEY_VERSION.to_owned(), DISCOVERY_VERSION.to_owned()),
        (TXT_KEY_CERT_SHA256.to_owned(), cert_sha256_hex),
        (TXT_KEY_CERT_PARTS.to_owned(), cert_parts.len().to_string()),
    ];

    for (index, part) in cert_parts.into_iter().enumerate() {
        properties.push((format!("{TXT_KEY_CERT_PART_PREFIX}{index}"), part));
    }

    let service = ServiceInfo::new(
        FASTTRANSFER_SERVICE_TYPE,
        &instance_name,
        &host_name,
        "",
        bind_addr.port(),
        properties.as_slice(),
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
                if let Ok(candidate) = nearby_receiver_from(&service) {
                    upsert_receiver(&mut receivers, candidate);
                }
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

fn upsert_receiver(receivers: &mut BTreeMap<String, NearbyReceiver>, candidate: NearbyReceiver) {
    let key = candidate.service_name.clone();
    receivers
        .entry(key)
        .and_modify(|existing| merge_receiver(existing, &candidate))
        .or_insert(candidate);
}

fn nearby_receiver_from(service: &ResolvedService) -> Result<NearbyReceiver> {
    let mut addresses = resolved_addresses(service);
    addresses.sort_unstable();
    addresses.dedup();

    let certificate_der = decode_certificate(service)?;
    let advertised_fingerprint = property_or_else(service, TXT_KEY_CERT_SHA256, String::new);
    if advertised_fingerprint.is_empty() {
        bail!("discovered receiver {} did not advertise a certificate fingerprint", service.get_fullname());
    }
    let actual_fingerprint = format_sha256(&sha256_bytes(&certificate_der));
    if actual_fingerprint != advertised_fingerprint {
        bail!(
            "discovered receiver {} advertised fingerprint {} but certificate decoded to {}",
            service.get_fullname(),
            advertised_fingerprint,
            actual_fingerprint
        );
    }

    Ok(NearbyReceiver {
        peer_id: property_or_else(service, TXT_KEY_PEER_ID, || service.get_fullname().to_owned()),
        device_name: property_or_else(service, TXT_KEY_DEVICE_NAME, default_device_name),
        addresses,
        transport: property_or_else(service, TXT_KEY_TRANSPORT, || "quic".to_owned()),
        service_name: service.get_fullname().to_owned(),
        host_name: service.get_hostname().to_owned(),
        certificate_der,
        certificate_sha256_hex: advertised_fingerprint,
    })
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

    if existing.certificate_der.is_empty() && !incoming.certificate_der.is_empty() {
        existing.certificate_der = incoming.certificate_der.clone();
        existing.certificate_sha256_hex = incoming.certificate_sha256_hex.clone();
    }
}

fn decode_certificate(service: &ResolvedService) -> Result<Vec<u8>> {
    let part_count = property_or_else(service, TXT_KEY_CERT_PARTS, String::new)
        .parse::<usize>()
        .context("failed to parse mDNS certificate part count")?;
    if part_count == 0 {
        bail!("discovered receiver {} advertised zero certificate parts", service.get_fullname());
    }

    let mut encoded = String::new();
    for index in 0..part_count {
        let key = format!("{TXT_KEY_CERT_PART_PREFIX}{index}");
        let part = property_or_else(service, &key, String::new);
        if part.is_empty() {
            bail!("discovered receiver {} is missing certificate part {index}", service.get_fullname());
        }
        encoded.push_str(&part);
    }

    BASE64_STANDARD
        .decode(encoded)
        .context("failed to decode advertised receiver certificate")
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

fn split_string(value: &str, chunk_len: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let end = (start + chunk_len).min(value.len());
        parts.push(value[start..end].to_owned());
        start = end;
    }
    parts
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
