use anyhow::{bail, Context, Result};
use embedded_svc::http::{client::Client, Method};
use embedded_svc::io::Write;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection, FollowRedirectsPolicy};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;
use zeroize::Zeroizing;

pub const ORIGIN: &str = "https://transcribe.nisi.sh";
pub struct Credentials {
    pub id: String,
    pub secret: Zeroizing<String>,
}
#[derive(Debug)]
pub struct HttpFailure(pub u16);
impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cloud HTTP {}", self.0)
    }
}
impl std::error::Error for HttpFailure {}
impl Credentials {
    pub fn validate(id: String, secret: String) -> Result<Self> {
        if !id.ends_with(".access")
            || id.len() > 80
            || id.len() < 12
            || id.bytes().any(|b| !b.is_ascii_graphic())
        {
            bail!("invalid Access client ID");
        }
        if !(32..=128).contains(&secret.len()) || secret.bytes().any(|b| !b.is_ascii_graphic()) {
            bail!("invalid Access client secret");
        }
        Ok(Self {
            id,
            secret: Zeroizing::new(secret),
        })
    }
    fn request(
        &self,
        method: Method,
        path: &str,
        body: &[u8],
        kind: &str,
        checksum: Option<&str>,
    ) -> Result<Value> {
        // ponytail: fresh TLS per chunk avoids stale-connection state; reuse keep-alive if measured power/latency warrants it.
        let config = Configuration {
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            timeout: Some(Duration::from_secs(8)),
            follow_redirects_policy: FollowRedirectsPolicy::FollowNone,
            ..Default::default()
        };
        let mut client = Client::wrap(EspHttpConnection::new(&config)?);
        let uri = format!("{ORIGIN}{path}");
        let length = body.len().to_string();
        let mut headers = vec![
            ("User-Agent", "riker-badge/1.0"),
            ("CF-Access-Client-Id", self.id.as_str()),
            ("CF-Access-Client-Secret", self.secret.as_str()),
        ];
        if method != Method::Get {
            headers.push(("Content-Type", kind));
            headers.push(("Content-Length", &length));
        }
        if let Some(hash) = checksum {
            headers.push(("X-Audio-Sha256", hash));
        }
        let mut request = client.request(method, &uri, &headers)?;
        if !body.is_empty() {
            request.write_all(body)?;
        }
        let mut response = request.submit()?;
        let status = response.status();
        if !(200..300).contains(&status) {
            return Err(HttpFailure(status).into());
        }
        let mut output = Vec::new();
        let mut buffer = [0u8; 512];
        loop {
            let n = response.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            if output.len() + n > 4096 {
                bail!("cloud response too large");
            }
            output.extend_from_slice(&buffer[..n]);
        }
        serde_json::from_slice(&output).context("invalid cloud response")
    }
    pub fn check(&self) -> Result<()> {
        let v = self.request(Method::Get, "/api/device/config", &[], "", None)?;
        if v["sample_rate"].as_u64() != Some(16000)
            || v["channels"].as_u64() != Some(1)
            || v["format"].as_str() != Some("s16le")
            || v["chunk_samples"].as_u64() != Some(160000)
        {
            bail!("cloud audio protocol mismatch");
        }
        Ok(())
    }
    pub fn create(&self, id: &str, title: &str) -> Result<()> {
        let body = serde_json::to_vec(&serde_json::json!({"id":id,"title":title}))?;
        let v = self.request(
            Method::Post,
            "/api/device/recordings",
            &body,
            "application/json",
            None,
        )?;
        if v["id"].as_str() != Some(id) {
            bail!("cloud recording identity mismatch");
        }
        Ok(())
    }
    pub fn upload(&self, id: &str, seq: u32, pcm: &[i16]) -> Result<()> {
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr().cast::<u8>(), pcm.len() * 2) };
        let hash: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let v = self.request(
            Method::Put,
            &format!("/api/device/recordings/{id}/chunks/{seq}"),
            bytes,
            "application/octet-stream",
            Some(&hash),
        )?;
        if v["saved"].as_bool() != Some(true)
            || v["seq"].as_u64() != Some(seq as u64)
            || v["sha256"].as_str() != Some(hash.as_str())
        {
            bail!("cloud did not acknowledge this chunk");
        }
        Ok(())
    }
    /// Close an interrupted prior session without ever starting capture.
    pub fn recover(&self, id: &str) -> Result<bool> {
        let v = match self.request(
            Method::Get,
            &format!("/api/device/recordings/{id}"),
            &[],
            "",
            None,
        ) {
            Ok(v) => v,
            Err(error)
                if error
                    .downcast_ref::<HttpFailure>()
                    .is_some_and(|e| e.0 == 404) =>
            {
                return Ok(false)
            }
            Err(error) => return Err(error),
        };
        if v["id"].as_str() != Some(id) {
            bail!("recovery identity mismatch");
        }
        match v["status"].as_str() {
            Some("open") => {
                let (chunks, samples) = if v["last_chunk"].is_null() {
                    (0, 0)
                } else {
                    let seq = v["last_chunk"]["seq"]
                        .as_u64()
                        .context("invalid recovery sequence")?;
                    let count = v["last_chunk"]["samples"]
                        .as_u64()
                        .context("invalid recovery length")?;
                    if seq >= 2880 || count > 160000 {
                        bail!("invalid recovery extent");
                    }
                    ((seq + 1) as u32, seq * 160000 + count)
                };
                match self.seal(id, chunks, samples, true) {
                    Ok(()) => Ok(true),
                    Err(error)
                        if error
                            .downcast_ref::<HttpFailure>()
                            .is_some_and(|e| e.0 == 410) =>
                    {
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            }
            Some("sealed" | "finalizing" | "complete" | "partial" | "notes-failed") => Ok(true),
            _ => bail!("invalid recovery status"),
        }
    }
    pub fn seal(&self, id: &str, chunks: u32, samples: u64, interrupted: bool) -> Result<()> {
        let body = serde_json::to_vec(
            &serde_json::json!({"expected_chunks":chunks,"total_samples":samples,"reason":if interrupted{"interrupted"}else{"stopped"}}),
        )?;
        self.request(
            Method::Post,
            &format!("/api/device/recordings/{id}/seal"),
            &body,
            "application/json",
            None,
        )?;
        Ok(())
    }
}
pub fn valid_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub fn id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| anyhow::anyhow!("recording ID RNG failed"))?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h = crate::hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}
