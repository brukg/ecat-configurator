//! Online commissioning of single slaves on a live bus: state changes, status and error
//! counters, EEPROM read/write and CoE SDO upload/download. Bus access needs Linux and
//! CAP_NET_RAW (same as the scan). The frame encoding is plain code and unit-tested.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::mpsc::{channel, Receiver};

/// What to do with the slave at auto-increment position `pos`.
#[derive(Clone, Debug)]
pub enum Request {
    Status,
    SetState(u8),
    ReadEeprom,
    WriteEeprom(Vec<u8>),
    SdoUpload {
        index: u16,
        sub: u8,
    },
    SdoDownload {
        index: u16,
        sub: u8,
        data: Vec<u8>,
    },
    /// Firmware download over FoE in BOOT state (bootstrap mailbox).
    FoeWrite {
        file_name: String,
        password: u32,
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub al_status: u16,
    pub al_code: u16,
    /// 0x0300..0x0313: RX error counters, forwarded errors, processing/PDI errors, lost links.
    pub counters: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum Reply {
    Status(Status),
    State(u16),
    Eeprom(Vec<u8>),
    Written(usize),
    Sdo(Vec<u8>),
    SdoWritten,
    FoeWritten(usize),
}

pub enum OnlineMsg {
    Progress(String),
    Done(Result<Reply, String>),
}

pub fn al_code_text(code: u16) -> &'static str {
    match code {
        0x0000 => "no error",
        0x0001 => "unspecified error",
        0x0002 => "no memory",
        0x0011 => "invalid requested state change",
        0x0012 => "unknown requested state",
        0x0013 => "bootstrap not supported",
        0x0014 => "no valid firmware",
        0x0015 => "invalid mailbox configuration (bootstrap)",
        0x0016 => "invalid mailbox configuration",
        0x0017 => "invalid sync manager configuration",
        0x0018 => "no valid inputs available",
        0x0019 => "no valid outputs",
        0x001A => "synchronization error",
        0x001B => "sync manager watchdog",
        0x001C => "invalid sync manager types",
        0x001D => "invalid output configuration",
        0x001E => "invalid input configuration",
        0x001F => "invalid watchdog configuration",
        0x0020 => "slave needs cold start",
        0x0021 => "slave needs INIT",
        0x0022 => "slave needs PREOP",
        0x0023 => "slave needs SAFEOP",
        0x0024 => "invalid input mapping",
        0x0025 => "invalid output mapping",
        0x0026 => "inconsistent settings",
        0x0027 => "free run not supported",
        0x0028 => "synchronization not supported",
        0x0029 => "free run needs 3-buffer mode",
        0x002A => "background watchdog",
        0x002B => "no valid inputs and outputs",
        0x002C => "fatal sync error",
        0x002D => "no sync error",
        0x0030 => "invalid DC SYNC configuration",
        0x0031 => "invalid DC latch configuration",
        0x0032 => "PLL error",
        0x0033 => "DC sync IO error",
        0x0034 => "DC sync timeout error",
        0x0035 => "DC invalid sync cycle time",
        0x0036 => "DC SYNC0 cycle time",
        0x0037 => "DC SYNC1 cycle time",
        0x0041 => "MBX_AOE",
        0x0042 => "MBX_EOE",
        0x0043 => "MBX_COE",
        0x0044 => "MBX_FOE",
        0x0045 => "MBX_SOE",
        0x004F => "MBX_VOE",
        0x0050 => "EEPROM no access",
        0x0051 => "EEPROM error",
        0x0060 => "slave restarted locally",
        0x0061 => "device identification value updated",
        _ => "vendor specific or unknown",
    }
}

pub fn sdo_abort_text(code: u32) -> &'static str {
    match code {
        0x0503_0000 => "toggle bit not changed",
        0x0504_0000 => "SDO protocol timeout",
        0x0504_0001 => "client/server command specifier not valid",
        0x0504_0005 => "out of memory",
        0x0601_0000 => "unsupported access to an object",
        0x0601_0001 => "attempt to read a write-only object",
        0x0601_0002 => "attempt to write a read-only object",
        0x0602_0000 => "object does not exist",
        0x0604_0041 => "object cannot be mapped to the PDO",
        0x0604_0042 => "PDO length exceeded",
        0x0604_0043 => "general parameter incompatibility",
        0x0606_0000 => "access failed due to a hardware error",
        0x0607_0010 => "data type does not match, length does not match",
        0x0607_0012 => "data type does not match, length too high",
        0x0607_0013 => "data type does not match, length too low",
        0x0609_0011 => "subindex does not exist",
        0x0609_0030 => "value range exceeded",
        0x0609_0031 => "value too high",
        0x0609_0032 => "value too low",
        0x0800_0000 => "general error",
        0x0800_0020 => "data cannot be transferred or stored",
        0x0800_0021 => "data cannot be transferred because of local control",
        0x0800_0022 => "data cannot be transferred in the present device state",
        0x0800_0023 => "no object dictionary present",
        _ => "unknown abort code",
    }
}

