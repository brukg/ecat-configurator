//! Other mailbox protocols configured at start-up: SoE (servo drive IDNs) and EoE (IP settings).

use serde::{Deserialize, Serialize};

/// An SoE write of an IDN during a state transition.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SoeInitCmd {
    pub transition: String,
    pub drive_no: u8,
    pub idn: u16,
    pub data: Vec<u8>,
    pub comment: String,
    pub enabled: bool,
}

/// IP settings sent to an EoE slave at start-up ("Set IP Parameter").
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EoeCfg {
    pub enabled: bool,
    pub mac: String,
    pub ip: String,
    pub subnet: String,
    pub gateway: String,
    pub dns: String,
    pub dns_name: String,
}

/// "S-0-1000" / "P-0-0001" / "s-1-12" / plain number -> IDN word
/// (bit 15 = product specific, bits 12-14 = parameter set, bits 0-11 = data block number).
pub fn parse_idn(t: &str) -> Option<u16> {
    let t = t.trim();
    let parts: Vec<&str> = t.split('-').collect();
    if parts.len() == 3 {
        let kind = parts[0].trim().to_ascii_uppercase();
        let p = if kind == "P" {
            1u16
        } else if kind == "S" {
            0
        } else {
            return None;
        };
        let set: u16 = parts[1].trim().parse().ok()?;
        let num: u16 = parts[2].trim().parse().ok()?;
        if set > 7 || num > 4095 {
            return None;
        }
        return Some(p << 15 | set << 12 | num);
    }
    crate::util::parse_num(t).filter(|v| (0..=0xFFFF).contains(v)).map(|v| v as u16)
}

pub fn format_idn(idn: u16) -> String {
    format!("{}-{}-{:04}", if idn & 0x8000 != 0 { "P" } else { "S" }, (idn >> 12) & 7, idn & 0x0FFF)
}

fn parse_ipv4(t: &str) -> Option<[u8; 4]> {
    let a: Vec<u8> = t.trim().split('.').map(|x| x.parse::<u8>()).collect::<Result<Vec<_>, _>>().ok()?;
    if a.len() == 4 {
        Some([a[0], a[1], a[2], a[3]])
    } else {
        None
    }
}

fn parse_mac(t: &str) -> Option<[u8; 6]> {
    let b = crate::util::hex_to_bytes(t);
    if b.len() == 6 {
        let mut m = [0u8; 6];
        m.copy_from_slice(&b);
        Some(m)
    } else {
        None
    }
}

const F_MAC: u32 = 1;
const F_IP: u32 = 2;
const F_SUBNET: u32 = 4;
const F_GATEWAY: u32 = 8;
const F_DNS: u32 = 16;
const F_DNS_NAME: u32 = 32;

/// EoE "Set IP Parameter" request data (EoE header, flags, then the given fields;
/// addresses in network byte order, DNS name padded to 32 bytes). Empty fields are left out.
pub fn eoe_set_ip_data(c: &EoeCfg) -> Result<Vec<u8>, String> {
    let mut flags = 0u32;
    let mut body = Vec::new();
    if !c.mac.trim().is_empty() {
        body.extend_from_slice(&parse_mac(&c.mac).ok_or("MAC must be 6 hex bytes, e.g. 02:00:00:00:10:01")?);
        flags |= F_MAC;
    }
    for (txt, f, name) in [
        (&c.ip, F_IP, "IP address"),
        (&c.subnet, F_SUBNET, "subnet mask"),
        (&c.gateway, F_GATEWAY, "gateway"),
        (&c.dns, F_DNS, "DNS server"),
    ] {
        if !txt.trim().is_empty() {
            body.extend_from_slice(&parse_ipv4(txt).ok_or(format!("{} must look like 192.168.1.10", name))?);
            flags |= f;
        }
    }
    if !c.dns_name.trim().is_empty() {
        let n = c.dns_name.trim().as_bytes();
        if n.len() > 32 {
            return Err("DNS name is longer than 32 characters".into());
        }
        let mut b = n.to_vec();
        b.resize(32, 0);
        body.extend_from_slice(&b);
        flags |= F_DNS_NAME;
    }
    if flags == 0 {
        return Err("enter at least one setting".into());
    }
    // EoE header: frame type 2 (set IP parameter request), last fragment
    let mut d = vec![0x02, 0x01, 0x00, 0x00];
    d.extend_from_slice(&flags.to_le_bytes());
    d.extend_from_slice(&body);
    Ok(d)
}

/// Read EoE "Set IP Parameter" data back into settings.
pub fn eoe_parse_set_ip(d: &[u8]) -> Option<EoeCfg> {
    if d.len() < 8 || d[0] & 0x0F != 0x02 {
        return None;
    }
    let flags = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    let mut p = 8usize;
    let mut take = |n: usize| -> Option<Vec<u8>> {
        let v = d.get(p..p + n)?.to_vec();
        p += n;
        Some(v)
    };
    let ip = |b: Vec<u8>| format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]);
    let mut c = EoeCfg {
        enabled: true,
        ..Default::default()
    };
    if flags & F_MAC != 0 {
        c.mac = take(6)?.iter().map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(":");
    }
    if flags & F_IP != 0 {
        c.ip = ip(take(4)?);
    }
    if flags & F_SUBNET != 0 {
        c.subnet = ip(take(4)?);
    }
    if flags & F_GATEWAY != 0 {
        c.gateway = ip(take(4)?);
    }
    if flags & F_DNS != 0 {
        c.dns = ip(take(4)?);
    }
    if flags & F_DNS_NAME != 0 {
        c.dns_name = String::from_utf8_lossy(&take(32)?).trim_end_matches('\0').to_string();
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idn_and_ip() {
        assert_eq!(parse_idn("S-0-1000"), Some(1000));
        assert_eq!(parse_idn("P-0-0001"), Some(0x8001));
        assert_eq!(parse_idn("s-2-15"), Some(2 << 12 | 15));
        assert_eq!(parse_idn("S-9-1"), None);
        assert_eq!(format_idn(0x8001), "P-0-0001");
        let c = EoeCfg {
            enabled: true,
            mac: "02:00:00:00:10:05".into(),
            ip: "192.168.10.5".into(),
            subnet: "255.255.255.0".into(),
            gateway: "".into(),
            dns: "".into(),
            dns_name: "drive5".into(),
        };
        let d = eoe_set_ip_data(&c).unwrap();
        assert_eq!(&d[..4], &[2, 1, 0, 0]);
        assert_eq!(eoe_parse_set_ip(&d).unwrap(), c);
        assert!(eoe_set_ip_data(&EoeCfg {
            ip: "300.1.1.1".into(),
            ..Default::default()
        })
        .is_err());
    }
}
