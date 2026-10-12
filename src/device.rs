use super::*;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use riker_badge::Status;
use std::ffi::{c_char, CString};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU8};

static PHASE: AtomicU8 = AtomicU8::new(5);
static TIME_READY: AtomicBool = AtomicBool::new(false);
static TEST_START_FAILURE: AtomicBool = AtomicBool::new(false);
pub fn request_start_failure_test() {
    TEST_START_FAILURE.store(true, Ordering::Release);
}
pub fn network_ready() -> bool {
    TIME_READY.load(Ordering::Acquire) && crate::portal::connected()
}
static ADDRESS: Mutex<String> = Mutex::new(String::new());
static SENDER: std::sync::OnceLock<std::net::IpAddr> = std::sync::OnceLock::new();
pub fn allowed_sender() -> Option<std::net::IpAddr> {
    SENDER.get().copied()
}

extern "C" {
    fn badge_begin() -> i32;
    fn badge_update(state: u8, detail: *const c_char, elapsed_ms: u32);
}

pub fn elapsed_ms() -> u64 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}
pub fn set_phase(state: u8) {
    PHASE.store(state, Ordering::Release);
}
pub fn set_address(address: &str) {
    if let Ok(mut value) = ADDRESS.lock() {
        *value = address.chars().take(48).collect();
    }
}