const MBX_COE: u8 = 3;
const MBX_FOE: u8 = 4;

/// FoE packet in a mailbox: header (6 bytes) + op code + reserved + rest.
fn foe_mailbox(counter: u8, op: u8, word: u32, rest: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(12 + rest.len());
    m.extend_from_slice(&((6 + rest.len()) as u16).to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes());
    m.push(0);
    m.push(MBX_FOE | ((counter & 7) << 4));
    m.push(op);
    m.push(0);
    m.extend_from_slice(&word.to_le_bytes());
    m.extend_from_slice(rest);
    m
}

/// FoE write request (WRQ) with password and file name.
pub fn foe_write_request(counter: u8, password: u32, file_name: &str) -> Vec<u8> {
    foe_mailbox(counter, 2, password, file_name.as_bytes())
}

/// FoE data packet `no` (counting from 1).
pub fn foe_data(counter: u8, no: u32, data: &[u8]) -> Vec<u8> {
    foe_mailbox(counter, 3, no, data)
}

/// Data bytes that fit in one FoE packet for a mailbox of `mbx_len` bytes.
pub fn foe_chunk(mbx_len: u16) -> usize {
    (mbx_len as usize).saturating_sub(12).max(1)
}

#[derive(Debug, PartialEq)]
pub enum FoeAnswer {
    Ack(u32),
    Busy { done: u16, entire: u16 },
    Error(String),
}

pub fn parse_foe(m: &[u8]) -> Result<FoeAnswer, String> {
    if m.len() < 12 {
        return Err("FoE answer too short".into());
    }
    if m[5] & 0x0F != MBX_FOE {
        return Err(format!("unexpected mailbox type {} (expected FoE)", m[5] & 0x0F));
    }
    let len = (u16::from_le_bytes([m[0], m[1]]) as usize).min(m.len() - 6);
    let w = u32::from_le_bytes([m[8], m[9], m[10], m[11]]);
    match m[6] {
        4 => Ok(FoeAnswer::Ack(w)),
        5 => {
            let text = String::from_utf8_lossy(&m[12..(6 + len).max(12)]).trim_end_matches('\0').to_string();
            Ok(FoeAnswer::Error(format!(
                "FoE error {:#x}{}",
                w,
                if text.is_empty() { String::new() } else { format!(": {}", text) }
            )))
        }
        6 => Ok(FoeAnswer::Busy {
            done: u16::from_le_bytes([m[8], m[9]]),
            entire: u16::from_le_bytes([m[10], m[11]]),
        }),
        op => Err(format!("unexpected FoE op code {}", op)),
    }
}
const COE_SDO_REQUEST: u16 = 2 << 12;

fn mailbox(counter: u8, coe_payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(6 + 2 + coe_payload.len());
    m.extend_from_slice(&((2 + coe_payload.len()) as u16).to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes()); // station address
    m.push(0); // channel / priority
    m.push(MBX_COE | ((counter & 7) << 4));
    m.extend_from_slice(&COE_SDO_REQUEST.to_le_bytes());
    m.extend_from_slice(coe_payload);
    m
}

