//! Network configuration model, process image layout, init command generation, validation.

use crate::esi::*;
use serde::{Deserialize, Serialize};

pub const CMD_APRD: u8 = 1;
pub const CMD_APWR: u8 = 2;
pub const CMD_FPRD: u8 = 4;
pub const CMD_FPWR: u8 = 5;
pub const CMD_BRD: u8 = 7;
pub const CMD_BWR: u8 = 8;
pub const CMD_LRD: u8 = 10;
pub const CMD_LWR: u8 = 11;
pub const CMD_LRW: u8 = 12;

pub fn cmd_name(c: u8) -> &'static str {
    match c {
        0 => "NOP",
        1 => "APRD",
        2 => "APWR",
        3 => "APRW",
        4 => "FPRD",
        5 => "FPWR",
        6 => "FPRW",
        7 => "BRD",
        8 => "BWR",
        9 => "BRW",
        10 => "LRD",
        11 => "LWR",
        12 => "LRW",
        13 => "ARMW",
        14 => "FRMW",
        _ => "?",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MasterCfg {
    pub name: String,
    pub cycle_time_us: u32,
    pub adapter: String,
    pub src_mac: String,
    pub dst_mac: String,
    pub ether_type: u16,
    pub use_lrw: bool,
    pub log_start: u32,
    pub max_frame_data: u32,
    pub timeout_init_preop_ms: u32,
    pub timeout_preop_safeop_ms: u32,
    pub timeout_safeop_op_ms: u32,
    pub timeout_mailbox_ms: u32,
    pub dc_enabled: bool,
    /// Index of the slave used as DC reference clock (None = first DC slave).
    pub ref_clock: Option<usize>,
}

impl Default for MasterCfg {
    fn default() -> Self {
        MasterCfg {
            name: "EtherCAT Master".into(),
            cycle_time_us: 1000,
            adapter: String::new(),
            src_mac: "02:00:00:00:00:01".into(),
            dst_mac: "ff:ff:ff:ff:ff:ff".into(),
            ether_type: 0x88A4,
            use_lrw: true,
            log_start: 0x0001_0000,
            max_frame_data: 1486,
            timeout_init_preop_ms: 3000,
            timeout_preop_safeop_ms: 10000,
            timeout_safeop_op_ms: 10000,
            timeout_mailbox_ms: 3000,
            dc_enabled: true,
            ref_clock: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PdoCfg {
    pub pdo: Pdo,
    pub is_tx: bool,
    pub assigned: bool,
    pub sm: u8,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RevCheck {
    None,
    Equal,
    GreaterOrEqual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlaveCfg {
    /// Stable identity used by views (tabs, windows) that refer to this slave.
    #[serde(default)]
    pub uid: u64,
    pub name: String,
    pub device: EsiDevice,
    pub phys_addr: u16,
    pub serial: u32,
    pub pdos: Vec<PdoCfg>,
    /// Mailbox sizes (out, in) in bytes.
    pub mbx_sizes: (u16, u16),
    /// Index into device.dc_opmodes (None = no DC / free run).
    pub dc_mode: Option<usize>,
    /// Sync0 cycle override in ns (0 = derive from master cycle and ESI factor).
    pub sync0_cycle_ns: u32,
    pub sync0_shift_ns: i32,
    pub sync1_cycle_ns: u32,
    pub sync1_shift_ns: i32,
    pub coe_cmds: Vec<CoeInitCmd>,
    pub check_vendor: bool,
    pub check_product: bool,
    pub check_revision: RevCheck,
    pub check_serial: bool,
    /// Upstream device: None = master, Some(i) = slave index i.
    pub parent: Option<usize>,
    /// Port of the upstream device this slave is connected to (0=A,1=B,2=C,3=D).
    pub parent_port: u8,
    pub watchdog_enabled: bool,
    pub watchdog_ms: u32,
    pub generate_pdo_cmds: bool,
}

static UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_uid() -> u64 {
    UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub fn port_letter(p: u8) -> char {
    (b'A' + p.min(3)) as char
}

impl SlaveCfg {
    pub fn from_device(dev: &EsiDevice, phys: u16) -> SlaveCfg {
        let mut pdos = Vec::new();
        let first_sm = |kind: SmKind| dev.sms.iter().position(|s| s.kind == kind).unwrap_or(0) as u8;
        for (list, is_tx, kind) in [(&dev.rx_pdos, false, SmKind::Outputs), (&dev.tx_pdos, true, SmKind::Inputs)] {
            let any_sm = list.iter().any(|p| p.sm.is_some());
            let mut chosen: Vec<u16> = Vec::new();
            for p in list.iter() {
                let wanted = if any_sm { p.sm.is_some() || p.mandatory } else { true };
                let conflict = chosen.iter().any(|c| p.excludes.contains(c));
                let assigned = wanted && (!conflict || p.mandatory);
                if assigned {
                    chosen.push(p.index);
                }
                pdos.push(PdoCfg {
                    pdo: p.clone(),
                    is_tx,
                    assigned,
                    sm: p.sm.unwrap_or_else(|| first_sm(kind)),
                });
            }
        }
        let mbx_size = |kind: SmKind| {
            dev.sms
                .iter()
                .find(|s| s.kind == kind)
                .map(|s| if s.default_size > 0 { s.default_size } else { s.max_size.unwrap_or(128) })
                .unwrap_or(0)
        };
        let dc_mode = if dev.dc_opmodes.is_empty() {
            None
        } else {
            // prefer the first mode that activates SYNC0; free run otherwise
            dev.dc_opmodes.iter().position(|m| m.assign_activate != 0).or(Some(0))
        };
        SlaveCfg {
            uid: next_uid(),
            name: format!("Slave_{} ({})", phys, dev.type_name),
            device: dev.clone(),
            phys_addr: phys,
            serial: 0,
            pdos,
            mbx_sizes: (mbx_size(SmKind::MBoxOut), mbx_size(SmKind::MBoxIn)),
            dc_mode,
            sync0_cycle_ns: 0,
            sync0_shift_ns: 0,
            sync1_cycle_ns: 0,
            sync1_shift_ns: 0,
            coe_cmds: dev.coe().map(|c| c.init_cmds.clone()).unwrap_or_default(),
            check_vendor: true,
            check_product: true,
            check_revision: RevCheck::None,
            check_serial: false,
            parent: None,
            parent_port: 1,
            watchdog_enabled: false,
            watchdog_ms: 100,
            generate_pdo_cmds: true,
        }
    }

    pub fn has_mailbox(&self) -> bool {
        self.device.mailbox.is_some() && self.device.sms.iter().any(|s| s.kind == SmKind::MBoxOut)
    }

    pub fn dc_opmode(&self) -> Option<&DcOpMode> {
        self.dc_mode.and_then(|i| self.device.dc_opmodes.get(i))
    }

    pub fn dc_active(&self) -> bool {
        self.dc_opmode().map(|m| m.assign_activate != 0).unwrap_or(false)
    }

    pub fn sync0_cycle(&self, master_cycle_us: u32) -> u32 {
        if self.sync0_cycle_ns > 0 {
            return self.sync0_cycle_ns;
        }
        let base = master_cycle_us.saturating_mul(1000);
        match self.dc_opmode().map(|m| m.cycle_sync0_factor).unwrap_or(1) {
            f if f > 0 => base.saturating_mul(f as u32),
            f if f < 0 => base / (-f) as u32,
            _ => base,
        }
    }

    pub fn sync1_cycle(&self, master_cycle_us: u32) -> u32 {
        if self.sync1_cycle_ns > 0 {
            return self.sync1_cycle_ns;
        }
        let m = match self.dc_opmode() {
            Some(m) => m,
            None => return 0,
        };
        if m.assign_activate & 0x0400 == 0 {
            return 0;
        }
        let base = master_cycle_us.saturating_mul(1000);
        match m.cycle_sync1_factor {
            f if f > 0 => base.saturating_mul(f as u32),
            f if f < 0 => base / (-f) as u32,
            _ => 0,
        }
    }

    pub fn assigned(&self, is_tx: bool) -> impl Iterator<Item = &PdoCfg> {
        self.pdos.iter().filter(move |p| p.is_tx == is_tx && p.assigned)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Project {
    pub master: MasterCfg,
    pub slaves: Vec<SlaveCfg>,
}

impl Project {
    /// Give every slave a unique, non-zero uid (projects from older files have none).
    pub fn ensure_uids(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for s in self.slaves.iter_mut() {
            if s.uid == 0 || !seen.insert(s.uid) {
                s.uid = next_uid();
                seen.insert(s.uid);
            }
        }
        if let Some(m) = self.slaves.iter().map(|s| s.uid).max() {
            UID.fetch_max(m + 1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn index_of_uid(&self, uid: u64) -> Option<usize> {
        self.slaves.iter().position(|s| s.uid == uid)
    }

    pub fn next_phys_addr(&self) -> u16 {
        let mut a = 1001u16;
        while self.slaves.iter().any(|s| s.phys_addr == a) {
            a += 1;
        }
        a
    }

    pub fn add_device(&mut self, dev: &EsiDevice, at: Option<usize>) -> usize {
        let phys = self.next_phys_addr();
        let mut s = SlaveCfg::from_device(dev, phys);
        let idx = at.map(|i| (i + 1).min(self.slaves.len())).unwrap_or(self.slaves.len());
        s.parent = if idx == 0 { None } else { Some(idx - 1) };
        // fix up parent indices of slaves after the insertion point
        for sl in self.slaves.iter_mut() {
            if let Some(p) = sl.parent {
                if p >= idx {
                    sl.parent = Some(p + 1);
                }
            }
        }
        if idx < self.slaves.len() {
            // the slave that followed now hangs on the new one
            if self.slaves[idx].parent == if idx == 0 { None } else { Some(idx - 1) } {
                self.slaves[idx].parent = Some(idx);
            }
        }
        self.slaves.insert(idx, s);
        idx
    }

    pub fn remove(&mut self, idx: usize) {
        if idx >= self.slaves.len() {
            return;
        }
        let removed_parent = self.slaves[idx].parent;
        self.slaves.remove(idx);
        for sl in self.slaves.iter_mut() {
            match sl.parent {
                Some(p) if p == idx => sl.parent = removed_parent,
                Some(p) if p > idx => sl.parent = Some(p - 1),
                _ => {}
            }
        }
        if let Some(r) = self.master.ref_clock {
            if r == idx {
                self.master.ref_clock = None;
            } else if r > idx {
                self.master.ref_clock = Some(r - 1);
            }
        }
    }

    /// Swap slave `i` with `i+1` keeping a linear chain.
    pub fn move_down(&mut self, i: usize) {
        if i + 1 >= self.slaves.len() {
            return;
        }
        self.slaves.swap(i, i + 1);
        self.relinearize();
    }

    pub fn relinearize(&mut self) {
        for (i, s) in self.slaves.iter_mut().enumerate() {
            s.parent = if i == 0 { None } else { Some(i - 1) };
            s.parent_port = 1;
        }
    }

    pub fn ref_clock_index(&self) -> Option<usize> {
        if !self.master.dc_enabled {
            return None;
        }
        match self.master.ref_clock {
            Some(i) if i < self.slaves.len() => Some(i),
            _ => self.slaves.iter().position(|s| !s.device.dc_opmodes.is_empty()),
        }
    }

    pub fn auto_inc(i: usize) -> u16 {
        0u16.wrapping_sub(i as u16)
    }
}

// ---------------------------------------------------------------- layout

#[derive(Clone, Debug)]
pub struct SmLayout {
    pub index: u8,
    pub kind: SmKind,
    pub start: u16,
    pub len: u16,
    pub control: u8,
    pub enable: bool,
    pub pdos: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct FmmuLayout {
    pub log_start: u32,
    pub len: u16,
    pub log_start_bit: u8,
    pub log_end_bit: u8,
    pub phys_start: u16,
    pub phys_start_bit: u8,
    /// 1 = read (inputs), 2 = write (outputs)
    pub dir: u8,
    pub sm: u8,
}

impl FmmuLayout {
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16);
        b.extend_from_slice(&self.log_start.to_le_bytes());
        b.extend_from_slice(&self.len.to_le_bytes());
        b.push(self.log_start_bit);
        b.push(self.log_end_bit);
        b.extend_from_slice(&self.phys_start.to_le_bytes());
        b.push(self.phys_start_bit);
        b.push(self.dir);
        b.push(1); // activate
        b.extend_from_slice(&[0, 0, 0]);
        b
    }
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub data_type: String,
    pub bit_size: u16,
    /// Bit offset inside the process image (inputs or outputs image).
    pub bit_offs: u32,
    pub is_input: bool,
    pub slave: usize,
    pub pdo: u16,
    pub index: u16,
    pub subindex: u8,
}

#[derive(Clone, Debug, Default)]
pub struct SlaveLayout {
    pub sms: Vec<SmLayout>,
    pub fmmus: Vec<FmmuLayout>,
    pub out_bytes: u32,
    pub in_bytes: u32,
    /// Byte offset of this slave's outputs in the output image / inputs in the input image.
    pub out_offs: u32,
    pub in_offs: u32,
    pub out_bits: u32,
    pub in_bits: u32,
}

#[derive(Clone, Debug, Default)]
pub struct NetLayout {
    pub slaves: Vec<SlaveLayout>,
    pub out_size: u32,
    pub in_size: u32,
    pub out_log: u32,
    pub in_log: u32,
    pub vars: Vec<Variable>,
    pub n_out_slaves: u16,
    pub n_in_slaves: u16,
}

impl NetLayout {
    pub fn lrw_wkc(&self) -> u16 {
        // per slave: +2 for a successful write, +1 for a successful read
        self.slaves
            .iter()
            .map(|s| (if s.out_bytes > 0 { 2 } else { 0 }) + (if s.in_bytes > 0 { 1 } else { 0 }))
            .sum()
    }
}

pub fn sm_bytes(start: u16, len: u16, control: u8, enable: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(8);
    b.extend_from_slice(&start.to_le_bytes());
    b.extend_from_slice(&len.to_le_bytes());
    b.push(control);
    b.push(0);
    b.push(if enable && len > 0 { 1 } else { 0 });
    b.push(0);
    b
}

pub fn compute_layout(p: &Project) -> NetLayout {
    let mut lay = NetLayout::default();
    let mut out_cursor = 0u32;
    let mut slave_out = Vec::new();
    // pass 1: SM sizes and output layout
    for s in p.slaves.iter() {
        let mut sl = SlaveLayout::default();
        for (i, sm) in s.device.sms.iter().enumerate() {
            let i = i as u8;
            let (len, pdos) = match sm.kind {
                SmKind::MBoxOut => (s.mbx_sizes.0, vec![]),
                SmKind::MBoxIn => (s.mbx_sizes.1, vec![]),
                SmKind::Outputs | SmKind::Inputs => {
                    let list: Vec<&PdoCfg> = s.pdos.iter().filter(|pc| pc.assigned && pc.sm == i).collect();
                    let bits: u32 = list.iter().map(|pc| pc.pdo.bit_len()).sum();
                    (((bits + 7) / 8) as u16, list.iter().map(|pc| pc.pdo.index).collect())
                }
                SmKind::Unused => (0, vec![]),
            };
            sl.sms.push(SmLayout {
                index: i,
                kind: sm.kind,
                start: sm.start,
                len,
                control: sm.control,
                enable: sm.enable || len > 0,
                pdos,
            });
        }
        sl.out_offs = out_cursor;
        for sm in sl.sms.iter().filter(|x| x.kind == SmKind::Outputs && x.len > 0) {
            sl.out_bytes += sm.len as u32;
        }
        sl.out_bits = s.assigned(false).map(|pc| pc.pdo.bit_len()).sum();
        out_cursor += sl.out_bytes;
        slave_out.push(sl);
    }
    lay.out_size = out_cursor;
    lay.out_log = p.master.log_start;
    lay.in_log = p.master.log_start.wrapping_add(out_cursor);
    let mut in_cursor = 0u32;
    for (si, (s, mut sl)) in p.slaves.iter().zip(slave_out.into_iter()).enumerate() {
        sl.in_offs = in_cursor;
        for sm in sl.sms.iter().filter(|x| x.kind == SmKind::Inputs && x.len > 0) {
            sl.in_bytes += sm.len as u32;
        }
        sl.in_bits = s.assigned(true).map(|pc| pc.pdo.bit_len()).sum();
        in_cursor += sl.in_bytes;
        // FMMUs + variables
        let mut o = sl.out_offs;
        let mut ii = sl.in_offs;
        for sm in sl.sms.clone().iter() {
            let is_out = sm.kind == SmKind::Outputs;
            if !(is_out || sm.kind == SmKind::Inputs) || sm.len == 0 {
                continue;
            }
            let img_byte = if is_out { o } else { ii };
            let log = if is_out {
                lay.out_log.wrapping_add(img_byte)
            } else {
                lay.in_log.wrapping_add(img_byte)
            };
            sl.fmmus.push(FmmuLayout {
                log_start: log,
                len: sm.len,
                log_start_bit: 0,
                log_end_bit: 7,
                phys_start: sm.start,
                phys_start_bit: 0,
                dir: if is_out { 2 } else { 1 },
                sm: sm.index,
            });
            let mut bit = if is_out { img_byte * 8 } else { (lay.out_size + img_byte) * 8 };
            for pidx in sm.pdos.iter() {
                if let Some(pc) = s.pdos.iter().find(|pc| pc.pdo.index == *pidx && pc.assigned && pc.sm == sm.index) {
                    for e in pc.pdo.entries.iter() {
                        if !e.is_gap() {
                            lay.vars.push(Variable {
                                name: format!("{}.{}.{}", s.name, pc.pdo.name, e.name),
                                data_type: if e.data_type.is_empty() { bits_type(e.bitlen) } else { e.data_type.clone() },
                                bit_size: e.bitlen,
                                bit_offs: bit,
                                is_input: !is_out,
                                slave: si,
                                pdo: pc.pdo.index,
                                index: e.index,
                                subindex: e.subindex,
                            });
                        }
                        bit += e.bitlen as u32;
                    }
                }
            }
            if is_out {
                o += sm.len as u32;
            } else {
                ii += sm.len as u32;
            }
        }
        if sl.out_bytes > 0 {
            lay.n_out_slaves += 1;
        }
        if sl.in_bytes > 0 {
            lay.n_in_slaves += 1;
        }
        lay.slaves.push(sl);
    }
    lay.in_size = in_cursor;
    lay
}

fn bits_type(bits: u16) -> String {
    match bits {
        1 => "BOOL".into(),
        8 => "USINT".into(),
        16 => "UINT".into(),
        32 => "UDINT".into(),
        64 => "ULINT".into(),
        n => format!("BIT{}", n),
    }
}

// ---------------------------------------------------------------- init commands

#[derive(Clone, Debug, Default)]
pub struct Validate {
    pub data: Vec<u8>,
    pub mask: Option<Vec<u8>>,
    pub timeout_ms: u32,
}

#[derive(Clone, Debug, Default)]
pub struct EcInitCmd {
    pub transitions: Vec<String>,
    pub comment: String,
    pub requires: String,
    pub cmd: u8,
    pub adp: u16,
    pub ado: u16,
    pub data: Vec<u8>,
    pub cnt: Option<u16>,
    pub retries: u16,
    pub validate: Option<Validate>,
    pub before_slave: bool,
}

fn ic(tr: &[&str], comment: &str, cmd: u8, adp: u16, ado: u16, data: Vec<u8>) -> EcInitCmd {
    EcInitCmd {
        transitions: tr.iter().map(|s| s.to_string()).collect(),
        comment: comment.to_string(),
        requires: "cycle".into(),
        cmd,
        adp,
        ado,
        data,
        cnt: Some(1),
        retries: 3,
        validate: None,
        before_slave: false,
    }
}

fn with_validate(mut c: EcInitCmd, data: Vec<u8>, mask: Option<Vec<u8>>, t: u32) -> EcInitCmd {
    c.validate = Some(Validate { data, mask, timeout_ms: t });
    c
}

pub fn master_init_cmds(p: &Project) -> Vec<EcInitCmd> {
    let n = p.slaves.len() as u16;
    let mut v = Vec::new();
    let mut c = ic(&["IP"], "read slave count", CMD_BRD, 0, 0x0000, vec![0]);
    c.cnt = Some(n);
    v.push(c);
    let mut c = ic(&["IP"], "request init state and ack errors", CMD_BWR, 0, 0x0120, vec![0x11, 0x00]);
    c.cnt = Some(n);
    v.push(c);
    let mut c = ic(&["IP"], "clear configured addresses", CMD_BWR, 0, 0x0010, vec![0, 0]);
    c.cnt = Some(n);
    v.push(c);
    let mut c = ic(&["IP"], "clear fmmus", CMD_BWR, 0, 0x0600, vec![0; 256]);
    c.cnt = Some(n);
    v.push(c);
    let mut c = ic(&["IP"], "clear sync managers", CMD_BWR, 0, 0x0800, vec![0; 128]);
    c.cnt = Some(n);
    v.push(c);
    if p.ref_clock_index().is_some() {
        let mut c = ic(&["IP"], "DC: latch receive times (port delay measurement)", CMD_BWR, 0, 0x0900, vec![0; 4]);
        c.cnt = Some(n);
        v.push(c);
        let mut c = ic(&["IP"], "DC: reset system time filter", CMD_BWR, 0, 0x0930, vec![0x00, 0x10]);
        c.cnt = Some(n);
        v.push(c);
    }
    let mut c = ic(&["PI", "SI", "OI"], "request init state", CMD_BWR, 0, 0x0120, vec![0x01, 0x00]);
    c.cnt = Some(n);
    v.push(c);
    v
}

fn state_req(tr: &[&str], name: &str, state: u16, adp: u16, timeout: u32) -> Vec<EcInitCmd> {
    vec![
        ic(tr, &format!("request {} state", name), CMD_FPWR, adp, 0x0120, state.to_le_bytes().to_vec()),
        with_validate(
            ic(tr, &format!("check {} state", name), CMD_FPRD, adp, 0x0130, vec![0, 0]),
            state.to_le_bytes().to_vec(),
            Some(vec![0x1F, 0x00]),
            timeout,
        ),
    ]
}

fn sii_read(tr: &[&str], what: &str, adp: u16, word: u16, expect: u32) -> Vec<EcInitCmd> {
    let w = word.to_le_bytes();
    vec![
        ic(
            tr,
            &format!("read {} from SII", what),
            CMD_FPWR,
            adp,
            0x0502,
            vec![0x00, 0x01, w[0], w[1], 0, 0],
        ),
        with_validate(
            ic(tr, &format!("check {}", what), CMD_FPRD, adp, 0x0508, vec![0; 4]),
            expect.to_le_bytes().to_vec(),
            None,
            100,
        ),
    ]
}

pub fn slave_init_cmds(p: &Project, lay: &NetLayout, i: usize) -> Vec<EcInitCmd> {
    let s = &p.slaves[i];
    let sl = &lay.slaves[i];
    let a = s.phys_addr;
    let m = &p.master;
    let mut v = Vec::new();
    v.push(ic(
        &["IP"],
        "set station address",
        CMD_APWR,
        Project::auto_inc(i),
        0x0010,
        a.to_le_bytes().to_vec(),
    ));
    v.push(with_validate(
        ic(&["IP"], "check station address", CMD_FPRD, a, 0x0010, vec![0, 0]),
        a.to_le_bytes().to_vec(),
        None,
        100,
    ));
    if s.check_vendor {
        v.extend(sii_read(&["IP"], "vendor id", a, 0x0008, s.device.vendor_id));
    }
    if s.check_product {
        v.extend(sii_read(&["IP"], "product code", a, 0x000A, s.device.product_code));
    }
    if s.check_revision != RevCheck::None {
        v.extend(sii_read(&["IP"], "revision number", a, 0x000C, s.device.revision));
    }
    if s.check_serial {
        v.extend(sii_read(&["IP"], "serial number", a, 0x000E, s.serial));
    }
    if !s.device.dc_opmodes.is_empty() {
        v.push(ic(&["IP"], "clear DC activation", CMD_FPWR, a, 0x0980, vec![0, 0]));
    }
    if s.watchdog_enabled {
        v.push(ic(
            &["IP"],
            "set watchdog divider (100 us)",
            CMD_FPWR,
            a,
            0x0400,
            2498u16.to_le_bytes().to_vec(),
        ));
        let t = (s.watchdog_ms.saturating_mul(10)).min(0xFFFF) as u16;
        v.push(ic(&["IP"], "set process data watchdog", CMD_FPWR, a, 0x0420, t.to_le_bytes().to_vec()));
    }
    for sm in sl.sms.iter().filter(|x| matches!(x.kind, SmKind::MBoxOut | SmKind::MBoxIn)) {
        let nm = if sm.kind == SmKind::MBoxOut { "mailbox out" } else { "mailbox in" };
        v.push(ic(
            &["IP"],
            &format!("set sm {} ({})", sm.index, nm),
            CMD_FPWR,
            a,
            0x0800 + 8 * sm.index as u16,
            sm_bytes(sm.start, sm.len, sm.control, sm.enable),
        ));
    }
    v.extend(state_req(&["IP"], "preop", 0x0002, a, m.timeout_init_preop_ms));
    // PS
    for sm in sl.sms.iter().filter(|x| matches!(x.kind, SmKind::Outputs | SmKind::Inputs)) {
        let nm = if sm.kind == SmKind::Outputs { "outputs" } else { "inputs" };
        v.push(ic(
            &["PS"],
            &format!("set sm {} ({})", sm.index, nm),
            CMD_FPWR,
            a,
            0x0800 + 8 * sm.index as u16,
            sm_bytes(sm.start, sm.len, sm.control, sm.enable && sm.len > 0),
        ));
    }
    for (k, f) in sl.fmmus.iter().enumerate() {
        let nm = if f.dir == 2 { "outputs" } else { "inputs" };
        v.push(ic(&["PS"], &format!("set fmmu {} ({})", k, nm), CMD_FPWR, a, 0x0600 + 16 * k as u16, f.bytes()));
    }
    if p.master.dc_enabled && s.dc_active() {
        let mut d = Vec::new();
        d.extend_from_slice(&s.sync0_cycle(m.cycle_time_us).to_le_bytes());
        d.extend_from_slice(&s.sync1_cycle(m.cycle_time_us).to_le_bytes());
        v.push(ic(&["PS"], "DC: set sync0/sync1 cycle time", CMD_FPWR, a, 0x09A0, d));
        let aa = s.dc_opmode().map(|o| o.assign_activate).unwrap_or(0);
        v.push(ic(
            &["PS"],
            "DC: activate sync signals (start time set by master)",
            CMD_FPWR,
            a,
            0x0980,
            aa.to_le_bytes().to_vec(),
        ));
    }
    v.extend(state_req(&["PS"], "safeop", 0x0004, a, m.timeout_preop_safeop_ms));
    v.extend(state_req(&["SO"], "op", 0x0008, a, m.timeout_safeop_op_ms));
    v.extend(state_req(&["OS"], "safeop", 0x0004, a, m.timeout_preop_safeop_ms));
    v.extend(state_req(&["SP", "OP"], "preop", 0x0002, a, m.timeout_preop_safeop_ms));
    v.extend(state_req(&["PI", "SI", "OI"], "init", 0x0001, a, 5000));
    v
}

/// CoE commands for this slave: generated PDO mapping/assignment followed by user/ESI commands.
pub fn slave_coe_cmds(p: &Project, i: usize) -> Vec<CoeInitCmd> {
    let s = &p.slaves[i];
    let mut v = Vec::new();
    let coe = match s.device.coe() {
        Some(c) => c,
        None => return v,
    };
    if s.generate_pdo_cmds {
        let mk = |idx: u16, sub: u8, data: Vec<u8>, comment: String, ca: bool| CoeInitCmd {
            transition: "PS".into(),
            index: idx,
            subindex: sub,
            data,
            comment,
            complete_access: ca,
            fixed: true,
            enabled: true,
            generated: true,
            timeout_ms: p.master.timeout_mailbox_ms,
        };
        if coe.pdo_config {
            for pc in s.pdos.iter().filter(|pc| pc.assigned && !pc.pdo.fixed) {
                let pi = pc.pdo.index;
                v.push(mk(pi, 0, vec![0], format!("clear pdo {:#06x} entries", pi), false));
                for (k, e) in pc.pdo.entries.iter().enumerate() {
                    let val: u32 = (e.index as u32) << 16 | (e.subindex as u32) << 8 | (e.bitlen as u32 & 0xFF);
                    v.push(mk(
                        pi,
                        (k + 1) as u8,
                        val.to_le_bytes().to_vec(),
                        format!("download pdo {:#06x} entry {}", pi, k + 1),
                        false,
                    ));
                }
                v.push(mk(
                    pi,
                    0,
                    vec![pc.pdo.entries.len() as u8],
                    format!("download pdo {:#06x} entry count", pi),
                    false,
                ));
            }
        }
        if coe.pdo_assign {
            for (smi, sm) in s.device.sms.iter().enumerate() {
                if !matches!(sm.kind, SmKind::Outputs | SmKind::Inputs) {
                    continue;
                }
                let obj = 0x1C10 + smi as u16;
                let list: Vec<u16> = s.pdos.iter().filter(|pc| pc.assigned && pc.sm as usize == smi).map(|pc| pc.pdo.index).collect();
                if coe.complete_access {
                    let mut d = (list.len() as u16).to_le_bytes().to_vec();
                    for x in list.iter() {
                        d.extend_from_slice(&x.to_le_bytes());
                    }
                    v.push(mk(obj, 0, d, format!("download pdo {:#06x} assignment (complete access)", obj), true));
                } else {
                    v.push(mk(obj, 0, vec![0], format!("clear pdo {:#06x} assignment", obj), false));
                    for (k, x) in list.iter().enumerate() {
                        v.push(mk(
                            obj,
                            (k + 1) as u8,
                            x.to_le_bytes().to_vec(),
                            format!("download pdo {:#06x} index {}", obj, k + 1),
                            false,
                        ));
                    }
                    v.push(mk(obj, 0, vec![list.len() as u8], format!("download pdo {:#06x} count", obj), false));
                }
            }
        }
    }
    v.extend(s.coe_cmds.iter().filter(|c| c.enabled).cloned());
    v
}

// ---------------------------------------------------------------- validation

#[derive(Clone, Debug, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub sev: Severity,
    pub slave: Option<usize>,
    pub text: String,
}

pub fn validate(p: &Project, lay: &NetLayout) -> Vec<Issue> {
    let mut v = Vec::new();
    let mut push = |sev, slave, text: String| v.push(Issue { sev, slave, text });
    if p.slaves.is_empty() {
        push(Severity::Warning, None, "The configuration contains no slaves.".into());
    }
    if p.master.cycle_time_us == 0 {
        push(Severity::Error, None, "Cycle time must be greater than 0.".into());
    }
    let total = if p.master.use_lrw {
        lay.out_size + lay.in_size
    } else {
        lay.out_size.max(lay.in_size)
    };
    if total > p.master.max_frame_data {
        push(
            Severity::Error,
            None,
            format!(
                "Process data ({} bytes) does not fit into one frame ({} bytes max).",
                total, p.master.max_frame_data
            ),
        );
    }
    for (i, s) in p.slaves.iter().enumerate() {
        if s.phys_addr == 0 {
            push(Severity::Error, Some(i), format!("{}: station address 0 is not allowed.", s.name));
        }
        if let Some(j) = p.slaves.iter().position(|o| o.phys_addr == s.phys_addr) {
            if j < i {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: station address {} is also used by {}.", s.name, s.phys_addr, p.slaves[j].name),
                );
            }
        }
        if let Some(par) = s.parent {
            if par >= i {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: upstream device must come before it in the list.", s.name),
                );
            } else if s.parent_port as usize >= p.slaves[par].device.port_count() {
                push(
                    Severity::Warning,
                    Some(i),
                    format!("{}: port {} does not exist on {}.", s.name, port_letter(s.parent_port), p.slaves[par].name),
                );
            }
        }
        for pc in s.pdos.iter() {
            if pc.pdo.mandatory && !pc.assigned {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: mandatory PDO {:#06X} is not assigned.", s.name, pc.pdo.index),
                );
            }
            if pc.assigned {
                for x in pc.pdo.excludes.iter() {
                    if *x > pc.pdo.index && s.pdos.iter().any(|o| o.pdo.index == *x && o.assigned) {
                        push(
                            Severity::Error,
                            Some(i),
                            format!("{}: PDO {:#06X} excludes PDO {:#06X} but both are assigned.", s.name, pc.pdo.index, x),
                        );
                    }
                }
                if s.device.sms.get(pc.sm as usize).is_none() {
                    push(
                        Severity::Error,
                        Some(i),
                        format!("{}: PDO {:#06X} is assigned to missing sync manager {}.", s.name, pc.pdo.index, pc.sm),
                    );
                }
            }
        }
        let has_assign = s.device.coe().map(|c| c.pdo_assign).unwrap_or(false);
        if !has_assign {
            let default = SlaveCfg::from_device(&s.device, 0);
            let changed = default.pdos.iter().zip(s.pdos.iter()).any(|(a, b)| a.assigned != b.assigned);
            if changed {
                push(
                    Severity::Warning,
                    Some(i),
                    format!(
                        "{}: PDO assignment changed but the device cannot download a PDO assignment (no CoE PdoAssign).",
                        s.name
                    ),
                );
            }
        }
        if let Some(sl) = lay.slaves.get(i) {
            for (k, sm) in sl.sms.iter().enumerate() {
                if let Some(max) = s.device.sms.get(k).and_then(|d| d.max_size) {
                    if sm.len > max {
                        push(
                            Severity::Error,
                            Some(i),
                            format!("{}: sync manager {} needs {} bytes, maximum is {}.", s.name, k, sm.len, max),
                        );
                    }
                }
                if let Some(min) = s.device.sms.get(k).and_then(|d| d.min_size) {
                    if matches!(sm.kind, SmKind::MBoxIn | SmKind::MBoxOut) && sm.len < min {
                        push(
                            Severity::Error,
                            Some(i),
                            format!("{}: mailbox sync manager {} is {} bytes, minimum is {}.", s.name, k, sm.len, min),
                        );
                    }
                }
            }
            if sl.fmmus.len() > s.device.fmmus.len().max(if s.device.fmmus.is_empty() { 8 } else { 0 }) {
                push(
                    Severity::Warning,
                    Some(i),
                    format!("{}: needs {} FMMUs, device describes {}.", s.name, sl.fmmus.len(), s.device.fmmus.len()),
                );
            }
        }
        if s.dc_active() && !p.master.dc_enabled {
            push(
                Severity::Warning,
                Some(i),
                format!("{}: DC mode selected but distributed clocks are disabled in the master.", s.name),
            );
        }
        if s.dc_active() && p.master.dc_enabled && s.sync0_cycle(p.master.cycle_time_us) == 0 {
            push(Severity::Error, Some(i), format!("{}: SYNC0 cycle time is 0.", s.name));
        }
    }
    if let Some(r) = p.master.ref_clock {
        if let Some(s) = p.slaves.get(r) {
            if s.device.dc_opmodes.is_empty() {
                push(
                    Severity::Error,
                    Some(r),
                    format!("{}: selected as reference clock but has no DC support.", s.name),
                );
            }
        }
    }
    if v.is_empty() {
        v.push(Issue {
            sev: Severity::Info,
            slave: None,
            text: "No problems found.".into(),
        });
    }
    v
}

pub fn demo_project(lib: &EsiLibrary) -> Project {
    let mut p = Project::default();
    for (prod, _) in [
        (0x00010001u32, 0),
        (0x00020008, 0),
        (0x00030008, 0),
        (0x00040004, 0),
        (0x00100200, 0),
        (0x00100200, 0),
    ] {
        if let Some(d) = lib.devices.iter().find(|d| d.product_code == prod) {
            let d = d.clone();
            p.add_device(&d, Some(p.slaves.len().saturating_sub(1)).filter(|_| !p.slaves.is_empty()));
        }
    }
    p.relinearize();
    p
}
