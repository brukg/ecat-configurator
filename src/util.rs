//! Small helpers: number parsing in ESI/ENI notation, hex strings.

/// Parse a number in ESI/ENI notation: `#x1A00`, `0x1A00`, or decimal (may be negative).
pub fn parse_num(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let (neg, t) = if let Some(r) = t.strip_prefix('-') { (true, r) } else { (false, t) };
    let v = if let Some(h) = t.strip_prefix("#x").or_else(|| t.strip_prefix("#X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else {
        t.parse::<i64>().ok()?
    };
    Some(if neg { -v } else { v })
}

pub fn parse_u32(s: &str) -> u32 {
    parse_num(s).map(|v| v as u32).unwrap_or(0)
}

pub fn parse_bool(s: &str) -> bool {
    matches!(s.trim(), "1" | "true" | "TRUE" | "True")
}

/// Bytes from a hex string such as `0a1B ff`.
pub fn hex_to_bytes(s: &str) -> Vec<u8> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let mut out = Vec::new();
    let b = clean.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if let Ok(v) = u8::from_str_radix(&clean[i..i + 2], 16) {
            out.push(v);
        }
        i += 2;
    }
    out
}

pub fn bytes_to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

pub fn bytes_to_hex_spaced(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")
}

pub fn hx(v: u32, w: usize) -> String {
    format!("#x{:0w$X}", v, w = w)
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Byte size of a CoE data type name.
pub fn datatype_bits(dt: &str) -> Option<u16> {
    Some(match dt.to_ascii_uppercase().as_str() {
        "BOOL" | "BIT" => 1,
        "SINT" | "USINT" | "BYTE" | "INT8" | "UINT8" => 8,
        "INT" | "UINT" | "WORD" | "INT16" | "UINT16" => 16,
        "DINT" | "UDINT" | "DWORD" | "REAL" | "INT32" | "UINT32" | "REAL32" => 32,
        "LINT" | "ULINT" | "LWORD" | "LREAL" | "INT64" | "UINT64" | "REAL64" => 64,
        _ => return None,
    })
}

/// CoE data type code (ETG.1000.6) used in the SII PDO entries.
pub fn datatype_code(dt: &str) -> u8 {
    match dt.to_ascii_uppercase().as_str() {
        "BOOL" | "BIT" => 0x01,
        "SINT" | "INT8" => 0x02,
        "INT" | "INT16" => 0x03,
        "DINT" | "INT32" => 0x04,
        "USINT" | "BYTE" | "UINT8" => 0x05,
        "UINT" | "WORD" | "UINT16" => 0x06,
        "UDINT" | "DWORD" | "UINT32" => 0x07,
        "REAL" | "REAL32" => 0x08,
        "LREAL" | "REAL64" => 0x11,
        "LINT" | "INT64" => 0x15,
        "ULINT" | "LWORD" | "UINT64" => 0x1B,
        _ => 0x00,
    }
}

pub fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    #[cfg(not(unix))]
    {
        let s = secs % 86400;
        tm.tm_hour = (s / 3600) as i32;
        tm.tm_min = ((s / 60) % 60) as i32;
        tm.tm_sec = (s % 60) as i32;
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nums() {
        assert_eq!(parse_num("#x1A00"), Some(0x1A00));
        assert_eq!(parse_num("100"), Some(100));
        assert_eq!(parse_num("-5"), Some(-5));
        assert_eq!(hex_to_bytes("0a FF"), vec![0x0a, 0xff]);
    }
}
