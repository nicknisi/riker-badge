//! Temporary password-protected setup AP. HTTP requests are checked against
//! BOTH local and peer socket addresses, never Host headers. Credentials stay local.
use anyhow::{bail, Context, Result};
use embedded_svc::http::Method;
use embedded_svc::io::Write;
use embedded_svc::wifi::{
    AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration,
};
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpConnection, EspHttpServer};
use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use riker_badge::provisioning::{ap_address, dns_reply, parse_form, Credentials};
use std::ffi::{c_char, CString};
use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

pub const AP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
static ACTION: AtomicU8 = AtomicU8::new(0);
static UI_ACTION: AtomicU8 = AtomicU8::new(0);
pub fn request_open_via_display() {
    UI_ACTION.store(1, Ordering::Release);
}
pub fn request_close_via_display() {
    UI_ACTION.store(2, Ordering::Release);
}
static STATION_IP: AtomicU32 = AtomicU32::new(0);
#[derive(Clone)]
struct View {
    phase: u8,
    qr: Zeroizing<String>,
    message: String,
    accepting: bool,
}
static VIEW: OnceLock<Mutex<View>> = OnceLock::new();
fn view() -> &'static Mutex<View> {
    VIEW.get_or_init(|| {
        Mutex::new(View {
            phase: 0,
            qr: Zeroizing::new(String::new()),
            message: String::new(),
            accepting: false,
        })
    })
}
extern "C" {
    fn badge_wifi_set(phase: u8, qr: *const c_char, message: *const c_char);
    fn badge_wifi_take_action() -> u8;
    fn badge_try_wifi() -> i32;
    fn badge_end_wifi();
}
pub fn connected() -> bool {
    STATION_IP.load(Ordering::Acquire) != 0
}
pub fn active() -> bool {
    view().lock().map(|v| v.phase != 0).unwrap_or(true)
}
pub fn request_open() -> bool {
    if unsafe { badge_try_wifi() } == 0 {
        return false;
    }
    let mut v = view().lock().unwrap();
    v.phase = 1;
    v.message = "Starting setup hotspot...".into();
    ACTION.store(1, Ordering::Release);
    true
}
pub fn request_close() {
    ACTION.store(2, Ordering::Release);
}
pub fn update_display() {
    let action = unsafe { badge_wifi_take_action() };
    let queued = UI_ACTION.swap(0, Ordering::AcqRel);
    let action = if queued != 0 { queued } else { action };
    if action == 1 {
        request_open();
    } else if action == 2 {
        request_close();
    }
    let v = view().lock().unwrap().clone();
    let qr = CString::new(v.qr.as_bytes()).unwrap_or_default();
    let message = CString::new(v.message).unwrap_or_default();
    unsafe {
        badge_wifi_set(v.phase, qr.as_ptr(), message.as_ptr());
    }
}
fn message(text: &str, accepting: bool) {
    let mut v = view().lock().unwrap();
    v.message = text.into();
    v.accepting = accepting;
}
fn hide() {
    let mut v = view().lock().unwrap();
    v.phase = 0;
    v.qr.zeroize();
    v.message.clear();
    v.accepting = false;
}

