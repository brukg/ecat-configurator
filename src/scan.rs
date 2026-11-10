#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! Online bus scan over a raw Ethernet socket (Linux, needs CAP_NET_RAW or root).

use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Debug, Default)]
pub struct ScannedSlave {
    pub vendor: u32,
    pub product: u32,
    pub revision: u32,
    pub serial: u32,
    pub al_status: u16,
    pub esc_type: u8,
    pub esc_rev: u8,
    pub dl_status: u16,
    pub station: u16,
}

pub enum ScanMsg {
    Log(String),
    Done(Result<Vec<ScannedSlave>, String>),
}

pub fn al_state_name(s: u16) -> String {
    let base = match s & 0x0F {
        1 => "INIT",
        2 => "PREOP",
        3 => "BOOT",
        4 => "SAFEOP",
        8 => "OP",
        _ => "UNKNOWN",
    };
    if s & 0x10 != 0 {
        format!("{} + ERROR", base)
    } else {
        base.to_string()
    }
}

/// Network adapters with their MAC addresses.
pub fn list_adapters() -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == "lo" {
                continue;
            }
            let mac = std::fs::read_to_string(e.path().join("address")).unwrap_or_default().trim().to_string();
            v.push((name, mac));
        }
    }
    v.sort();
    v
}

pub fn start_scan(adapter: String) -> Receiver<ScanMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = scan(&adapter, &tx);
        let _ = tx.send(ScanMsg::Done(r));
    });
    rx
}

#[cfg(target_os = "linux")]
mod raw {
    use std::time::{Duration, Instant};

    pub struct Sock {
        fd: i32,
        idx: u8,
        mac: [u8; 6],
    }

    impl Drop for Sock {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.fd);
            }
        }
    }

    impl Sock {
        pub fn open(ifname: &str, mac: [u8; 6]) -> Result<Sock, String> {
            unsafe {
                let proto = (0x88A4u16).to_be() as i32;
                let fd = libc::socket(libc::AF_PACKET, libc::SOCK_RAW, proto);
                if fd < 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(if err.raw_os_error() == Some(libc::EPERM) {
                        "permission denied: the bus scan needs root rights or the CAP_NET_RAW capability".into()
                    } else {
                        format!("cannot open raw socket: {}", err)
                    });
                }
                let cname = std::ffi::CString::new(ifname).map_err(|_| "bad adapter name")?;
                let ifindex = libc::if_nametoindex(cname.as_ptr());
                if ifindex == 0 {
                    libc::close(fd);
                    return Err(format!("adapter '{}' not found", ifname));
                }
                let mut sll: libc::sockaddr_ll = std::mem::zeroed();
                sll.sll_family = libc::AF_PACKET as u16;
                sll.sll_protocol = 0x88A4u16.to_be();
                sll.sll_ifindex = ifindex as i32;
                if libc::bind(fd, &sll as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_ll>() as u32) < 0 {
                    let e = std::io::Error::last_os_error();
                    libc::close(fd);
                    return Err(format!("cannot bind to '{}': {}", ifname, e));
                }
                let tv = libc::timeval { tv_sec: 0, tv_usec: 20_000 };
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &tv as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as u32,
                );
                Ok(Sock { fd, idx: 0, mac })
            }
        }

        /// Send one datagram and wait for it to come back. Returns (data, working counter).
        pub fn txrx(&mut self, cmd: u8, adp: u16, ado: u16, data: &[u8]) -> Result<(Vec<u8>, u16), String> {
            self.idx = self.idx.wrapping_add(1);
            let n = data.len();
            let mut f = Vec::with_capacity(64 + n);
            f.extend_from_slice(&[0xFF; 6]);
            f.extend_from_slice(&self.mac);
            f.extend_from_slice(&[0x88, 0xA4]);
            let dlen = (10 + n + 2) as u16;
            f.extend_from_slice(&((dlen & 0x7FF) | 0x1000).to_le_bytes());
            f.push(cmd);
            f.push(self.idx);
            f.extend_from_slice(&adp.to_le_bytes());
            f.extend_from_slice(&ado.to_le_bytes());
            f.extend_from_slice(&((n as u16) & 0x7FF).to_le_bytes());
            f.extend_from_slice(&[0, 0]);
            f.extend_from_slice(data);
            f.extend_from_slice(&[0, 0]);
            while f.len() < 60 {
                f.push(0);
            }
            unsafe {
                if libc::send(self.fd, f.as_ptr() as *const libc::c_void, f.len(), 0) < 0 {
                    return Err(format!("send failed: {}", std::io::Error::last_os_error()));
                }
            }
            let start = Instant::now();
            let mut buf = [0u8; 1600];
            while start.elapsed() < Duration::from_millis(200) {
                let mut from: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
                let mut flen = std::mem::size_of::<libc::sockaddr_ll>() as u32;
                let r = unsafe {
                    libc::recvfrom(
                        self.fd,
                        buf.as_mut_ptr() as *mut libc::c_void,
                        buf.len(),
                        0,
                        &mut from as *mut _ as *mut libc::sockaddr,
                        &mut flen,
                    )
                };
                if r <= 0 {
                    continue;
                }
                if from.sll_pkttype == libc::PACKET_OUTGOING as u8 {
                    continue;
                }
                let r = r as usize;
                if r < 14 + 2 + 10 + n + 2 || buf[12] != 0x88 || buf[13] != 0xA4 {
                    continue;
                }
                let d = &buf[16..];
                if d[0] != cmd || d[1] != self.idx {
                    continue;
                }
                let payload = d[10..10 + n].to_vec();
                let wkc = u16::from_le_bytes([d[10 + n], d[11 + n]]);
                return Ok((payload, wkc));
            }
            Err("no reply from the bus (timeout)".into())
        }
    }
}