/// Mailbox bytes of an SDO upload (read) request.
pub fn sdo_upload_request(counter: u8, index: u16, sub: u8) -> Vec<u8> {
    let mut p = vec![0x40];
    p.extend_from_slice(&index.to_le_bytes());
    p.push(sub);
    p.extend_from_slice(&[0; 4]);
    mailbox(counter, &p)
}

/// Mailbox bytes of an SDO download (write) request: expedited up to 4 bytes, normal above.
pub fn sdo_download_request(counter: u8, index: u16, sub: u8, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    if !data.is_empty() && data.len() <= 4 {
        p.push(0x23 | (((4 - data.len()) as u8) << 2));
        p.extend_from_slice(&index.to_le_bytes());
        p.push(sub);
        let mut d = data.to_vec();
        d.resize(4, 0);
        p.extend_from_slice(&d);
    } else {
        p.push(0x21);
        p.extend_from_slice(&index.to_le_bytes());
        p.push(sub);
        p.extend_from_slice(&(data.len() as u32).to_le_bytes());
        p.extend_from_slice(data);
    }
    mailbox(counter, &p)
}

/// Parse an SDO response mailbox. Ok(data) for uploads, Ok(empty) for download confirmations.
pub fn parse_sdo_response(m: &[u8]) -> Result<Vec<u8>, String> {
    if m.len() < 16 {
        return Err("mailbox response too short".into());
    }
    let len = u16::from_le_bytes([m[0], m[1]]) as usize;
    if m[5] & 0x0F != MBX_COE {
        return Err(format!("unexpected mailbox type {}", m[5] & 0x0F));
    }
    let svc = u16::from_le_bytes([m[6], m[7]]) >> 12;
    let b0 = m[8];
    if b0 == 0x80 {
        let code = u32::from_le_bytes([m[12], m[13], m[14], m[15]]);
        return Err(format!("SDO abort {:#010x}: {}", code, sdo_abort_text(code)));
    }
    if svc != 3 {
        return Err(format!("unexpected CoE service {}", svc));
    }
    match b0 >> 5 {
        2 => {
            // upload response
            if b0 & 0x02 != 0 {
                let n = if b0 & 0x01 != 0 { 4 - ((b0 >> 2) & 3) as usize } else { 4 };
                Ok(m[12..12 + n].to_vec())
            } else {
                let size = u32::from_le_bytes([m[12], m[13], m[14], m[15]]) as usize;
                let avail = len.saturating_sub(10).min(m.len().saturating_sub(16));
                if size > avail {
                    return Err(format!(
                        "object is {} bytes; segmented transfer is not supported yet ({} received)",
                        size, avail
                    ));
                }
                Ok(m[16..16 + size].to_vec())
            }
        }
        3 => Ok(vec![]), // download response
        _ => Err(format!("unexpected SDO response {:#04x}", b0)),
    }
}

pub fn start(adapter: String, pos: u16, req: Request) -> Receiver<OnlineMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = run(&adapter, pos, req, &tx);
        let _ = tx.send(OnlineMsg::Done(r));
    });
    rx
}

#[cfg(not(target_os = "linux"))]
fn run(_a: &str, _p: u16, _r: Request, _tx: &std::sync::mpsc::Sender<OnlineMsg>) -> Result<Reply, String> {
    Err("online commissioning is only available on Linux in this version".into())
}

