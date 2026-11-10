//! SII (slave EEPROM) image generation from an ESI device description (ETG.2010 layout).

use crate::esi::*;

pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

struct Strings(Vec<String>);
impl Strings {
    fn idx(&mut self, s: &str) -> u8 {
        if s.is_empty() {
            return 0;
        }
        if let Some(i) = self.0.iter().position(|x| x == s) {
            return (i + 1) as u8;
        }
        if self.0.len() >= 255 {
            return 0;
        }
        self.0.push(s.chars().take(255).collect());
        self.0.len() as u8
    }
}

fn category(out: &mut Vec<u8>, kind: u16, mut data: Vec<u8>) {
    if data.len() % 2 == 1 {
        data.push(0);
    }
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&((data.len() / 2) as u16).to_le_bytes());
    out.extend_from_slice(&data);
}

/// Build the EEPROM content. Returns the bytes, sized to the ESI ByteSize when given.
pub fn build_sii(dev: &EsiDevice, serial: u32, alias: u16) -> Vec<u8> {
    let mut e = vec![0u8; 128];
    let ee = dev.eeprom.clone().unwrap_or_default();
    // words 0..6: config area (PDI control, PDI config, sync impulse, PDI config 2, alias)
    for (i, b) in ee.config_data.iter().take(14).enumerate() {
        e[i] = *b;
    }
    if alias != 0 {
        e[8..10].copy_from_slice(&alias.to_le_bytes());
    }
    e[14] = crc8(&e[0..14]);
    let put32 = |e: &mut Vec<u8>, w: usize, v: u32| e[w * 2..w * 2 + 4].copy_from_slice(&v.to_le_bytes());
    let put16 = |e: &mut Vec<u8>, w: usize, v: u16| e[w * 2..w * 2 + 2].copy_from_slice(&v.to_le_bytes());
    put32(&mut e, 0x08, dev.vendor_id);
    put32(&mut e, 0x0A, dev.product_code);
    put32(&mut e, 0x0C, dev.revision);
    put32(&mut e, 0x0E, serial);
    // bootstrap mailbox (words 0x14..0x17)
    if ee.boot_strap.len() >= 8 {
        for (i, b) in ee.boot_strap.iter().take(8).enumerate() {
            e[0x14 * 2 + i] = *b;
        }
    }
    // standard mailbox (words 0x18..0x1B) and protocols (0x1C)
    let mo = dev.sms.iter().find(|s| s.kind == SmKind::MBoxOut);
    let mi = dev.sms.iter().find(|s| s.kind == SmKind::MBoxIn);
    if let (Some(o), Some(i)) = (mo, mi) {
        put16(&mut e, 0x18, o.start);
        put16(&mut e, 0x19, if o.default_size > 0 { o.default_size } else { o.max_size.unwrap_or(128) });
        put16(&mut e, 0x1A, i.start);
        put16(&mut e, 0x1B, if i.default_size > 0 { i.default_size } else { i.max_size.unwrap_or(128) });
    }
    put16(&mut e, 0x1C, dev.mailbox.as_ref().map(|m| m.protocol_word()).unwrap_or(0));
    let size_kbit = ((ee.byte_size.max(128) * 8) / 1024).max(1) as u16;
    put16(&mut e, 0x3E, size_kbit - 1);
    put16(&mut e, 0x3F, 1);

    // categories
    let mut strings = Strings(Vec::new());
    let group_idx = strings.idx(&dev.group_type);
    let img_idx = 0u8;
    let order_idx = strings.idx(&dev.type_name);
    let name_idx = strings.idx(&dev.name);
    let mut general = vec![0u8; 32];
    general[0] = group_idx;
    general[1] = img_idx;
    general[2] = order_idx;
    general[3] = name_idx;
    if let Some(c) = dev.coe() {
        general[5] = 0x01
            | (c.sdo_info as u8) << 1
            | (c.pdo_assign as u8) << 2
            | (c.pdo_config as u8) << 3
            | 0x10 // upload at startup
            | (c.complete_access as u8) << 5;
    }
    if dev.mailbox.as_ref().map(|m| m.foe).unwrap_or(false) {
        general[6] = 1;
    }
    if dev.mailbox.as_ref().map(|m| m.eoe).unwrap_or(false) {
        general[7] = 1;
    }
    if dev.profile_no == Some(402) {
        general[9] = 1;
    }
    general[14] = group_idx;
    // physical ports: 4 bits per port, 1 = MII (Y), 2 = reserved, 3 = EBUS (E/K)
    let mut ports: u16 = 0;
    for (k, ch) in dev.physics.chars().take(4).enumerate() {
        let code = match ch {
            'Y' | 'H' => 0x1,
            'E' | 'K' => 0x3,
            _ => 0x0,
        };
        ports |= code << (4 * k);
    }
    general[16..18].copy_from_slice(&ports.to_le_bytes());

    let fmmu: Vec<u8> = dev
        .fmmus
        .iter()
        .map(|f| match f.as_str() {
            "Outputs" => 1,
            "Inputs" => 2,
            "MBoxState" => 3,
            _ => 0,
        })
        .collect();
    let mut syncm = Vec::new();
    for s in dev.sms.iter() {
        syncm.extend_from_slice(&s.start.to_le_bytes());
        syncm.extend_from_slice(&s.default_size.to_le_bytes());
        syncm.push(s.control);
        syncm.push(0);
        syncm.push(s.enable as u8);
        syncm.push(s.kind.sii_code());
    }
    let pdo_cat = |strings: &mut Strings, list: &[Pdo]| {
        let mut d = Vec::new();
        for p in list {
            d.extend_from_slice(&p.index.to_le_bytes());
            d.push(p.entries.len() as u8);
            d.push(p.sm.unwrap_or(0xFF));
            d.push(0);
            d.push(strings.idx(&p.name));
            let flags: u16 = (p.mandatory as u16) | (p.fixed as u16) << 4;
            d.extend_from_slice(&flags.to_le_bytes());
            for en in p.entries.iter() {
                d.extend_from_slice(&en.index.to_le_bytes());
                d.push(en.subindex);
                d.push(strings.idx(&en.name));
                d.push(crate::util::datatype_code(&en.data_type));
                d.push(en.bitlen as u8);
                d.extend_from_slice(&0u16.to_le_bytes());
            }
        }
        d
    };
    let tx = pdo_cat(&mut strings, &dev.tx_pdos);
    let rx = pdo_cat(&mut strings, &dev.rx_pdos);
    let mut sd = vec![strings.0.len() as u8];
    for s in strings.0.iter() {
        let b: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        sd.push(b.len() as u8);
        sd.extend_from_slice(&b);
    }
    category(&mut e, 10, sd);
    category(&mut e, 30, general);
    if !fmmu.is_empty() {
        category(&mut e, 40, fmmu);
    }
    if !syncm.is_empty() {
        category(&mut e, 41, syncm);
    }
    if !tx.is_empty() {
        category(&mut e, 50, tx);
    }
    if !rx.is_empty() {
        category(&mut e, 51, rx);
    }
    e.extend_from_slice(&[0xFF, 0xFF]);
    let target = ee.byte_size as usize;
    if target > e.len() {
        e.resize(target, 0xFF);
    }
    e
}

