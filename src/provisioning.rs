//! Host-testable boundaries for the temporary, AP-only Wi-Fi form and DNS responder.
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Credentials {
    pub ssid: String,
    pub password: Zeroizing<String>,
}
impl Credentials {
    pub fn new(ssid: String, password: String) -> Result<Self, &'static str> {
        if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
            return Err("Network name must be 1-32 UTF-8 bytes without control characters");
        }
        if (!password.is_empty() && !(8..=63).contains(&password.len()))
            || password.chars().any(char::is_control)
        {
            return Err("Password must be empty for an open network, or 8-63 bytes");
        }
        Ok(Self {
            ssid,
            password: Zeroizing::new(password),
        })
    }
}
fn decode(text: &[u8]) -> Result<String, &'static str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let digits = text.get(i + 1..i + 3).ok_or("Invalid form encoding")?;
                let digits = std::str::from_utf8(digits).map_err(|_| "Invalid form encoding")?;
                out.push(u8::from_str_radix(digits, 16).map_err(|_| "Invalid form encoding")?);
                i += 2;
            }
            value => out.push(value),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| "Invalid UTF-8")
}
pub fn parse_form(body: &[u8], expected_nonce: &str) -> Result<Credentials, &'static str> {
    if body.len() > 512 {
        return Err("Form too large");
    }
    let mut ssid = None;
    let mut password = None;
    let mut nonce = None;
    for pair in body.split(|b| *b == b'&') {
        let at = pair.iter().position(|b| *b == b'=').ok_or("Invalid form")?;
        let key = decode(&pair[..at])?;
        let value = decode(&pair[at + 1..])?;
        match key.as_str() {
            "ssid" if ssid.is_none() => ssid = Some(value),
            "password" if password.is_none() => password = Some(value),
            "nonce" if nonce.is_none() => nonce = Some(value),
            _ => return Err("Unexpected or duplicate form field"),
        }
    }
    if nonce.as_deref() != Some(expected_nonce) || expected_nonce.len() != 32 {
        return Err("Setup session expired; reopen Wi-Fi setup on the badge");
    }
    Credentials::new(
        ssid.ok_or("Network name missing")?,
        password.ok_or("Password missing")?,
    )
}
pub fn dns_reply(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 17 || query.len() > 512 || query[2] & 0xf8 != 0 || query[4..6] != [0, 1] {
        return None;
    }
    let mut end = 12;
    loop {
        let length = *query.get(end)? as usize;
        if length > 63 {
            return None;
        } // no compressed question names
        end += 1;
        if length == 0 {
            break;
        }
        end = end.checked_add(length)?;
        if end > query.len() || end > 267 {
            return None;
        }
    }
    let question_type = u16::from_be_bytes(query.get(end..end + 2)?.try_into().ok()?);
    if query.get(end + 2..end + 4)? != [0, 1] {
        return None;
    }
    end += 4;
    let answer = question_type == 1;
    let mut out = query[..end].to_vec();
    out[2] = 0x84 | (query[2] & 1);
    out[3] = 0;
    out[6..12].fill(0);
    out[7] = u8::from(answer);
    if answer {
        // Authoritative wildcard A reply, TTL=0 so the phone doesn't retain a captive DNS answer.
        out.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 192, 168, 4, 1]);
    }
    Some(out)
}
pub fn ap_address(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[..3] == [192, 168, 4] && (1..=254).contains(&o[3])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_form_without_echoing_secrets() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let good = format!("nonce={nonce}&ssid=My+Network&password=test%2Bpassword");
        let c = parse_form(good.as_bytes(), nonce).unwrap();
        assert_eq!(c.ssid, "My Network");
        assert_eq!(&*c.password, "test+password");
        for bad in [
            good.clone() + "&nonce=x",
            good.replace(nonce, "wrong"),
            good.replace("My+Network", "%00"),
            good.replace("test%2Bpassword", "short"),
            good.replace("test%2Bpassword", "%GG"),
        ] {
            assert!(parse_form(bad.as_bytes(), nonce).is_err());
        }
        assert!(Credentials::new("Open".into(), "".into()).is_ok());
        assert!(Credentials::new("a".repeat(33), "".into()).is_err());
        assert!(!ap_address("172.20.10.2".parse().unwrap()));
        assert!(!ap_address("192.168.4.255".parse().unwrap()));
        assert!(ap_address("192.168.4.2".parse().unwrap()));
    }
    #[test]
    fn bounded_captive_dns() {
        let q = [
            0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, b'a', 0, 0, 1, 0, 1,
        ];
        let out = dns_reply(&q).unwrap();
        assert_eq!(&out[..2], &q[..2]);
        assert_eq!(&out[out.len() - 4..], &[192, 168, 4, 1]);
        assert_eq!(&out[out.len() - 10..out.len() - 6], &[0, 0, 0, 0]);
        let mut aaaa = q;
        aaaa[16] = 28;
        assert_eq!(dns_reply(&aaaa).unwrap()[7], 0);
        let mut compressed = q;
        compressed[12] = 0xc0;
        assert!(dns_reply(&compressed).is_none());
        assert!(dns_reply(&q[..15]).is_none());
    }
}