pub fn load(settings: &EspNvs<NvsDefault>) -> Result<Option<Credentials>> {
    let mut buffer = Zeroizing::new([0u8; 384]);
    if let Some(text) = settings.get_str("wifi_v1", &mut *buffer)? {
        let (ssid, password): (String, String) =
            serde_json::from_str(text).context("saved Wi-Fi config invalid")?;
        return Ok(Some(
            Credentials::new(ssid, password).map_err(anyhow::Error::msg)?,
        ));
    }
    let mut ssid = [0u8; 33];
    let mut password = Zeroizing::new([0u8; 64]);
    let ssid = settings.get_str("ssid", &mut ssid)?.map(str::to_owned);
    let password = settings.get_str("pass", &mut *password)?.map(str::to_owned);
    Ok(match (ssid, password) {
        (Some(s), Some(p)) => Some(Credentials::new(s, p).map_err(anyhow::Error::msg)?),
        _ => None,
    })
}
fn station(credentials: Option<&Credentials>) -> Result<ClientConfiguration> {
    let Some(c) = credentials else {
        return Ok(ClientConfiguration::default());
    };
    Ok(ClientConfiguration {
        ssid: c
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: c
            .password
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method: if c.password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    })
}
fn apply(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    credentials: Option<&Credentials>,
    ap: Option<&AccessPointConfiguration>,
) -> Result<()> {
    let target = station(credentials)?;
    let same = wifi
        .get_configuration()
        .ok()
        .and_then(|c| c.as_client_conf_ref().cloned())
        .is_some_and(|c| c == target);
    if !same {
        let _ = wifi.wifi_mut().disconnect();
    }
    let config = match ap {
        Some(ap) => Configuration::Mixed(target, ap.clone()),
        None => Configuration::Client(target),
    };
    wifi.set_configuration(&config)?;
    if !wifi.is_started()? {
        wifi.start()?;
    }
    if credentials.is_some() && !wifi.is_connected()? {
        // A connection may already be in progress; the manager retries asynchronously.
        let _ = wifi.wifi_mut().connect();
    }
    Ok(())
}
pub fn previous_wifi(
    partition: esp_idf_svc::nvs::EspNvsPartition<NvsDefault>,
    current: Option<&Credentials>,
) -> Option<Credentials> {
    // Read only the original firmware's Wi-Fi key on this same badge. Never read its auth/profile tokens.
    let original = EspNvs::new(partition, "devices", false).ok()?;
    let mut buffer = Zeroizing::new([0u8; 384]);
    let text = original.get_str("wifi", &mut *buffer).ok()??;
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let candidate = Credentials::new(
        value.get("ssid")?.as_str()?.to_owned(),
        value.get("password")?.as_str()?.to_owned(),
    )
    .ok()?;
    if candidate.password.is_empty()
        || current.is_some_and(|c| {
            c.ssid == candidate.ssid && c.password.as_str() == candidate.password.as_str()
        })
    {
        return None;
    }
    Some(candidate)
}