pub fn hex_dump(b: &[u8]) -> String {
    let mut s = String::new();
    for (i, chunk) in b.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|x| format!("{:02X}", x)).collect();
        let asc: String = chunk.iter().map(|&c| if (32..127).contains(&c) { c as char } else { '.' }).collect();
        s.push_str(&format!("{:04X}  {:<48}  {}\n", i * 16, hex.join(" "), asc));
    }
    s
}

/// Intel HEX output for EEPROM programmers.
pub fn intel_hex(b: &[u8]) -> String {
    let mut s = String::new();
    for (i, chunk) in b.chunks(16).enumerate() {
        let addr = (i * 16) as u16;
        let mut rec = vec![chunk.len() as u8, (addr >> 8) as u8, addr as u8, 0];
        rec.extend_from_slice(chunk);
        let sum: u8 = rec.iter().fold(0u8, |a, x| a.wrapping_add(*x));
        rec.push(0u8.wrapping_sub(sum));
        s.push(':');
        for x in rec {
            s.push_str(&format!("{:02X}", x));
        }
        s.push('\n');
    }
    s.push_str(":00000001FF\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crc() {
        // ETG example: config data 0x080C 0x0000 0xCC88 0x0013 ... checksum computed over 14 bytes
        assert_eq!(crc8(&[]), 0xFF);
        let d = parse_esi(SAMPLE_DRIVE, "x").unwrap();
        let e = build_sii(&d[0], 0, 0);
        assert_eq!(e.len(), 4096);
        assert_eq!(&e[0x10..0x14], &0x0000E1A0u32.to_le_bytes());
    }
}