#[cfg(target_os = "linux")]
fn run(adapter: &str, pos: u16, req: Request, tx: &std::sync::mpsc::Sender<OnlineMsg>) -> Result<Reply, String> {
    use crate::model::{CMD_APRD, CMD_APWR};
    use crate::scan::raw::Sock;
    use std::time::{Duration, Instant};
    if adapter.is_empty() {
        return Err("no network adapter selected".into());
    }
    let mac = crate::scan::list_adapters()
        .into_iter()
        .find(|a| a.0 == adapter)
        .map(|a| crate::scan::parse_mac(&a.1))
        .unwrap_or([2, 0, 0, 0, 0, 1]);
    let mut s = Sock::open(adapter, mac)?;
    let adp = 0u16.wrapping_sub(pos);
    let rd = |s: &mut Sock, ado: u16, n: usize| -> Result<Vec<u8>, String> {
        let (d, w) = s.txrx(CMD_APRD, adp, ado, &vec![0; n])?;
        if w == 0 {
            return Err(format!("slave at position {} did not answer", pos));
        }
        Ok(d)
    };
    let wr = |s: &mut Sock, ado: u16, d: &[u8]| -> Result<(), String> {
        let (_, w) = s.txrx(CMD_APWR, adp, ado, d)?;
        if w == 0 {
            return Err(format!("slave at position {} did not accept the write to {:#06x}", pos, ado));
        }
        Ok(())
    };
    let sii_wait = |s: &mut Sock| -> Result<(), String> {
        let t = Instant::now();
        loop {
            let st = rd(s, 0x0502, 2)?;
            let v = u16::from_le_bytes([st[0], st[1]]);
            if v & 0x8000 == 0 {
                if v & 0x6000 != 0 {
                    return Err(format!("EEPROM error (status {:#06x})", v));
                }
                return Ok(());
            }
            if t.elapsed() > Duration::from_millis(200) {
                return Err("EEPROM busy (timeout)".into());
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    };
    let sii_read = |s: &mut Sock, word: u32| -> Result<[u8; 4], String> {
        let w = word.to_le_bytes();
        wr(s, 0x0502, &[0x00, 0x01, w[0], w[1], w[2], w[3]])?;
        sii_wait(s)?;
        let d = rd(s, 0x0508, 4)?;
        Ok([d[0], d[1], d[2], d[3]])
    };
    let state_req = |s: &mut Sock, st: u8| -> Result<u16, String> {
        wr(s, 0x0120, &[st | 0x10, 0])?;
        let t = Instant::now();
        loop {
            let d = rd(s, 0x0130, 2)?;
            let al = u16::from_le_bytes([d[0], d[1]]);
            if al & 0x0F == st as u16 {
                return Ok(al);
            }
            if al & 0x10 != 0 || t.elapsed() > Duration::from_secs(5) {
                let c = rd(s, 0x0134, 2)?;
                let code = u16::from_le_bytes([c[0], c[1]]);
                return Err(format!(
                    "state change refused: AL status {:#06x}, code {:#06x} ({})",
                    al,
                    code,
                    al_code_text(code)
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    // EEPROM to EtherCAT
    let _ = wr(&mut s, 0x0500, &[0x00]);
    match req {
        Request::Status => {
            let d = rd(&mut s, 0x0130, 6)?;
            let c = rd(&mut s, 0x0300, 20)?;
            Ok(Reply::Status(Status {
                al_status: u16::from_le_bytes([d[0], d[1]]),
                al_code: u16::from_le_bytes([d[4], d[5]]),
                counters: c,
            }))
        }
        Request::SetState(st) => Ok(Reply::State(state_req(&mut s, st)?)),
        Request::ReadEeprom => {
            let size_word = sii_read(&mut s, 0x3E)?;
            let kbit = u16::from_le_bytes([size_word[0], size_word[1]]) as usize + 1;
            let bytes = (kbit * 128).clamp(128, 64 * 1024);
            let mut out = Vec::with_capacity(bytes);
            let mut w = 0u32;
            while out.len() < bytes {
                out.extend_from_slice(&sii_read(&mut s, w)?);
                w += 2;
                if w % 256 == 0 {
                    let _ = tx.send(OnlineMsg::Progress(format!("EEPROM read {} of {} bytes", out.len(), bytes)));
                }
            }
            out.truncate(bytes);
            Ok(Reply::Eeprom(out))
        }
        Request::WriteEeprom(data) => {
            for (k, ch) in data.chunks(2).enumerate() {
                let word = [ch[0], *ch.get(1).unwrap_or(&0xFF)];
                wr(&mut s, 0x0508, &word)?;
                let a = (k as u32).to_le_bytes();
                wr(&mut s, 0x0502, &[0x01, 0x02, a[0], a[1], a[2], a[3]])?;
                sii_wait(&mut s)?;
                if k % 128 == 0 {
                    let _ = tx.send(OnlineMsg::Progress(format!("EEPROM write {} of {} bytes", k * 2, data.len())));
                }
            }
            Ok(Reply::Written(data.len()))
        }
        Request::FoeWrite { file_name, password, data } => {
            // bootstrap mailbox from the EEPROM (words 0x14..0x17), then BOOT
            let a = sii_read(&mut s, 0x14)?;
            let b = sii_read(&mut s, 0x16)?;
            let (out_start, out_len) = (u16::from_le_bytes([a[0], a[1]]), u16::from_le_bytes([a[2], a[3]]));
            let (in_start, in_len) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
            if out_len < 16 || in_len < 16 {
                return Err("the slave has no bootstrap mailbox in its EEPROM (no FoE firmware update)".into());
            }
            state_req(&mut s, 1)?;
            wr(&mut s, 0x0800, &crate::model::sm_bytes(out_start, out_len, 0x26, true))?;
            wr(&mut s, 0x0808, &crate::model::sm_bytes(in_start, in_len, 0x22, true))?;
            state_req(&mut s, 3)?;
            let mbx = |s: &mut Sock, m: Vec<u8>, counter: &mut u8| -> Result<FoeAnswer, String> {
                *counter = if *counter >= 7 { 1 } else { *counter + 1 };
                let mut m = m;
                m[5] = (m[5] & 0x0F) | (*counter << 4);
                m.resize(out_len as usize, 0);
                wr(s, out_start, &m)?;
                let t = Instant::now();
                loop {
                    let st = rd(s, 0x080D, 1)?;
                    if st[0] & 0x08 != 0 {
                        break;
                    }
                    if t.elapsed() > Duration::from_secs(10) {
                        return Err("no FoE answer from the slave (timeout)".into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                parse_foe(&rd(s, in_start, in_len as usize)?)
            };
            let mut counter = 0u8;
            let chunk = foe_chunk(out_len);
            match mbx(&mut s, foe_write_request(0, password, &file_name), &mut counter)? {
                FoeAnswer::Ack(_) => {}
                FoeAnswer::Error(e) => return Err(e),
                FoeAnswer::Busy { .. } => {}
            }
            let mut no = 1u32;
            let mut sent = 0usize;
            loop {
                let end = (sent + chunk).min(data.len());
                let part = &data[sent..end];
                loop {
                    match mbx(&mut s, foe_data(0, no, part), &mut counter)? {
                        FoeAnswer::Ack(_) => break,
                        FoeAnswer::Busy { done, entire } => {
                            let _ = tx.send(OnlineMsg::Progress(format!("slave busy ({} of {})", done, entire)));
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        FoeAnswer::Error(e) => return Err(e),
                    }
                }
                sent = end;
                if no % 16 == 0 {
                    let _ = tx.send(OnlineMsg::Progress(format!("firmware {} of {} bytes", sent, data.len())));
                }
                // a packet shorter than the chunk ends the transfer (an empty one if it divides evenly)
                if part.len() < chunk {
                    break;
                }
                no += 1;
            }
            Ok(Reply::FoeWritten(data.len()))
        }
        Request::SdoUpload { .. } | Request::SdoDownload { .. } => {
            // standard mailbox from the slave's EEPROM (words 0x18..0x1B), then PREOP
            let a = sii_read(&mut s, 0x18)?;
            let b = sii_read(&mut s, 0x1A)?;
            let (out_start, out_len) = (u16::from_le_bytes([a[0], a[1]]), u16::from_le_bytes([a[2], a[3]]));
            let (in_start, in_len) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
            if out_len < 16 || in_len < 16 {
                return Err("the slave has no standard mailbox (CoE) in its EEPROM".into());
            }
            let al = rd(&mut s, 0x0130, 2)?;
            if al[0] & 0x0F == 1 {
                wr(&mut s, 0x0800, &crate::model::sm_bytes(out_start, out_len, 0x26, true))?;
                wr(&mut s, 0x0808, &crate::model::sm_bytes(in_start, in_len, 0x22, true))?;
                state_req(&mut s, 2)?;
            }
            // clear a pending answer
            let st = rd(&mut s, 0x080D, 1)?;
            if st[0] & 0x08 != 0 {
                let _ = rd(&mut s, in_start, in_len as usize)?;
            }
            let mut m = match &req {
                Request::SdoUpload { index, sub } => sdo_upload_request(1, *index, *sub),
                Request::SdoDownload { index, sub, data } => sdo_download_request(1, *index, *sub, data),
                _ => unreachable!(),
            };
            if m.len() > out_len as usize {
                return Err(format!(
                    "value too large for the mailbox ({} bytes); segmented transfer is not supported yet",
                    m.len()
                ));
            }
            m.resize(out_len as usize, 0);
            wr(&mut s, out_start, &m)?;
            let t = Instant::now();
            loop {
                let st = rd(&mut s, 0x080D, 1)?;
                if st[0] & 0x08 != 0 {
                    break;
                }
                if t.elapsed() > Duration::from_secs(3) {
                    return Err("no SDO answer from the slave (timeout)".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let resp = rd(&mut s, in_start, in_len as usize)?;
            let data = parse_sdo_response(&resp)?;
            Ok(match req {
                Request::SdoUpload { .. } => Reply::Sdo(data),
                _ => Reply::SdoWritten,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdo_frames() {
        let r = sdo_upload_request(1, 0x6060, 0);
        assert_eq!(r, vec![10, 0, 0, 0, 0, 0x13, 0x00, 0x20, 0x40, 0x60, 0x60, 0x00, 0, 0, 0, 0]);
        let d = sdo_download_request(2, 0x6081, 0, &[0x10, 0x27, 0, 0]);
        assert_eq!(d[8], 0x23);
        assert_eq!(&d[12..16], &[0x10, 0x27, 0, 0]);
        assert_eq!(sdo_download_request(1, 0x6060, 0, &[8])[8], 0x2F);
        // expedited upload response with 1 byte
        let mut resp = vec![10, 0, 0, 0, 0, 0x13, 0x00, 0x30, 0x4F, 0x60, 0x60, 0x00, 8, 0, 0, 0];
        assert_eq!(parse_sdo_response(&resp).unwrap(), vec![8]);
        // normal upload response with 5 bytes
        resp = vec![
            15, 0, 0, 0, 0, 0x13, 0x00, 0x30, 0x41, 0x08, 0x10, 0x00, 5, 0, 0, 0, b'S', b'D', b'2', b'0', b'0',
        ];
        assert_eq!(parse_sdo_response(&resp).unwrap(), b"SD200".to_vec());
        // abort
        resp = vec![10, 0, 0, 0, 0, 0x13, 0x00, 0x20, 0x80, 0x00, 0x20, 0x00, 0x00, 0x00, 0x02, 0x06];
        let e = parse_sdo_response(&resp).unwrap_err();
        assert!(e.contains("object does not exist"), "{}", e);
        // download confirmation
        resp = vec![10, 0, 0, 0, 0, 0x13, 0x00, 0x30, 0x60, 0x81, 0x60, 0x00, 0, 0, 0, 0];
        assert!(parse_sdo_response(&resp).unwrap().is_empty());
        assert_eq!(al_code_text(0x001B), "sync manager watchdog");
        // FoE
        let w = foe_write_request(1, 0x1234, "fw.efw");
        assert_eq!(&w[..12], &[12, 0, 0, 0, 0, 0x14, 2, 0, 0x34, 0x12, 0, 0]);
        assert_eq!(&w[12..], b"fw.efw");
        let d = foe_data(2, 3, &[9, 9]);
        assert_eq!((d[6], d[8], d.len()), (3, 3, 14));
        assert_eq!(foe_chunk(128), 116);
        assert_eq!(parse_foe(&[6, 0, 0, 0, 0, 0x14, 4, 0, 5, 0, 0, 0]).unwrap(), FoeAnswer::Ack(5));
        assert_eq!(
            parse_foe(&[6, 0, 0, 0, 0, 0x14, 6, 0, 3, 0, 10, 0]).unwrap(),
            FoeAnswer::Busy { done: 3, entire: 10 }
        );
        let mut e = vec![10, 0, 0, 0, 0, 0x14, 5, 0, 0x02, 0x80, 0, 0];
        e.extend_from_slice(b"bad!");
        assert_eq!(parse_foe(&e).unwrap(), FoeAnswer::Error("FoE error 0x8002: bad!".into()));
    }
}