pub struct Manager {
    fallback: Option<Credentials>,
    switch_at: Instant,
    wifi: BlockingWifi<EspWifi<'static>>,
    settings: EspNvs<NvsDefault>,
    current: Option<Credentials>,
    session: Option<Session>,
    trial: Option<(Credentials, Instant)>,
    reconnect_at: Instant,
}
impl Manager {
    pub fn new(
        wifi: BlockingWifi<EspWifi<'static>>,
        settings: EspNvs<NvsDefault>,
        mut current: Option<Credentials>,
        mut fallback: Option<Credentials>,
    ) -> Result<Self> {
        if current.is_none() {
            current = fallback.take();
        }
        let mut this = Self {
            fallback,
            switch_at: Instant::now(),
            wifi,
            settings,
            current,
            session: None,
            trial: None,
            reconnect_at: Instant::now(),
        };
        // Start STA even without credentials: radio entropy is available before
        // minting setup credentials. Never start an open AP as an intermediate step.
        apply(&mut this.wifi, this.current.as_ref(), None)?;
        Ok(this)
    }
    pub fn ip(&self) -> Option<Ipv4Addr> {
        if !self.wifi.is_connected().unwrap_or(false)
            || !self.wifi.wifi().sta_netif().is_up().unwrap_or(false)
        {
            return None;
        }
        self.wifi
            .wifi()
            .sta_netif()
            .get_ip_info()
            .ok()
            .map(|i| i.ip)
            .filter(|ip| !ip.is_unspecified())
    }
    pub fn tick(&mut self) -> Result<()> {
        match ACTION.swap(0, Ordering::AcqRel) {
            1 if self.session.is_none() => {
                if let Err(error) = self.open() {
                    self.close()?;
                    crate::device::set_address("Setup unavailable; try again");
                    log::warn!("Wi-Fi setup could not start: {error:#}");
                }
            }
            2 => self.close()?,
            _ => (),
        }
        if let Some(session) = self.session.as_ref() {
            if let Ok(candidate) = session.receiver.try_recv() {
                match apply(&mut self.wifi, Some(&candidate), Some(&session.ap)) {
                    Ok(()) => {
                        self.trial = Some((candidate, Instant::now()));
                        message("Connecting... allow 90 seconds", false);
                    }
                    Err(error) => {
                        apply(&mut self.wifi, self.current.as_ref(), Some(&session.ap))?;
                        message("Could not start connection. Try again.", true);
                        log::warn!("Wi-Fi configuration rejected: {error:#}");
                    }
                }
            }
        }
        if let Some((candidate, started)) = self.trial.as_ref() {
            let matched = self.ip().is_some()
                && self
                    .wifi
                    .wifi()
                    .get_ap_info()
                    .map(|ap| ap.ssid.as_str() == candidate.ssid)
                    .unwrap_or(false);
            if matched {
                let json = Zeroizing::new(serde_json::to_string(&(
                    candidate.ssid.as_str(),
                    candidate.password.as_str(),
                ))?);
                match self.settings.set_str("wifi_v1", &json) {
                    Ok(()) => {
                        self.current = Some(candidate.clone());
                        self.trial = None;
                        // Remove only this app's obsolete keys, and only after the atomic replacement committed.
                        if self.settings.remove("ssid").is_err()
                            || self.settings.remove("pass").is_err()
                        {
                            log::warn!("Could not remove obsolete Wi-Fi keys");
                        }
                        message("Connected and saved. Closing...", false);
                        if let Some(s) = self.session.as_mut() {
                            s.close_at = Some(Instant::now() + Duration::from_secs(6));
                        }
                        log::info!("WIFI_SAVED");
                    }
                    Err(_) => {
                        self.trial = None;
                        apply(
                            &mut self.wifi,
                            self.current.as_ref(),
                            self.session.as_ref().map(|s| &s.ap),
                        )?;
                        message("Could not save. Previous network kept.", true);
                    }
                }
            } else if started.elapsed() > Duration::from_secs(90) {
                self.trial = None;
                apply(
                    &mut self.wifi,
                    self.current.as_ref(),
                    self.session.as_ref().map(|s| &s.ap),
                )?;
                message("No connection. Check details and retry.", true);
            }
        }
        if self.session.as_ref().is_some_and(|s| {
            s.started.elapsed() > Duration::from_secs(600)
                || s.close_at.is_some_and(|t| Instant::now() >= t)
        }) {
            self.close()?;
        }
        if self.ip().is_none()
            && self.session.is_none()
            && self.trial.is_none()
            && self.fallback.is_some()
            && self.switch_at.elapsed() > Duration::from_secs(30)
        {
            std::mem::swap(&mut self.current, &mut self.fallback);
            apply(&mut self.wifi, self.current.as_ref(), None)?;
            self.switch_at = Instant::now();
            log::info!("Trying another previously saved network on this badge");
        }
        if self.ip().is_none() && self.reconnect_at.elapsed() > Duration::from_secs(8) {
            if self.current.is_some() || self.trial.is_some() {
                let _ = self.wifi.wifi_mut().connect();
            }
            self.reconnect_at = Instant::now();
        }
        STATION_IP.store(self.ip().map(u32::from).unwrap_or(0), Ordering::Release);
        Ok(())
    }
    fn open(&mut self) -> Result<()> {
        let mut random = [0u8; 32];
        getrandom::getrandom(&mut random).map_err(|_| anyhow::anyhow!("setup RNG unavailable"))?;
        let ssid = format!("Nick-Badge-{}", crate::hex(&random[..3]));
        let password = Zeroizing::new(crate::hex(&random[3..15]));
        let nonce = crate::hex(&random[16..]);
        let ap = AccessPointConfiguration {
            ssid: ssid.as_str().try_into().unwrap(),
            password: password.as_str().try_into().unwrap(),
            auth_method: AuthMethod::WPA2Personal,
            channel: self
                .wifi
                .wifi()
                .get_ap_info()
                .map(|a| a.channel)
                .unwrap_or(1),
            max_connections: 2,
            ..Default::default()
        };
        apply(&mut self.wifi, self.current.as_ref(), Some(&ap))?;
        if self.wifi.wifi().ap_netif().get_ip_info()?.ip != AP {
            bail!("unexpected setup IP configuration");
        }
        let (sender, receiver) = sync_channel::<Credentials>(1);
        let mut server = EspHttpServer::new(&HttpConfig {
            stack_size: 8192,
            max_sessions: 2,
            max_open_sockets: 2,
            max_uri_handlers: 4,
            max_resp_headers: 8,
            uri_match_wildcard: true,
            ..Default::default()
        })?;
        let page_nonce = nonce.clone();
        server.fn_handler::<anyhow::Error, _>("/", Method::Get, move |mut req| {
            if !from_ap(req.connection()) {
                req.into_status_response(403)?
                    .write_all(b"Setup hotspot only")?;
                return Ok(());
            }
            let v = view().lock().unwrap().clone();
            req.into_response(200, None, &HEADERS)?
                .write_all(page(&page_nonce, &v.message, v.accepting).as_bytes())?;
            Ok(())
        })?;
        server.fn_handler::<anyhow::Error, _>("/save", Method::Post, move |mut req| {
            if !from_ap(req.connection()) {
                req.into_status_response(403)?
                    .write_all(b"Setup hotspot only")?;
                return Ok(());
            }
            let valid_type = req.header("Content-Type").is_some_and(|v| {
                v.split(';').next().unwrap_or("").trim() == "application/x-www-form-urlencoded"
            });
            let length = req
                .header("Content-Length")
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| *n <= 512);
            if !valid_type || length.is_none() {
                req.into_status_response(400)?.write_all(b"Invalid form")?;
                return Ok(());
            }
            let mut body = Zeroizing::new(vec![0u8; length.unwrap()]);
            let mut used = 0;
            while used < body.len() {
                let n = req.read(&mut body[used..])?;
                if n == 0 {
                    break;
                }
                used += n;
            }
            if used != body.len() {
                req.into_status_response(400)?
                    .write_all(b"Incomplete form")?;
                return Ok(());
            }
            let candidate = match parse_form(&body, &nonce) {
                Ok(c) => c,
                Err(error) => {
                    req.into_response(400, None, &HEADERS)?
                        .write_all(page(&nonce, error, true).as_bytes())?;
                    return Ok(());
                }
            };
            let accepted = {
                let mut v = view().lock().unwrap();
                if v.accepting && sender.try_send(candidate).is_ok() {
                    v.accepting = false;
                    v.message = "Connecting... allow 90 seconds".into();
                    true
                } else {
                    false
                }
            };
            if !accepted {
                req.into_status_response(409)?
                    .write_all(b"A connection attempt is already running")?;
                return Ok(());
            }
            req.into_response(
                303,
                None,
                &[("Location", "/"), ("Cache-Control", "no-store")],
            )?
            .write_all(b"")?;
            Ok(())
        })?;
        server.fn_handler::<anyhow::Error, _>("/*", Method::Get, move |mut req| {
            if !from_ap(req.connection()) {
                req.into_status_response(403)?
                    .write_all(b"Setup hotspot only")?;
                return Ok(());
            }
            req.into_response(
                302,
                None,
                &[
                    ("Location", "http://192.168.4.1/"),
                    ("Cache-Control", "no-store"),
                ],
            )?
            .write_all(b"")?;
            Ok(())
        })?;
        let socket = UdpSocket::bind((AP, 53))?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        let dns = std::thread::Builder::new()
            .name("setup-dns".into())
            .stack_size(4096)
            .spawn(move || {
                let mut packet = [0u8; 512];
                while flag.load(Ordering::Acquire) {
                    if let Ok((len, source)) = socket.recv_from(&mut packet) {
                        if matches!(source.ip(), std::net::IpAddr::V4(ip) if ap_address(ip)) {
                            if let Some(reply) = dns_reply(&packet[..len]) {
                                let _ = socket.send_to(&reply, source);
                            }
                        }
                    }
                }
            })?;
        {
            let mut v = view().lock().unwrap();
            v.phase = 2;
            v.qr = Zeroizing::new(format!("WIFI:T:WPA;S:{ssid};P:{};;", password.as_str()));
            v.message = "Then open http://192.168.4.1".into();
            v.accepting = true;
        }
        self.session = Some(Session {
            _server: server,
            receiver,
            ap,
            running,
            dns: Some(dns),
            started: Instant::now(),
            close_at: None,
        });
        log::info!("WIFI_AP_READY (secured, local-only, ten-minute timeout)");
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        let existed = self.session.take().is_some() || active();
        self.trial = None;
        if existed {
            if let Err(error) = apply(&mut self.wifi, self.current.as_ref(), None) {
                // One bounded driver reset retry. Never release the recording lock while AP teardown is uncertain.
                let _ = self.wifi.stop();
                if apply(&mut self.wifi, self.current.as_ref(), None).is_err() {
                    message("Wi-Fi fault. Keep power to retain audio.", false);
                    return Err(error);
                }
            }
            log::info!("WIFI_AP_CLOSED");
        }
        hide();
        unsafe {
            badge_end_wifi();
        }
        Ok(())
    }
}
struct Session {
    _server: EspHttpServer<'static>,
    receiver: Receiver<Credentials>,
    ap: AccessPointConfiguration,
    running: Arc<AtomicBool>,
    dns: Option<std::thread::JoinHandle<()>>,
    started: Instant,
    close_at: Option<Instant>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(task) = self.dns.take() {
            let _ = task.join();
        }
    }
}

