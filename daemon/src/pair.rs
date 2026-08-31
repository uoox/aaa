//! `/pair` — QR payload with reachable hosts, ordered:
//! tailscale CGNAT (100.64/10) > easytier (10/8) > tailscale MagicDNS name >
//! other private IPv4.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

fn encode(s: &str) -> String {
    // keep unreserved chars readable
    const KEEP: percent_encoding::AsciiSet = NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(s, &KEEP).to_string()
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    if !ok {
        return "mac".to_string();
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char) };
    let name = cstr.to_string_lossy().into_owned();
    name.trim_end_matches(".local").to_string()
}

fn prio_of(ip: &std::net::Ipv4Addr) -> Option<u8> {
    let o = ip.octets();
    if o[0] == 100 && (64..128).contains(&o[1]) {
        Some(0) // tailscale CGNAT
    } else if o[0] == 10 {
        Some(1) // easytier
    } else if (o[0] == 192 && o[1] == 168) || (o[0] == 172 && (16..32).contains(&o[1])) {
        Some(3) // other private LAN, last resort
    } else {
        None
    }
}

fn magicdns_name() -> Option<String> {
    let out = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let name = v.get("Self")?.get("DNSName")?.as_str()?;
    let name = name.trim_end_matches('.');
    (!name.is_empty()).then(|| name.to_string())
}

pub fn build_payload(port: u16, token: &str) -> String {
    let mut hosts: Vec<(u8, String)> = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let std::net::IpAddr::V4(v4) = iface.ip() {
                if let Some(prio) = prio_of(&v4) {
                    hosts.push((prio, format!("{v4}:{port}")));
                }
            }
        }
    }
    if let Some(dns) = magicdns_name() {
        hosts.push((2, format!("{dns}:{port}")));
    }
    hosts.sort();
    hosts.dedup();
    let host_list = hosts
        .into_iter()
        .map(|(_, h)| h)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "aaa://pair?v=1&name={}&hosts={}&token={}",
        encode(&hostname()),
        host_list,
        encode(token)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities() {
        assert_eq!(prio_of(&"100.100.1.2".parse().unwrap()), Some(0));
        assert_eq!(prio_of(&"100.63.1.2".parse().unwrap()), None); // below CGNAT range
        assert_eq!(prio_of(&"10.5.6.7".parse().unwrap()), Some(1));
        assert_eq!(prio_of(&"192.168.1.10".parse().unwrap()), Some(3));
        assert_eq!(prio_of(&"172.20.0.1".parse().unwrap()), Some(3));
        assert_eq!(prio_of(&"8.8.8.8".parse().unwrap()), None);
    }

    #[test]
    fn payload_shape() {
        let p = build_payload(2730, "aaa_tk_x");
        assert!(p.starts_with("aaa://pair?v=1&name="));
        assert!(p.contains("&token=aaa_tk_x"));
    }
}