fn parse_mac(s: &str) -> [u8; 6] {
    let b = crate::util::hex_to_bytes(s);
    if b.len() == 6 && b.iter().any(|x| *x != 0) {
        let mut m = [0u8; 6];
        m.copy_from_slice(&b);
        m
    } else {
        [0x02, 0, 0, 0, 0, 0x01]
    }
}

#[cfg(target_os = "linux")]
fn scan(adapter: &str, log: &Sender<ScanMsg>) -> Result<Vec<ScannedSlave>, String> {
    use crate::model::{CMD_APRD, CMD_APWR, CMD_BRD};
    if adapter.is_empty() {
        return Err("no network adapter selected".into());
    }
    let mac = list_adapters()
        .into_iter()
        .find(|a| a.0 == adapter)
        .map(|a| parse_mac(&a.1))
        .unwrap_or([2, 0, 0, 0, 0, 1]);
    let mut s = raw::Sock::open(adapter, mac)?;
    let _ = log.send(ScanMsg::Log(format!("Scanning on {} ...", adapter)));
    let (_, wkc) = s.txrx(CMD_BRD, 0, 0x0000, &[0])?;
    let _ = log.send(ScanMsg::Log(format!("{} slave(s) answered the broadcast read", wkc)));
    let mut out = Vec::new();
    for i in 0..wkc as usize {
        let adp = 0u16.wrapping_sub(i as u16);
        let mut sl = ScannedSlave::default();
        if let Ok((d, _)) = s.txrx(CMD_APRD, adp, 0x0000, &[0, 0]) {
            sl.esc_type = d[0];
            sl.esc_rev = d[1];
        }
        if let Ok((d, _)) = s.txrx(CMD_APRD, adp, 0x0130, &[0, 0]) {
            sl.al_status = u16::from_le_bytes([d[0], d[1]]);
        }
        if let Ok((d, _)) = s.txrx(CMD_APRD, adp, 0x0110, &[0, 0]) {
            sl.dl_status = u16::from_le_bytes([d[0], d[1]]);
        }
        if let Ok((d, _)) = s.txrx(CMD_APRD, adp, 0x0010, &[0, 0]) {
            sl.station = u16::from_le_bytes([d[0], d[1]]);
        }
        // give the EEPROM interface to EtherCAT and read the identity words
        let _ = s.txrx(CMD_APWR, adp, 0x0500, &[0x00]);
        let mut read_word = |w: u16| -> Result<u32, String> {
            let wb = w.to_le_bytes();
            s.txrx(CMD_APWR, adp, 0x0502, &[0x00, 0x01, wb[0], wb[1], 0, 0])?;
            for _ in 0..50 {
                let (st, _) = s.txrx(CMD_APRD, adp, 0x0502, &[0, 0])?;
                if st[1] & 0x80 == 0 {
                    let (d, _) = s.txrx(CMD_APRD, adp, 0x0508, &[0; 4])?;
                    return Ok(u32::from_le_bytes([d[0], d[1], d[2], d[3]]));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err("EEPROM busy".into())
        };
        sl.vendor = read_word(0x0008).unwrap_or(0);
        sl.product = read_word(0x000A).unwrap_or(0);
        sl.revision = read_word(0x000C).unwrap_or(0);
        sl.serial = read_word(0x000E).unwrap_or(0);
        let _ = log.send(ScanMsg::Log(format!(
            "  #{}: vendor {:#010x} product {:#010x} rev {:#010x} state {}",
            i,
            sl.vendor,
            sl.product,
            sl.revision,
            al_state_name(sl.al_status)
        )));
        out.push(sl);
    }
    Ok(out)
}

#[cfg(not(target_os = "linux"))]
fn scan(_adapter: &str, _log: &Sender<ScanMsg>) -> Result<Vec<ScannedSlave>, String> {
    let _ = parse_mac;
    Err("the online bus scan is only available on Linux in this version".into())
}