fn socket_ipv4(fd: i32, peer: bool) -> Option<Ipv4Addr> {
    use esp_idf_svc::sys::*;
    unsafe {
        let mut address: sockaddr_storage = std::mem::zeroed();
        let mut len = std::mem::size_of::<sockaddr_storage>() as socklen_t;
        let ptr = (&mut address as *mut sockaddr_storage).cast::<sockaddr>();
        let result = if peer {
            lwip_getpeername(fd, ptr, &mut len)
        } else {
            lwip_getsockname(fd, ptr, &mut len)
        };
        if result != 0 {
            return None;
        }
        if address.ss_family as u32 == AF_INET && len as usize >= std::mem::size_of::<sockaddr_in>()
        {
            let a = &*((&address as *const sockaddr_storage).cast::<sockaddr_in>());
            Some(Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr)))
        } else if address.ss_family as u32 == AF_INET6
            && len as usize >= std::mem::size_of::<sockaddr_in6>()
        {
            let a = &*((&address as *const sockaddr_storage).cast::<sockaddr_in6>());
            Ipv6Addr::from(a.sin6_addr.un.u8_addr).to_ipv4_mapped()
        } else {
            None
        }
    }
}
fn from_ap(connection: &mut EspHttpConnection<'_>) -> bool {
    let fd = unsafe { esp_idf_svc::sys::httpd_req_to_sockfd(connection.handle()) };
    fd >= 0 && socket_ipv4(fd, false) == Some(AP) && socket_ipv4(fd, true).is_some_and(ap_address)
}
/// USB diagnostic: real on-device HTTP requests, no submitted Wi-Fi credentials.
pub fn self_check() -> Result<()> {
    use embedded_svc::http::client::Client;
    use esp_idf_svc::http::client::{
        Configuration as ClientConfig, EspHttpConnection, FollowRedirectsPolicy,
    };
    if view().lock().unwrap().phase != 2 {
        bail!("setup hotspot is not ready");
    }
    let config = ClientConfig {
        timeout: Some(Duration::from_secs(3)),
        follow_redirects_policy: FollowRedirectsPolicy::FollowNone,
        ..Default::default()
    };
    let mut client = Client::wrap(EspHttpConnection::new(&config)?);
    {
        let mut response = client.get("http://192.168.4.1/")?.submit()?;
        if response.status() != 200 {
            bail!("AP page rejected");
        }
        let mut body = Vec::new();
        let mut buffer = [0u8; 512];
        loop {
            let n = response.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..n]);
            if body.len() > 8192 {
                bail!("unexpected page size");
            }
        }
        if !String::from_utf8_lossy(&body).contains("Connect badge") {
            bail!("form missing");
        }
    }
    {
        let body = b"nonce=invalid&ssid=Test&password=not-a-secret";
        let length = body.len().to_string();
        let headers = [
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Content-Length", length.as_str()),
        ];
        let mut request = client.request(Method::Post, "http://192.168.4.1/save", &headers)?;
        request.write_all(body)?;
        let response = request.submit()?;
        if response.status() != 400 {
            bail!("invalid nonce was not rejected");
        }
    }
    let ip = STATION_IP.load(Ordering::Acquire);
    if ip != 0 && Ipv4Addr::from(ip) != AP {
        let uri = format!("http://{}/", Ipv4Addr::from(ip));
        let response = client
            .request(Method::Get, &uri, &[("Host", "192.168.4.1")])?
            .submit()?;
        if response.status() != 403 {
            bail!("station-side request was not rejected");
        }
        log::info!("WIFI_CHECK_PASS AP form, CSRF rejection, station-interface rejection");
    } else {
        log::info!("WIFI_CHECK_PARTIAL AP form and CSRF rejection; no station IP for LAN check");
    }
    Ok(())
}

