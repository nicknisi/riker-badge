use crate::cloud_client::{self, Credentials, HttpFailure};
use anyhow::{bail, Context, Result};
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use riker_badge::audio_buffer::{AudioBuffer, AudioChunk, RATE};
use riker_badge::recording_start::StartBlocked;
use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{
    mpsc::{sync_channel, SyncSender},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Clone)]
struct View {
    phase: u8,
    kind: u8,
    message: String,
}
static VIEW: OnceLock<Mutex<View>> = OnceLock::new();
static STORE: OnceLock<EspNvsPartition<NvsDefault>> = OnceLock::new();
static COMMAND: OnceLock<SyncSender<Command>> = OnceLock::new();
static BUSY: AtomicBool = AtomicBool::new(false);
static CAPTURED: AtomicU32 = AtomicU32::new(0);
static SAVED: AtomicU32 = AtomicU32::new(0);
static DISCARD: AtomicBool = AtomicBool::new(false);
static RETRY: AtomicBool = AtomicBool::new(false);
struct StreamGuard;
impl Drop for StreamGuard {
    fn drop(&mut self) {
        stop();
    }
}
enum Command {
    Record {
        kind: u8,
        synthetic: bool,
        seconds: Option<u32>,
        intent: Instant,
    },
    RingTest(u32),
    OverflowTest,
}
extern "C" {
    fn badge_recorder_set(phase: u8, captured: u32, saved: u32, message: *const c_char);
    fn badge_recorder_take_action() -> u8;
    fn badge_recorder_cancelled() -> bool;
    fn badge_recorder_test_prepare();
    fn badge_stream_begin(
        callback: extern "C" fn(*mut c_void, *const i16, u32) -> bool,
        context: *mut c_void,
        synthetic: bool,
    ) -> i32;
    fn badge_stream_request_stop();
    fn badge_stream_running() -> bool;
    fn badge_stream_failed() -> bool;
    fn badge_stream_overflowed() -> bool;
    fn badge_stream_mic_active() -> bool;
}
fn view() -> &'static Mutex<View> {
    VIEW.get_or_init(|| {
        Mutex::new(View {
            phase: 0,
            kind: 0,
            message: String::new(),
        })
    })
}
fn status(phase: u8, message: &str) {
    let mut v = view().lock().unwrap();
    v.phase = phase;
    v.message = message.chars().take(100).collect();
}
fn counts(total: u64, saved: u64) {
    CAPTURED.fetch_max(total as u32, Ordering::Relaxed);
    SAVED.store(saved as u32, Ordering::Relaxed);
}
fn store() -> Result<EspNvs<NvsDefault>> {
    EspNvs::new(
        STORE.get().context("cloud settings unavailable")?.clone(),
        "rbcloud",
        true,
    )
    .map_err(Into::into)
}
fn credentials() -> Result<Credentials> {
    let s = store()?;
    let mut b = Zeroizing::new([0u8; 512]);
    let text = s
        .get_str("access_v1", &mut *b)?
        .context("cloud credentials not configured")?;
    let (id, secret): (String, String) = serde_json::from_str(text)?;
    Credentials::validate(id, secret)
}
fn active_id() -> Result<Option<String>> {
    let mut b = [0u8; 40];
    Ok(store()?.get_str("active", &mut b)?.map(str::to_owned))
}
fn clear_active() {
    if let Ok(s) = store() {
        let _ = s.remove("active");
    }
}

