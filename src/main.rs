// Network adapter derived from 0xdilo/tailscale-esp32's MIT-licensed
// wake-on-lan example, revision dbd49fe. See LICENSE.tailscale-esp32.
mod cloud_client;
mod device;
mod microphone;
mod portal;
mod recorder;
use riker_badge::{valid_approval_url, StatusTracker, STATUS_PORT};
use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use embedded_svc::http::client::Client as HttpClient;

use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::sntp::{EspSntp, SyncStatus};
use esp_idf_svc::tls::{Config as TlsConfiguration, EspTls, InternalSocket};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use log::{info, warn};
use tailscale_esp32::client::ControlConnection;
use tailscale_esp32::control::{
    ControlKeys, HostInfo, MapRequest, MapStreamDecoder, RegisterRequest, RegisterResponse,
    ENDPOINT_TYPE_LOCAL, ENDPOINT_TYPE_PORTMAPPED,
};
use tailscale_esp32::derp::{DerpClient, Event as DerpEvent};
use tailscale_esp32::disco::{open_call_me_maybe, open_ping, seal_ping, seal_pong};
use tailscale_esp32::h2::StreamEvent;
use tailscale_esp32::identity::{DeviceIdentity, ENCODED_IDENTITY_LEN};
use tailscale_esp32::key::{Machine, Node};
use tailscale_esp32::netmap::{icmp_echo_reply_in_place, parse_udp_packet, NetworkMap};
use tailscale_esp32::stun::{binding_request, parse_binding_response};
use tailscale_esp32::wireguard::{HandshakeResponder, WireGuardSession};
use tailscale_esp32::CAPABILITY_VERSION;
const TAILSCALE_HOSTNAME: &str = "riker-badge";
const CONTROL_HOST: &str = "controlplane.tailscale.com";
const CONTROL_KEY_URL: &str = "https://controlplane.tailscale.com/key?v=142";
const TAILSCALE_CONTROL_STACK_SIZE: usize = 40_960;
const TAILSCALE_DATA_STACK_SIZE: usize = 24_576;
const TAILSCALE_DERP_STACK_SIZE: usize = 40_960;
const TAILSCALE_WIREGUARD_PORT: u16 = 41_641;

const STUN_SERVER: &str = "derp1.tailscale.com:3478";
const STUN_INTERVAL: Duration = Duration::from_secs(20);
const CONTROL_RETRY_INTERVAL: Duration = Duration::from_secs(15);
const CONTROL_KEY_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

fn main() -> Result<()> {
    device::run()
}

fn load_or_create_identity(partition: EspNvsPartition<NvsDefault>) -> Result<DeviceIdentity> {
    let nvs = EspNvs::new(partition, "rbidentity", true)
        .context("could not open Tailscale NVS namespace")?;
    let mut encoded = [0_u8; ENCODED_IDENTITY_LEN];
    if let Some(stored) = nvs.get_blob("device", &mut encoded)? {
        let identity = DeviceIdentity::decode(stored).context("stored identity is invalid")?;
        info!("loaded persistent Tailscale device identity");
        return Ok(identity);
    }
    let identity = DeviceIdentity::generate().context("device identity RNG failed")?;
    nvs.set_blob("device", &identity.encode())
        .context("could not persist Tailscale device identity")?;
    info!("created persistent Tailscale device identity");
    Ok(identity)
}

fn host_info(keys: &DeviceIdentity, preferred_derp: u16) -> HostInfo {
    HostInfo::esp32(TAILSCALE_HOSTNAME, hex(keys.backend_log_id()))
        .with_preferred_derp(preferred_derp)
}

