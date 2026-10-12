//! USB-only quality test. Remote status packets cannot start a microphone.
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::time::Duration;
use zeroize::Zeroizing;

const SAMPLE_COUNT: usize = 16_000 * 30;
extern "C" {
    fn badge_mic_capture(samples: *mut i16, count: u32) -> i32;
    fn badge_mic_cancelled() -> i32;
    fn badge_mic_duration_ms() -> u32;
    fn badge_mic_finish();
    fn badge_usb_line(line: *const u8, length: u32) -> i32;
}

fn send(line: &str) -> Result<()> {
    if line.len() > 8192 {
        bail!("USB frame too large");
    }
    if unsafe { badge_usb_line(line.as_ptr(), line.len() as u32) } != 0 {
        bail!("USB write failed");
    }
    Ok(())
}

pub fn start_console() -> Result<()> {
    std::thread::Builder::new()
        .name("mic-test-console".into())
        .stack_size(12_288)
        .spawn(|| {
            loop {
                let _ = send("MIC_TEST_READY");
                match crate::device::read_line("", 32) {
                    Ok(command) if command == "CLOUD_CHECK" => {
                        if let Err(e) = crate::recorder::check_connection() {
                            let _ = send(&format!("CLOUD_CHECK_ERROR {e:#}"));
                        }
                    }
                    Ok(command) if command == "HEAP" => {
                        let _ = send(&format!(
                            "HEAP internal={} psram={} largest_internal={}",
                            unsafe {
                                esp_idf_svc::sys::heap_caps_get_free_size(
                                    esp_idf_svc::sys::MALLOC_CAP_INTERNAL
                                        | esp_idf_svc::sys::MALLOC_CAP_8BIT,
                                )
                            },
                            unsafe {
                                esp_idf_svc::sys::heap_caps_get_free_size(
                                    esp_idf_svc::sys::MALLOC_CAP_SPIRAM,
                                )
                            },
                            unsafe {
                                esp_idf_svc::sys::heap_caps_get_largest_free_block(
                                    esp_idf_svc::sys::MALLOC_CAP_INTERNAL
                                        | esp_idf_svc::sys::MALLOC_CAP_8BIT,
                                )
                            }
                        ));
                    }
                    Ok(command) if command == "CLOUD_CONFIG" => {
                        if let Err(e) = crate::recorder::configure() {
                            let _ = send(&format!("CLOUD_CONFIG_ERROR {e:#}"));
                        }
                    }
                    Ok(command) if command == "SYNTH_OVERFLOW_TEST" => {
                        if let Err(e) = crate::recorder::overflow_test() {
                            let _ = send(&format!("SYNTHETIC_ERROR {e:#}"));
                        }
                    }
                    Ok(command)
                        if command.starts_with("SYNTH_RING_TEST ")
                            || command.starts_with("SYNTH_CLOUD ") =>
                    {
                        let seconds = command
                            .split_whitespace()
                            .nth(1)
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(0);
                        if let Err(e) =
                            crate::recorder::synthetic(seconds, command.starts_with("SYNTH_CLOUD"))
                        {
                            let _ = send(&format!("SYNTHETIC_ERROR {e:#}"));
                        }
                    }
                    Ok(command) if command == "WIFI_START_FAILURE_TEST" => {
                        crate::device::request_start_failure_test();
                        let _ = send("WIFI_START_FAILURE_TEST_REQUESTED");
                    }
                    Ok(command) if command == "WIFI_UI_CLOSE" => {
                        crate::portal::request_close_via_display();
                        let _ = send("WIFI_UI_CLOSE_REQUESTED");
                    }
                    Ok(command) if command == "WIFI_UI_SETUP" => {
                        crate::portal::request_open_via_display();
                        let _ = send("WIFI_UI_SETUP_REQUESTED");
                    }
                    Ok(command) if command == "WIFI_SETUP" => {
                        let _ = send(if crate::portal::request_open() {
                            "WIFI_SETUP_REQUESTED"
                        } else {
                            "WIFI_SETUP_BUSY"
                        });
                    }
                    Ok(command) if command == "WIFI_CHECK" => {
                        if let Err(error) = crate::portal::self_check() {
                            let _ = send(&format!("WIFI_CHECK_ERROR {error:#}"));
                        }
                    }
                    Ok(command) if command == "WIFI_CLOSE" => {
                        crate::portal::request_close();
                        let _ = send("WIFI_CLOSE_REQUESTED");
                    }
                    Ok(command) if command == "MIC_TEST" => {
                        if let Err(error) = capture() {
                            // Error text contains no audio or credentials.
                            let _ = send(&format!("MIC_ERROR {error}"));
                        }
                    }
                    _ => {
                        let _ = send("MIC_ERROR unknown command");
                    }
                }
            }
        })
        .context("could not start USB mic-test console")?;
    Ok(())
}

fn capture() -> Result<()> {
    if crate::portal::active() {
        bail!("close Wi-Fi setup before recording");
    }
    let mut samples = Zeroizing::new(Vec::<i16>::new());
    samples
        .try_reserve_exact(SAMPLE_COUNT)
        .context("not enough memory for microphone test")?;
    samples.resize(SAMPLE_COUNT, 0);
    send("MIC_ARMED Press A on the badge to begin; B/tap cancels")?;
    let result = unsafe { badge_mic_capture(samples.as_mut_ptr(), SAMPLE_COUNT as u32) };
    if result != 0 {
        bail!("capture cancelled, timed out, or unavailable ({result})");
    }
    // Capture has ended and the I2S task is stopped before the buffer is read.
    let result = export(&samples);
    unsafe {
        badge_mic_finish();
    }
    result
}

fn export(samples: &[i16]) -> Result<()> {
    // ESP32-S3 is little-endian; the host verifies both byte count and SHA256.
    let pcm =
        unsafe { std::slice::from_raw_parts(samples.as_ptr().cast::<u8>(), samples.len() * 2) };
    let digest = Sha256::digest(pcm);
    let checksum: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let elapsed = unsafe { badge_mic_duration_ms() };
    send(&format!(
        "MIC_BEGIN 16000 {} {elapsed} {checksum}",
        samples.len()
    ))?;
    let mut line = Zeroizing::new(String::with_capacity(4200));
    for (index, bytes) in pcm.chunks(2048).enumerate() {
        if unsafe { badge_mic_cancelled() } != 0 {
            bail!("sample export cancelled");
        }
        line.clear();
        write!(&mut *line, "MIC_DATA {index} ")?;
        for byte in bytes {
            write!(&mut *line, "{byte:02x}")?;
        }
        send(&line)?;
        std::thread::sleep(Duration::from_millis(25));
    }
    send("MIC_END")
}