pub fn start(partition: EspNvsPartition<NvsDefault>) -> Result<()> {
    if STORE.set(partition).is_err() {
        return Ok(());
    }
    let (tx, rx) = sync_channel(1);
    let _ = COMMAND.set(tx);
    std::thread::Builder::new()
        .name("cloud-recorder".into())
        .stack_size(24_576)
        .spawn(move || {
            let mut recovery_at = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(command) => {
                        let result = match command {
                            Command::Record {
                                kind,
                                synthetic,
                                seconds,
                                intent,
                            } => record(kind, synthetic, seconds, intent),
                            Command::RingTest(seconds) => ring_test(seconds, false),
                            Command::OverflowTest => ring_test(45, true),
                        };
                        if let Err(error) = result {
                            unsafe {
                                badge_stream_request_stop();
                            }
                            while unsafe { badge_stream_running() } {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            log::warn!("recorder operation failed: {error:#}");
                            if let Some(block) = error.downcast_ref::<StartBlocked>() {
                                status(10, block.message());
                            } else {
                                status(7, "Not recording. Check setup or USB.");
                            }
                        }
                        BUSY.store(false, Ordering::Release);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(_) => {
                        if !BUSY.load(Ordering::Acquire)
                            && crate::device::network_ready()
                            && recovery_at.elapsed() > Duration::from_secs(30)
                        {
                            // Only finish bookkeeping for an interrupted old recording. Never restart the mic.
                            let _ = recover();
                            recovery_at = Instant::now();
                        }
                    }
                }
            }
        })
        .context("could not start recorder worker")?;
    Ok(())
}
fn reserve(command: Command) -> bool {
    if BUSY.swap(true, Ordering::AcqRel) {
        println!("RECORDER_START_REJECTED busy");
        return false;
    }
    DISCARD.store(false, Ordering::Release);
    RETRY.store(false, Ordering::Release);
    CAPTURED.store(0, Ordering::Relaxed);
    SAVED.store(0, Ordering::Relaxed);
    let synthetic = matches!(
        &command,
        Command::RingTest(_)
            | Command::OverflowTest
            | Command::Record {
                synthetic: true,
                ..
            }
    );
    if synthetic {
        unsafe {
            badge_recorder_test_prepare();
        }
        status(8, "Generated data only. No microphone.");
    } else {
        if let Some(block) = start_block() {
            BUSY.store(false, Ordering::Release);
            println!("RECORDER_START_BLOCKED {block}");
            status(10, block.message());
            return false;
        }
        status(2, "Checking private cloud connection...");
    }
    if COMMAND.get().is_none_or(|tx| tx.try_send(command).is_err()) {
        BUSY.store(false, Ordering::Release);
        println!("RECORDER_START_REJECTED worker_unavailable");
        status(7, "Recorder worker unavailable");
        return false;
    }
    true
}
pub fn update_display() {
    match unsafe { badge_recorder_take_action() } {
        action @ (1 | 2) if !BUSY.load(Ordering::Acquire) => {
            let mut v = view().lock().unwrap();
            v.phase = 1;
            CAPTURED.store(0, Ordering::Relaxed);
            SAVED.store(0, Ordering::Relaxed);
            v.kind = action - 1;
            v.message = "Audio goes to transcribe.nisi.sh".into();
        }
        3 => {
            let kind = view().lock().unwrap().kind;
            reserve(Command::Record {
                kind,
                synthetic: false,
                seconds: None,
                intent: Instant::now(),
            });
        }
        4 => {
            unsafe {
                badge_stream_request_stop();
            }
            if !BUSY.load(Ordering::Acquire) {
                status(0, "");
            }
        }
        6 => {
            retry_upload();
        }
        5 => {
            DISCARD.store(true, Ordering::Release);
            unsafe {
                badge_stream_request_stop();
            }
        }
        _ => (),
    }
    let mut v = view().lock().unwrap().clone();
    if v.phase == 1 {
        if let Some(block) = start_block() {
            v.message = block.message().into();
        }
    }
    let msg = CString::new(v.message).unwrap_or_default();
    unsafe {
        badge_recorder_set(
            v.phase,
            CAPTURED.load(Ordering::Relaxed) / RATE as u32,
            SAVED.load(Ordering::Relaxed) / RATE as u32,
            msg.as_ptr(),
        );
    }
}
fn start_block() -> Option<StartBlocked> {
    StartBlocked::check(
        crate::portal::connected(),
        crate::device::network_ready(),
        crate::portal::active(),
    )
}
pub fn check_connection() -> Result<()> {
    if let Some(block) = start_block() {
        return Err(block.into());
    }
    credentials()?.check()?;
    println!("CLOUD_CONNECTION_READY");
    Ok(())
}
pub fn retry_upload() {
    if BUSY.load(Ordering::Acquire) && !unsafe { badge_stream_running() } {
        RETRY.store(true, Ordering::Release);
    }
}
pub fn configure() -> Result<()> {
    if unsafe { badge_stream_running() } {
        bail!("finish recording before configuring cloud access");
    }
    let id = crate::device::read_line("CLOUD_CLIENT_ID>", 80)?;
    let secret = Zeroizing::new(crate::device::read_line("CLOUD_CLIENT_SECRET>", 128)?);
    let c = Credentials::validate(id, secret.to_string())?;
    let value = Zeroizing::new(serde_json::to_string(&(c.id.as_str(), c.secret.as_str()))?);
    store()?.set_str("access_v1", &value)?;
    retry_upload();
    println!("CLOUD_CONFIG_SAVED");
    Ok(())
}
pub fn overflow_test() -> Result<()> {
    if crate::portal::active() || !reserve(Command::OverflowTest) {
        bail!("recorder busy");
    }
    Ok(())
}
pub fn synthetic(seconds: u32, cloud: bool) -> Result<()> {
    if !(30..=14400).contains(&seconds) || crate::portal::active() {
        bail!("invalid or busy synthetic test");
    }
    if BUSY.load(Ordering::Acquire) {
        bail!("recording already active");
    }
    let cmd = if cloud {
        Command::Record {
            kind: 0,
            synthetic: true,
            seconds: Some(seconds),
            intent: Instant::now(),
        }
    } else {
        Command::RingTest(seconds)
    };
    if !reserve(cmd) {
        bail!("recorder busy");
    }
    Ok(())
}
extern "C" fn receive(context: *mut c_void, data: *const i16, length: u32) -> bool {
    if context.is_null() || data.is_null() || length != 320 {
        return false;
    }
    let ring = unsafe { &*context.cast::<Mutex<AudioBuffer>>() };
    let samples = unsafe { std::slice::from_raw_parts(data, length as usize) };
    match ring.lock() {
        Ok(mut r) => {
            let accepted = r.push(samples);
            CAPTURED.store(r.total() as u32, Ordering::Relaxed);
            accepted
        }
        Err(_) => false,
    }
}
fn stop() {
    unsafe {
        badge_stream_request_stop();
    }
    while unsafe { badge_stream_running() } {
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn cancelled() -> bool {
    unsafe { badge_recorder_cancelled() }
}
fn interrupted(ring: &Mutex<AudioBuffer>) -> bool {
    ring.lock().unwrap().interrupted()
        || unsafe { badge_stream_failed() || badge_stream_overflowed() }
}
fn fatal(error: &anyhow::Error) -> bool {
    error.downcast_ref::<HttpFailure>().is_some_and(|e| {
        matches!(
            e.0,
            301 | 302 | 303 | 307 | 308 | 400 | 401 | 403 | 404 | 409 | 410 | 413 | 415 | 422 | 429
        )
    })
}
fn recover() -> Result<()> {
    let Some(id) = active_id()? else {
        return Ok(());
    };
    if !cloud_client::valid_id(&id) {
        clear_active();
        status(7, "Interrupted session needs website review.");
        return Ok(());
    }
    let c = credentials()?;
    let writable = c.recover(&id)?;
    clear_active();
    if !writable {
        status(7, "Previous session unavailable; check website.");
    }
    log::info!("interrupted recording recovery handled; no capture restarted");
    Ok(())
}
fn record(kind: u8, synthetic: bool, seconds: Option<u32>, intent: Instant) -> Result<()> {
    if intent.elapsed() > Duration::from_secs(15) || cancelled() {
        bail!("recording start expired or cancelled");
    }
    if let Some(block) = start_block() {
        return Err(block.into());
    }
    recover()?;
    let ring = Arc::new(Mutex::new(
        AudioBuffer::new().context("not enough audio buffer memory")?,
    ));
    let mut c = credentials()?;
    let id = cloud_client::id()?;
    c.check()?;
    c.create(
        &id,
        if synthetic {
            "Synthetic badge test — no microphone"
        } else if kind == 1 {
            "Conference voice note"
        } else {
            "Meeting"
        },
    )?;
    store()?.set_str("active", &id)?;
    if cancelled() || intent.elapsed() > Duration::from_secs(15) {
        if c.seal(&id, 0, 0, true).is_ok() {
            clear_active();
        }
        status(7, "Start cancelled or timed out. Mic stayed off.");
        return Ok(());
    }
    if !synthetic {
        status(3, "Starting in three seconds...");
        let countdown = Instant::now();
        while countdown.elapsed() < Duration::from_secs(3) {
            if cancelled() {
                if c.seal(&id, 0, 0, true).is_ok() {
                    clear_active();
                }
                status(0, "");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let code =
        unsafe { badge_stream_begin(receive, Arc::as_ptr(&ring).cast_mut().cast(), synthetic) };
    if code != 0 {
        if c.seal(&id, 0, 0, true).is_ok() {
            clear_active();
        }
        bail!("audio could not start ({code})");
    }
    let _stream_guard = StreamGuard;
    status(
        if synthetic { 8 } else { 4 },
        if synthetic {
            "Synthetic source. No microphone."
        } else {
            "Recording to private cloud."
        },
    );
    let started = Instant::now();
    let mut saved = 0u64;
    let mut in_flight: Option<AudioChunk> = None;
    let mut flushed = false;
    let mut interrupted_capture = false;
    let mut next_attempt = Instant::now();
    let mut upload_blocked = false;
    loop {
        if RETRY.swap(false, Ordering::AcqRel) {
            match credentials() {
                Ok(updated) => {
                    c = updated;
                    upload_blocked = false;
                    next_attempt = Instant::now();
                    status(5, "Retrying buffered audio. Microphone is off.");
                }
                Err(_) => {
                    upload_blocked = true;
                    status(7, "Cloud setup needed. Buffers retained.");
                }
            }
        }
        if cancelled()
            || seconds.is_some_and(|s| started.elapsed() >= Duration::from_secs(s as u64))
        {
            unsafe {
                badge_stream_request_stop();
            }
        }
        if synthetic && unsafe { badge_stream_mic_active() } {
            stop();
            bail!("synthetic test detected active microphone");
        }
        if !unsafe { badge_stream_running() } && !flushed {
            interrupted_capture |= interrupted(&ring);
            ring.lock().unwrap().flush();
            flushed = true;
            status(
                if upload_blocked { 7 } else { 5 },
                if upload_blocked {
                    "Cloud blocked. Orange retries; buffers kept."
                } else if interrupted_capture {
                    "Capture interrupted; keeping buffers."
                } else {
                    "Microphone off. Saving remaining audio."
                },
            );
        }
        let total = ring.lock().unwrap().total();
        counts(total, saved);
        if DISCARD.load(Ordering::Acquire) {
            stop();
            ring.lock().unwrap().flush();
            let r = ring.lock().unwrap();
            let chunks = r.chunks();
            let total = r.total();
            drop(r);
            if c.seal(&id, chunks, total, true).is_ok() {
                clear_active();
            }
            status(7, "Unsent audio discarded. Saved audio remains.");
            return Ok(());
        }
        if in_flight.is_none() {
            in_flight = ring.lock().unwrap().take();
        }
        if let Some(chunk) = in_flight.as_ref() {
            if !upload_blocked && Instant::now() >= next_attempt {
                match c.upload(&id, chunk.seq, &chunk.pcm) {
                    Ok(()) => {
                        let chunk = in_flight.take().unwrap();
                        saved += chunk.pcm.len() as u64;
                        ring.lock().unwrap().recycle(chunk);
                        log::info!("CLOUD_CHUNK_SAVED samples={saved}");
                    }
                    Err(error) => {
                        if fatal(&error) {
                            unsafe {
                                badge_stream_request_stop();
                            }
                            interrupted_capture = true;
                            upload_blocked = true;
                            status(7, "Cloud blocked. Orange retries; buffers kept.");
                        } else if !unsafe { badge_stream_running() } {
                            status(5, "Offline. Buffers kept; both buttons: Wi-Fi.");
                        }
                        log::warn!("cloud upload waiting: {error:#}");
                        next_attempt = Instant::now() + Duration::from_secs(3);
                    }
                }
            }
        } else if flushed {
            let r = ring.lock().unwrap();
            let n = r.chunks();
            let total = r.total();
            drop(r);
            match c.seal(&id, n, total, interrupted_capture) {
                Ok(()) => {
                    clear_active();
                    counts(total, saved);
                    status(
                        6,
                        if interrupted_capture {
                            "Saved with interruption. Check your notes."
                        } else {
                            "Audio saved. Notes are processing."
                        },
                    );
                    if total == 0 && unsafe { badge_stream_failed() } {
                        status(7, "Microphone failed; no audio captured.");
                    }
                    println!("CLOUD_RECORDING_SAVED {id}");
                    return Ok(());
                }
                Err(error) => {
                    status(5, "Audio saved; waiting to close the session.");
                    log::warn!("cloud finalize waiting: {error:#}");
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn ring_test(seconds: u32, stall: bool) -> Result<()> {
    let ring = Arc::new(Mutex::new(AudioBuffer::new()?));
    let code = unsafe { badge_stream_begin(receive, Arc::as_ptr(&ring).cast_mut().cast(), true) };
    if code != 0 {
        bail!("synthetic source unavailable");
    }
    let _stream_guard = StreamGuard;
    status(8, "Generated data only. No microphone.");
    let start = Instant::now();
    let mut received = 0u64;
    let mut minimum = usize::MAX;
    while start.elapsed() < Duration::from_secs(seconds as u64)
        && unsafe { badge_stream_running() }
        && !cancelled()
    {
        if unsafe { badge_stream_mic_active() } {
            stop();
            bail!("microphone unexpectedly active");
        }
        let next = if stall {
            None
        } else {
            ring.lock().unwrap().take()
        };
        if let Some(c) = next {
            received += c.pcm.len() as u64;
            ring.lock().unwrap().recycle(c);
        }
        let total = ring.lock().unwrap().total();
        counts(total, received);
        minimum = minimum.min(unsafe {
            esp_idf_svc::sys::heap_caps_get_free_size(
                esp_idf_svc::sys::MALLOC_CAP_INTERNAL | esp_idf_svc::sys::MALLOC_CAP_8BIT,
            )
        });
        std::thread::sleep(Duration::from_millis(50));
    }
    stop();
    let mut r = ring.lock().unwrap();
    r.flush();
    while let Some(c) = r.take() {
        received += c.pcm.len() as u64;
        r.recycle(c);
    }
    if received != r.total() || r.interrupted() != stall || unsafe { badge_stream_failed() } {
        bail!("synthetic ring lost data or stopped unexpectedly");
    }
    if stall {
        if cancelled()
            || received
                != (riker_badge::audio_buffer::SLOTS * riker_badge::audio_buffer::CHUNK) as u64
            || !unsafe { badge_stream_overflowed() }
        {
            bail!("overflow did not stop at the exact buffer boundary");
        }
        println!(
            "SYNTHETIC_OVERFLOW_RESULT samples={} mic_used=false retained_all=true",
            received
        );
    } else {
        println!("SYNTHETIC_RING_RESULT seconds={} samples={} min_internal_heap={} mic_used=false cancelled={}",start.elapsed().as_secs(),received,minimum,cancelled());
    }
    counts(received, received);
    status(9, "Synthetic test finished. No mic or upload.");
    Ok(())
}