fn tailscale_control_loop(
    keys: Arc<DeviceIdentity>,
    network_map: Arc<Mutex<NetworkMap>>,
    endpoints: Arc<Mutex<Vec<(String, u8)>>>,
    endpoint_generation: Arc<AtomicU32>,
    preferred_derp: Arc<AtomicU16>,
) {
    let mut followup = None;
    let mut cached_control_key = None;
    let mut control_key_fetched_at = None;
    let mut map_resume = MapResume::default();
    loop {
        let key_expired = control_key_fetched_at
            .is_none_or(|fetched: Instant| fetched.elapsed() >= CONTROL_KEY_MAX_AGE);
        if cached_control_key.is_none() || key_expired {
            match fetch_control_key() {
                Ok(key) => {
                    cached_control_key = Some(key);
                    control_key_fetched_at = Some(Instant::now());
                }
                Err(error) => {
                    warn!("Tailscale control key refresh failed: {error:#}");
                    std::thread::sleep(CONTROL_RETRY_INTERVAL);
                    continue;
                }
            }
        }
        let control_key = cached_control_key.expect("control key initialized above");
        let context = ControlContext {
            keys: &keys,
            network_map: &network_map,
            endpoints: &endpoints,
            endpoint_generation: &endpoint_generation,
            preferred_derp: &preferred_derp,
        };
        if let Err(error) =
            tailscale_control_session(&context, control_key, &mut followup, &mut map_resume)
        {
            warn!("Tailscale control session failed: {error:#}");
        }
        std::thread::sleep(CONTROL_RETRY_INTERVAL);
    }
}

struct ControlContext<'a> {
    keys: &'a DeviceIdentity,
    network_map: &'a Mutex<NetworkMap>,
    endpoints: &'a Mutex<Vec<(String, u8)>>,
    endpoint_generation: &'a AtomicU32,
    preferred_derp: &'a AtomicU16,
}