pub fn run() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    if unsafe { badge_begin() } != 0 {
        bail!("StopWatch display initialization failed");
    }
    let status = Arc::new(Mutex::new(StatusTracker::new()));
    std::thread::Builder::new()
        .name("badge-network".into())
        .stack_size(16_384)
        .spawn({
            let status = status.clone();
            move || {
                if let Err(error) = start_network(status) {
                    warn!("badge startup stopped safely: {error:#}");
                    set_phase(5);
                    set_address("Setup stopped; see USB");
                }
            }
        })?;
    loop {
        let now = elapsed_ms();
        let current = status
            .lock()
            .map_err(|_| anyhow::anyhow!("status lock poisoned"))?
            .current(now);
        let state = if current == Status::Offline {
            PHASE.load(Ordering::Acquire)
        } else {
            current as u8
        };
        let detail = ADDRESS
            .lock()
            .map_err(|_| anyhow::anyhow!("address lock poisoned"))?
            .clone();
        let detail = CString::new(detail).unwrap_or_default();
        crate::portal::update_display();
        crate::recorder::update_display();
        unsafe {
            badge_update(state, detail.as_ptr(), now as u32);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start_network(status: Arc<Mutex<StatusTracker>>) -> Result<()> {
    let peripherals = Peripherals::take().context("peripherals already taken")?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs =
        EspDefaultNvsPartition::take_with(false).context("NVS unavailable; refusing to erase")?;
    let settings = EspNvs::new(nvs.clone(), "rikerbadge", true)?;
    let saved_wifi = crate::portal::load(&settings)?;
    if let Err(error) = crate::recorder::start(nvs.clone()) {
        warn!("Recorder unavailable; keeping Wi-Fi setup available: {error:#}");
    }
    let mut peer_buf = [0u8; 40];
    if let Some(peer) = settings.get_str("peer", &mut peer_buf)? {
        if let Ok(ip) = peer.parse::<std::net::Ipv4Addr>() {
            let octets = ip.octets();
            if octets[0] == 100 && (64..=127).contains(&octets[1]) {
                let _ = SENDER.set(ip.into());
            }
        }
    }
    // Missing peer configuration fails closed for Riker; it does not block phone Wi-Fi setup.
    if let Err(error) = crate::microphone::start_console() {
        warn!("mic test console unavailable: {error:#}");
    }
    // esp-idf-svc's default AP subnet is 192.168.71.0 with public DNS.
    // Pin the intended captive subnet and advertise this device as the DNS server.
    use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
    let mut ap_config = NetifConfiguration::wifi_default_router();
    if let Some(embedded_svc::ipv4::Configuration::Router(router)) =
        ap_config.ip_configuration.as_mut()
    {
        router.subnet.gateway = crate::portal::AP;
        router.dns = Some(crate::portal::AP);
        router.secondary_dns = None;
    }
    let driver =
        esp_idf_svc::wifi::WifiDriver::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))?;
    let wifi = BlockingWifi::wrap(
        EspWifi::wrap_all(
            driver,
            EspNetif::new(NetifStack::Sta)?,
            EspNetif::new_with_conf(&ap_config)?,
        )?,
        sys_loop,
    )?;
    let storage_result = unsafe {
        esp_idf_svc::sys::esp_wifi_set_storage(esp_idf_svc::sys::wifi_storage_t_WIFI_STORAGE_RAM)
    };
    if storage_result != esp_idf_svc::sys::ESP_OK {
        bail!("could not select RAM Wi-Fi configuration");
    }
    set_phase(if saved_wifi.is_some() { 6 } else { 5 });
    set_address("Both buttons: Wi-Fi setup");
    let fallback = crate::portal::previous_wifi(nvs.clone(), saved_wifi.as_ref());
    log::info!(
        "Previously saved Wi-Fi fallback available: {}",
        fallback.is_some()
    );
    let mut manager = crate::portal::Manager::new(wifi, settings, saved_wifi, fallback)?;
    let endpoints = Arc::new(Mutex::new(Vec::<(String, u8)>::new()));
    let endpoint_generation = Arc::new(AtomicU32::new(0));
    let mut sntp = None;
    let mut clock_attempt = None::<Instant>;
    let mut riker_attempted = false;
    // The owner of the radio/setup server never exits for optional-service errors.
    loop {
        if let Err(error) = manager.tick() {
            warn!("Wi-Fi manager: {error:#}");
        }
        refresh_endpoint(&manager, &endpoints, &endpoint_generation);
        if manager.ip().is_some()
            && sntp.is_none()
            && clock_attempt.is_none_or(|last| last.elapsed() > Duration::from_secs(5))
        {
            clock_attempt = Some(Instant::now());
            match EspSntp::new_default() {
                Ok(clock) => sntp = Some(clock),
                Err(error) => {
                    warn!("Clock startup failed; Wi-Fi setup remains available: {error:#}")
                }
            }
        }
        if sntp
            .as_ref()
            .is_some_and(|clock| clock.get_sync_status() == SyncStatus::Completed)
            && !TIME_READY.swap(true, Ordering::AcqRel)
        {
            println!("WIFI_READY");
        }
        let test_failure = TEST_START_FAILURE.swap(false, Ordering::AcqRel);
        if test_failure || (!riker_attempted && network_ready() && !crate::portal::active()) {
            riker_attempted = true;
            let result = if test_failure {
                Err(anyhow::anyhow!(
                    "RIKER_START_TEST_FAILURE: simulated allocation failure"
                ))
            } else {
                set_phase(6);
                set_address("Contacting Tailscale");
                start_tailscale(
                    nvs.clone(),
                    status.clone(),
                    endpoints.clone(),
                    endpoint_generation.clone(),
                )
            };
            if let Err(error) = result {
                warn!("Riker startup failed; Wi-Fi controller retained: {error:#}");
                set_phase(0);
                set_address("Riker off; Wi-Fi setup available");
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn start_tailscale(
    nvs: EspDefaultNvsPartition,
    status: Arc<Mutex<StatusTracker>>,
    endpoints: Arc<Mutex<Vec<(String, u8)>>>,
    endpoint_generation: Arc<AtomicU32>,
) -> Result<()> {
    let keys = Arc::new(load_or_create_identity(nvs)?);
    let network_map = Arc::new(Mutex::new(NetworkMap::default()));
    let preferred_derp = Arc::new(AtomicU16::new(0));
    let (probe_sender, probe_receiver) = sync_channel(8);
    let control: riker_badge::workers::Work = {
        let (keys, map, endpoints, generation, derp) = (
            keys.clone(),
            network_map.clone(),
            endpoints.clone(),
            endpoint_generation.clone(),
            preferred_derp.clone(),
        );
        Box::new(move || tailscale_control_loop(keys, map, endpoints, generation, derp))
    };
    let data: riker_badge::workers::Work = {
        let (keys, map, endpoints, generation, status) = (
            keys.clone(),
            network_map.clone(),
            endpoints.clone(),
            endpoint_generation.clone(),
            status.clone(),
        );
        Box::new(move || {
            tailscale_data_loop(keys, map, endpoints, generation, probe_receiver, status)
        })
    };
    let derp: riker_badge::workers::Work = Box::new(move || {
        tailscale_derp_loop(
            keys,
            network_map,
            endpoint_generation,
            preferred_derp,
            probe_sender,
            status,
        )
    });
    riker_badge::workers::start_workers(vec![
        ("tailscale-control", TAILSCALE_CONTROL_STACK_SIZE, control),
        ("tailscale-data", TAILSCALE_DATA_STACK_SIZE, data),
        ("tailscale-derp", TAILSCALE_DERP_STACK_SIZE, derp),
    ])?;
    Ok(())
}

fn refresh_endpoint(
    manager: &crate::portal::Manager,
    endpoints: &Mutex<Vec<(String, u8)>>,
    generation: &AtomicU32,
) {
    if let Some(ip) = manager.ip() {
        let endpoint = format!("{ip}:{TAILSCALE_WIREGUARD_PORT}");
        if let Ok(mut current) = endpoints.lock() {
            if current
                .iter()
                .find(|(_, kind)| *kind == ENDPOINT_TYPE_LOCAL)
                .is_none_or(|(old, _)| old != &endpoint)
            {
                current.retain(|(_, kind)| *kind != ENDPOINT_TYPE_LOCAL);
                current.push((endpoint, ENDPOINT_TYPE_LOCAL));
                generation.fetch_add(1, Ordering::Release);
            }
        }
    }
}

pub(super) fn read_line(prompt: &str, max: usize) -> Result<String> {
    if !prompt.is_empty() {
        println!("{prompt}");
        std::io::stdout().flush()?;
    }
    let mut value = Vec::with_capacity(max);
    let mut byte = [0u8; 1];
    let mut overflow = false;
    loop {
        match std::io::stdin().read(&mut byte) {
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Ok(_) => match byte[0] {
                b'\n' => break,
                b'\r' => continue,
                0 => {
                    overflow = true;
                }
                b if value.len() < max => value.push(b),
                _ => overflow = true,
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e.into()),
        }
    }
    if overflow {
        bail!("provisioning input exceeds allowed length or contains NUL");
    }
    String::from_utf8(value).context("provisioning input must be UTF-8")
}