const HEADERS: [(&str, &str); 5] = [
    ("Content-Type", "text/html; charset=utf-8"),
    ("Cache-Control", "no-store"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
    (
        "Content-Security-Policy",
        "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",
    ),
];
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn page(nonce: &str, message: &str, accepting: bool) -> String {
    format!(
        r#"<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Nick's badge · Wi-Fi</title>{refresh}<style>body{{font:18px system-ui;background:#101512;color:#f7f2e7;max-width:30rem;margin:3rem auto;padding:1.3rem;line-height:1.5}}h1{{font-size:2.4rem;line-height:1.1}}label{{display:block;margin:1.4rem 0 .4rem}}input,button{{font:inherit;box-sizing:border-box;width:100%;padding:.8rem;border-radius:.5rem;border:1px solid #88998b}}input{{background:#18231c;color:#fff}}button{{margin-top:1.5rem;background:#addeb7;color:#101512;font-weight:700}}button:disabled{{opacity:.55}}small{{display:block;margin-top:1.5rem}}input:focus-visible,button:focus-visible{{outline:3px solid #efbc69;outline-offset:3px}}</style><main><h1>Get Nick online.</h1><p role="status">{message}</p><form action="/save" method="post"><input type="hidden" name="nonce" value="{nonce}"><label for="ssid">Wi-Fi name</label><input id="ssid" name="ssid" maxlength="32" required autocomplete="off"><label for="password">Wi-Fi password</label><input id="password" name="password" type="password" maxlength="63" autocomplete="off"><button {disabled}>Connect badge</button></form><small>2.4 GHz personal Wi-Fi. Leave the password empty only for an open network. Enterprise sign-in and guest login pages aren't supported—use your hotspot instead.</small><small>Using this phone's hotspot? Submit, leave this setup network, and turn your hotspot back on. The badge will try for 90 seconds.</small><small>Your password stays on the badge. This temporary setup network closes after success or 10 minutes.</small></main></html>"#,
        refresh = if accepting {
            ""
        } else {
            r#"<meta http-equiv="refresh" content="2">"#
        },
        message = escape(message),
        nonce = escape(nonce),
        disabled = if accepting { "" } else { "disabled" }
    )
}