fn tailscale_control_session(
    context: &ControlContext<'_>,
    control_key: tailscale_esp32::key::PublicKey<Machine>,
    followup: &mut Option<String>,
    map_resume: &mut MapResume,
) -> Result<()> {
    let stream =
        TcpStream::connect((CONTROL_HOST, 80)).context("could not connect to Tailscale control")?;
    stream.set_read_timeout(Some(Duration::from_secs(130)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let mut control = ControlConnection::upgrade(
        stream,
        CONTROL_HOST,
        context.keys.machine_key(),
        control_key,
        CAPABILITY_VERSION,
    )
    .context("Tailscale control upgrade failed")?;

    let node_key = context.keys.node_key().public();
    let load_balance_key = node_key.to_string();
    let mut registration = RegisterRequest::new(
        CAPABILITY_VERSION,
        node_key,
        context.keys.network_lock_key().public(),
        host_info(context.keys, context.preferred_derp.load(Ordering::Acquire)),
    );
    if let Some(url) = followup.as_ref() {
        info!("waiting for Tailscale approval at: {url}");
        registration = registration.with_followup(url);
    }
    let registration_response: RegisterResponse = control
        .post_json_decode(
            "/machine/register",
            &registration,
            &[load_balance_key.as_str()],
            64 * 1024,
        )
        .context("Tailscale registration failed")?;
    if !registration_response.error.is_empty() {
        bail!(
            "Tailscale rejected registration: {}",
            registration_response.error
        );
    }
    if !registration_response.machine_authorized {
        if registration_response.auth_url.is_empty() {
            bail!("Tailscale registration is pending without an approval URL");
        }
        if !valid_approval_url(&registration_response.auth_url) {
            bail!("rejected unexpected Tailscale approval URL");
        }
        device::set_phase(7);
        *followup = Some(registration_response.auth_url.clone());
        info!(
            "TAILSCALE APPROVAL REQUIRED: {}",
            registration_response.auth_url
        );
        return Ok(());
    }
    *followup = None;

    let advertised_generation = context.endpoint_generation.load(Ordering::Acquire);
    update_network_map(context, node_key, &load_balance_key, &mut control)?;
    stream_network_maps(
        context,
        node_key,
        &load_balance_key,
        &mut control,
        advertised_generation,
        map_resume,
    )
}

#[derive(Default)]
struct MapResume {
    handle: String,
    sequence: i64,
}

fn stream_network_maps(
    context: &ControlContext<'_>,
    node_key: tailscale_esp32::key::PublicKey<Node>,
    load_balance_key: &str,
    control: &mut ControlConnection<TcpStream>,
    advertised_generation: u32,
    resume: &mut MapResume,
) -> Result<()> {
    let request = MapRequest::new(
        CAPABILITY_VERSION,
        node_key,
        context.keys.disco_key().public(),
        host_info(context.keys, context.preferred_derp.load(Ordering::Acquire)),
    )
    .streaming()
    .resume(resume.handle.clone(), resume.sequence);
    let mut stream = control
        .start_json_stream("/machine/map", &request, &[load_balance_key])
        .context("could not start streaming Tailscale map")?;
    let mut decoder = MapStreamDecoder::new(1024 * 1024);
    let mut headers_received = false;
    loop {
        match control
            .read_stream_event(&mut stream)
            .context("streaming Tailscale map failed")?
        {
            StreamEvent::Headers {
                status, end_stream, ..
            } => {
                if status != 200 {
                    bail!("streaming Tailscale map returned HTTP {status}");
                }
                if end_stream {
                    bail!("streaming Tailscale map ended before sending data");
                }
                headers_received = true;
            }
            StreamEvent::Data {
                payload,
                end_stream,
            } => {
                if !headers_received {
                    bail!("streaming Tailscale map sent data before HTTP headers");
                }
                let maps = decoder
                    .push(&payload)
                    .context("could not decode streaming Tailscale map")?;
                let mut current = context
                    .network_map
                    .lock()
                    .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?;
                for map in maps {
                    if !map.map_session_handle.is_empty() {
                        resume.handle.clone_from(&map.map_session_handle);
                    }
                    if map.sequence > 0 {
                        resume.sequence = map.sequence;
                    }
                    current.apply(map);
                }
                drop(current);
                if context.endpoint_generation.load(Ordering::Acquire) != advertised_generation {
                    bail!("Tailscale endpoints changed; reconnecting the map stream");
                }
                if end_stream {
                    bail!("streaming Tailscale map ended");
                }
            }
        }
    }
}

fn update_network_map(
    context: &ControlContext<'_>,
    node_key: tailscale_esp32::key::PublicKey<Node>,
    load_balance_key: &str,
    control: &mut ControlConnection<TcpStream>,
) -> Result<()> {
    let mut map_request = MapRequest::new(
        CAPABILITY_VERSION,
        node_key,
        context.keys.disco_key().public(),
        host_info(context.keys, context.preferred_derp.load(Ordering::Acquire)),
    );
    let endpoints = context
        .endpoints
        .lock()
        .map_err(|_| anyhow::anyhow!("endpoint lock poisoned"))?;
    for (endpoint, kind) in endpoints.iter() {
        map_request.endpoints.push(endpoint.clone());
        map_request.endpoint_types.push(*kind);
    }
    drop(endpoints);
    let response = control
        .post_json(
            "/machine/map",
            &map_request,
            &[load_balance_key],
            1024 * 1024,
        )
        .context("Tailscale map request failed")?;
    if response.status != 200 {
        bail!("Tailscale map request returned HTTP {}", response.status);
    }
    let mut decoder = MapStreamDecoder::new(1024 * 1024);
    let maps = decoder
        .push(&response.body)
        .context("could not decode Tailscale network map")?;
    let first = maps
        .first()
        .context("Tailscale returned an empty network map")?;
    let addresses = first
        .node
        .as_ref()
        .and_then(|node| node.addresses.first())
        .map_or("unknown", String::as_str);
    let peer_count = first.peers.as_ref().map_or(0, Vec::len);
    info!("Tailscale control ready: addresses={addresses}, peers={peer_count}");
    device::set_address(addresses);
    device::set_phase(0);
    let mut current = context
        .network_map
        .lock()
        .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?;
    for map in maps {
        current.apply(map);
    }
    Ok(())
}

struct PeerSession {
    session: WireGuardSession,
    peer_key: tailscale_esp32::key::PublicKey<Node>,
}

struct DataPlaneState {
    sessions: BTreeMap<u32, PeerSession>,

    inner_packet: Vec<u8>,
    outbound_packet: Vec<u8>,
}

struct DirectProbe {
    peer: tailscale_esp32::key::PublicKey<Node>,
    disco_key: tailscale_esp32::key::PublicKey<tailscale_esp32::key::Disco>,
    endpoints: Vec<SocketAddr>,
}

impl DataPlaneState {
    fn new(max_packet_len: usize) -> Self {
        Self {
            sessions: BTreeMap::new(),

            inner_packet: Vec::with_capacity(max_packet_len),
            outbound_packet: Vec::with_capacity(max_packet_len),
        }
    }
}

struct StdTls(EspTls<InternalSocket>);

impl std::io::Read for StdTls {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .read(buffer)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

impl std::io::Write for StdTls {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .write(buffer)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn tailscale_derp_loop(
    keys: Arc<DeviceIdentity>,
    network_map: Arc<Mutex<NetworkMap>>,
    endpoint_generation: Arc<AtomicU32>,
    preferred_derp: Arc<AtomicU16>,
    probe_sender: SyncSender<DirectProbe>,
    status: Arc<Mutex<StatusTracker>>,
) {
    loop {
        if let Err(error) = tailscale_derp_session(
            &keys,
            &network_map,
            &endpoint_generation,
            &preferred_derp,
            &probe_sender,
            &status,
        ) {
            warn!("Tailscale DERP session failed: {error:#}");
        }
        std::thread::sleep(CONTROL_RETRY_INTERVAL);
    }
}

fn tailscale_derp_session(
    keys: &DeviceIdentity,
    network_map: &Mutex<NetworkMap>,
    endpoint_generation: &AtomicU32,
    preferred_derp: &AtomicU16,
    probe_sender: &SyncSender<DirectProbe>,
    status: &Mutex<StatusTracker>,
) -> Result<()> {
    let (region_id, derp_node) = {
        let map = network_map
            .lock()
            .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?;
        let (region_id, node) = map
            .preferred_derp_node()
            .context("Tailscale map does not have a usable DERP node yet")?;
        (region_id, node.clone())
    };
    if preferred_derp.swap(region_id, Ordering::AcqRel) != region_id {
        endpoint_generation.fetch_add(1, Ordering::Release);
    }
    let certificate_name = if derp_node.cert_name.is_empty() {
        derp_node.host_name.as_str()
    } else {
        derp_node.cert_name.as_str()
    };
    let mut tls = EspTls::new().context("could not initialize DERP TLS")?;
    tls.connect(
        &derp_node.host_name,
        derp_node.relay_port(),
        &TlsConfiguration {
            common_name: Some(certificate_name),
            timeout_ms: 130_000,
            use_crt_bundle_attach: true,
            ..Default::default()
        },
    )
    .with_context(|| format!("could not connect to DERP node {}", derp_node.name))?;
    let mut derp = DerpClient::connect(StdTls(tls), &derp_node.host_name, keys.node_key().clone())
        .context("DERP authentication failed")?;
    if !matches!(derp.receive()?, DerpEvent::ServerInfo(_)) {
        bail!("DERP server did not send its authenticated server information");
    }
    derp.note_preferred(true)?;
    info!(
        "Tailscale DERP connected: {} (region {region_id})",
        derp_node.name
    );

    let mut last_timestamps = BTreeMap::<tailscale_esp32::key::PublicKey<Node>, [u8; 12]>::new();
    let mut state = DataPlaneState::new(2048);
    loop {
        let DerpEvent::Packet { source, payload } = derp.receive()? else {
            continue;
        };
        if payload.len() < 4 {
            continue;
        }
        if payload.starts_with(b"TS\xf0\x9f\x92\xac") {
            let Ok(message) = open_call_me_maybe(keys.disco_key(), &payload) else {
                continue;
            };
            let trusted = network_map
                .lock()
                .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?
                .peer_matches_disco(source, message.sender_disco_key);
            if trusted {
                let _ = probe_sender.try_send(DirectProbe {
                    peer: source,
                    disco_key: message.sender_disco_key,
                    endpoints: message.endpoints,
                });
            }
            continue;
        }
        match u32::from_le_bytes(payload[..4].try_into().expect("length checked")) {
            1 => handle_derp_wireguard_handshake(
                &mut derp,
                keys,
                network_map,
                &mut state.sessions,
                &mut last_timestamps,
                source,
                &payload,
            )?,
            4 => {
                if process_wireguard_transport(network_map, status, &mut state, &payload)? {
                    derp.send(source, &state.outbound_packet)?;
                }
            }
            _ => {}
        }
    }
}

fn handle_derp_wireguard_handshake<S: std::io::Read + std::io::Write>(
    derp: &mut DerpClient<S>,
    keys: &DeviceIdentity,
    network_map: &Mutex<NetworkMap>,
    sessions: &mut BTreeMap<u32, PeerSession>,
    last_timestamps: &mut BTreeMap<tailscale_esp32::key::PublicKey<Node>, [u8; 12]>,
    derp_source: tailscale_esp32::key::PublicKey<Node>,
    packet: &[u8],
) -> Result<()> {
    let local_index = random_session_index(sessions)?;
    let Ok(response) = HandshakeResponder::respond(keys.node_key(), packet, local_index) else {
        return Ok(());
    };
    if response.remote_public != derp_source {
        return Ok(());
    }
    let peer_exists = network_map
        .lock()
        .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?
        .contains_peer(response.remote_public);
    if !peer_exists
        || last_timestamps
            .get(&response.remote_public)
            .is_some_and(|last| *last >= response.timestamp)
    {
        return Ok(());
    }
    last_timestamps.insert(response.remote_public, response.timestamp);
    derp.send(derp_source, &response.packet)?;
    sessions.insert(
        local_index,
        PeerSession {
            session: response.session,
            peer_key: response.remote_public,
        },
    );
    while sessions.len() > 16 {
        let Some(oldest) = sessions.keys().next().copied() else {
            break;
        };
        sessions.remove(&oldest);
    }
    info!("Tailscale WireGuard session established over DERP");
    Ok(())
}

fn tailscale_data_loop(
    keys: Arc<DeviceIdentity>,
    network_map: Arc<Mutex<NetworkMap>>,
    endpoints: Arc<Mutex<Vec<(String, u8)>>>,
    endpoint_generation: Arc<AtomicU32>,
    probe_receiver: Receiver<DirectProbe>,
    status: Arc<Mutex<StatusTracker>>,
) {
    loop {
        if let Err(error) = tailscale_data(
            keys.clone(),
            network_map.clone(),
            endpoints.clone(),
            endpoint_generation.clone(),
            &probe_receiver,
            status.clone(),
        ) {
            warn!("Tailscale data session failed: {error:#}");
        }
        std::thread::sleep(CONTROL_RETRY_INTERVAL);
    }
}

fn tailscale_data(
    keys: Arc<DeviceIdentity>,
    network_map: Arc<Mutex<NetworkMap>>,
    endpoints: Arc<Mutex<Vec<(String, u8)>>>,
    endpoint_generation: Arc<AtomicU32>,
    probe_receiver: &Receiver<DirectProbe>,
    status: Arc<Mutex<StatusTracker>>,
) -> Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", TAILSCALE_WIREGUARD_PORT))
        .context("could not bind Tailscale UDP socket")?;
    let mut last_timestamps = BTreeMap::<tailscale_esp32::key::PublicKey<Node>, [u8; 12]>::new();
    let mut packet = [0_u8; 2048];
    let mut state = DataPlaneState::new(packet.len());
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let stun_server = STUN_SERVER
        .to_socket_addrs()
        .context("could not resolve Tailscale STUN server")?
        .find(SocketAddr::is_ipv4)
        .context("Tailscale STUN server has no IPv4 address")?;
    let mut stun_transaction = [0_u8; 12];
    let mut last_stun = Instant::now()
        .checked_sub(STUN_INTERVAL)
        .unwrap_or_else(Instant::now);
    info!("Tailscale data plane listening on UDP {TAILSCALE_WIREGUARD_PORT}");

    loop {
        while let Ok(probe) = probe_receiver.try_recv() {
            for destination in probe.endpoints {
                let mut transaction_id = [0_u8; 12];
                getrandom::getrandom(&mut transaction_id)
                    .map_err(|error| anyhow::anyhow!("DISCO transaction RNG failed: {error}"))?;
                let packet = seal_ping(
                    keys.disco_key(),
                    probe.disco_key,
                    transaction_id,
                    keys.node_key().public(),
                )?;
                if destination.is_ipv4() {
                    if let Err(error) = socket.send_to(&packet, destination) {
                        warn!("could not send direct-path probe to {destination}: {error}");
                    }
                }
            }
            info!("sent direct-path probes requested by {:?}", probe.peer);
        }
        if last_stun.elapsed() >= STUN_INTERVAL {
            getrandom::getrandom(&mut stun_transaction)
                .map_err(|error| anyhow::anyhow!("STUN transaction RNG failed: {error}"))?;
            socket.send_to(&binding_request(stun_transaction), stun_server)?;
            last_stun = Instant::now();
        }
        let (length, source) = match socket.recv_from(&mut packet) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("Tailscale UDP receive failed"),
        };
        let packet = &packet[..length];
        if source.port() == stun_server.port() {
            if let Ok(public_endpoint) = parse_binding_response(packet, stun_transaction) {
                let public_endpoint = public_endpoint.to_string();
                let mut current = endpoints
                    .lock()
                    .map_err(|_| anyhow::anyhow!("endpoint lock poisoned"))?;
                let changed = current
                    .iter()
                    .find(|(_, kind)| *kind == ENDPOINT_TYPE_PORTMAPPED)
                    .is_none_or(|(endpoint, _)| endpoint != &public_endpoint);
                current.retain(|(_, kind)| *kind != ENDPOINT_TYPE_PORTMAPPED);
                // This socket continuously refreshes an endpoint-independent NAT
                // mapping. Advertising it as port-mapped keeps the endpoint in
                // peer maps; ordinary STUN endpoints are intentionally pruned by
                // control because full Tailscale exchanges them over DERP.
                current.push((public_endpoint.clone(), ENDPOINT_TYPE_PORTMAPPED));
                if changed {
                    endpoint_generation.fetch_add(1, Ordering::Release);
                    info!("Tailscale STUN endpoint discovered: {public_endpoint}");
                }
                continue;
            }
        }
        if packet.starts_with(b"TS\xf0\x9f\x92\xac") {
            handle_disco(&socket, &keys, &network_map, packet, source)?;
            continue;
        }
        if packet.len() < 4 {
            continue;
        }
        match u32::from_le_bytes(packet[..4].try_into().expect("length checked")) {
            1 => handle_wireguard_handshake(
                &socket,
                &keys,
                &network_map,
                &mut state.sessions,
                &mut last_timestamps,
                packet,
                source,
            )?,
            4 => {
                if process_wireguard_transport(&network_map, &status, &mut state, packet)? {
                    socket.send_to(&state.outbound_packet, source)?;
                }
            }
            _ => {}
        }
    }
}

fn handle_disco(
    socket: &UdpSocket,
    keys: &DeviceIdentity,
    network_map: &Mutex<NetworkMap>,
    packet: &[u8],
    source: SocketAddr,
) -> Result<()> {
    let Ok(ping) = open_ping(keys.disco_key(), packet) else {
        return Ok(());
    };
    let Some(node_key) = ping.node_key else {
        return Ok(());
    };
    let allowed = network_map
        .lock()
        .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?
        .peer_matches_disco(node_key, ping.sender_disco_key);
    if !allowed {
        return Ok(());
    }
    let pong = seal_pong(
        keys.disco_key(),
        ping.sender_disco_key,
        ping.transaction_id,
        source,
    )?;
    socket.send_to(&pong, source)?;
    Ok(())
}

fn handle_wireguard_handshake(
    socket: &UdpSocket,
    keys: &DeviceIdentity,
    network_map: &Mutex<NetworkMap>,
    sessions: &mut BTreeMap<u32, PeerSession>,
    last_timestamps: &mut BTreeMap<tailscale_esp32::key::PublicKey<Node>, [u8; 12]>,
    packet: &[u8],
    source: SocketAddr,
) -> Result<()> {
    let local_index = random_session_index(sessions)?;
    let Ok(response) = HandshakeResponder::respond(keys.node_key(), packet, local_index) else {
        return Ok(());
    };
    let peer_exists = network_map
        .lock()
        .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?
        .contains_peer(response.remote_public);
    if !peer_exists {
        return Ok(());
    }
    if last_timestamps
        .get(&response.remote_public)
        .is_some_and(|last| *last >= response.timestamp)
    {
        return Ok(());
    }
    last_timestamps.insert(response.remote_public, response.timestamp);
    socket.send_to(&response.packet, source)?;
    sessions.insert(
        local_index,
        PeerSession {
            session: response.session,
            peer_key: response.remote_public,
        },
    );
    while sessions.len() > 16 {
        let Some(oldest) = sessions.keys().next().copied() else {
            break;
        };
        sessions.remove(&oldest);
    }
    info!("Tailscale WireGuard session established");
    Ok(())
}

fn process_wireguard_transport(
    network_map: &Mutex<NetworkMap>,
    status: &Mutex<StatusTracker>,
    state: &mut DataPlaneState,
    packet: &[u8],
) -> Result<bool> {
    if packet.len() < 8 {
        return Ok(false);
    }
    let receiver = u32::from_le_bytes(packet[4..8].try_into().expect("length checked"));
    let Some(peer_session) = state.sessions.get_mut(&receiver) else {
        return Ok(false);
    };
    if peer_session
        .session
        .decrypt_into(packet, &mut state.inner_packet)
        .is_err()
    {
        return Ok(false);
    }
    if let Ok(echo) = icmp_echo_reply_in_place(&mut state.inner_packet) {
        let map = network_map
            .lock()
            .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?;
        let route_allowed = map.peer_allows_source(peer_session.peer_key, echo.source);
        let acl_allowed = map.allows(echo.source, echo.destination, 1, 0);
        drop(map);
        if !route_allowed || !acl_allowed {
            return Ok(false);
        }

        peer_session.session.encrypt_into(
            &state.inner_packet[..echo.packet_len],
            &mut state.outbound_packet,
        )?;
        return Ok(true);
    }
    let Ok(udp) = parse_udp_packet(&state.inner_packet) else {
        return Ok(false);
    };
    if udp.destination_port != STATUS_PORT || Some(udp.source) != device::allowed_sender() {
        return Ok(false);
    }

    let map = network_map
        .lock()
        .map_err(|_| anyhow::anyhow!("network map lock poisoned"))?;
    let route_allowed = map.peer_allows_source(peer_session.peer_key, udp.source);
    let acl_allowed = map.allows(udp.source, udp.destination, 17, udp.destination_port);
    if !route_allowed || !acl_allowed {
        return Ok(false);
    }
    drop(map);

    // WireGuard anti-replay, peer/source binding and the tailnet packet filter
    // have already succeeded. Only the fixed public status vocabulary is accepted.
    if status
        .lock()
        .map_err(|_| anyhow::anyhow!("status lock poisoned"))?
        .receive(udp.payload, device::elapsed_ms())
    {
        info!("accepted authenticated badge status");
    }
    Ok(false)
}

fn random_session_index(sessions: &BTreeMap<u32, PeerSession>) -> Result<u32> {
    loop {
        let mut bytes = [0_u8; 4];
        getrandom::getrandom(&mut bytes)
            .map_err(|error| anyhow::anyhow!("session index RNG failed: {error}"))?;
        let index = u32::from_le_bytes(bytes);
        if index != 0 && !sessions.contains_key(&index) {
            return Ok(index);
        }
    }
}

fn fetch_control_key() -> Result<tailscale_esp32::key::PublicKey<Machine>> {
    let configuration = HttpConfiguration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(Duration::from_secs(15)),
        ..Default::default()
    };
    let mut client = HttpClient::wrap(
        EspHttpConnection::new(&configuration).context("HTTPS client initialization failed")?,
    );
    let request = client
        .get(CONTROL_KEY_URL)
        .context("control key HTTPS request failed")?;
    let mut response = request
        .submit()
        .context("control key HTTPS response failed")?;
    if response.status() != 200 {
        bail!("control key endpoint returned HTTP {}", response.status());
    }
    let mut body = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        let read = response
            .read(&mut chunk)
            .context("could not read control key response")?;
        if read == 0 {
            break;
        }
        if body.len() + read > 64 * 1024 {
            bail!("control key response exceeded 64 KiB");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Ok(serde_json::from_slice::<ControlKeys>(&body)
        .context("invalid control key response")?
        .public_key)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
