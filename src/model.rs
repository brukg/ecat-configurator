//! Network configuration model, process image layout, init command generation, validation.

use crate::esi::*;
use serde::{Deserialize, Serialize};

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
    /// Modules plugged into the slots of a modular device, in slot order.
    #[serde(default)]
    pub modules: Vec<ModuleInst>,
    /// SoE start-up writes (servo drives).
    #[serde(default)]
    pub soe_cmds: Vec<crate::proto::SoeInitCmd>,
    /// EoE IP settings sent at start-up.
    #[serde(default)]
    pub eoe: crate::proto::EoeCfg,
    /// Set on the first slave of a hot-connect group.
    #[serde(default)]
    pub hot_connect: Option<HotConnect>,
}

/// A hot-connect group: this slave and the next `members - 1` slaves may be missing or
/// plugged in while running. The master recognises the group by the head's station alias.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HotConnect {
    pub members: u16,
    /// Configured station alias (SII word 4, register 0x0012) of the group head.
    pub alias: u16,
}

/// One configured slot position: which slot type and which module.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModuleInst {
    pub slot: usize,
    pub ident: u32,
}

/// A module PDO moved to its slot position (PDO and object indices shifted).
fn slot_pdo(p: &Pdo, pos: u32, sd: &SlotDef, md: &ModuleDef) -> Pdo {
    let pinc = (sd.pdo_inc as u32 * pos) as u16;
    let iinc = (sd.index_inc as u32 * pos) as u16;
    let mut q = p.clone();
    q.index = p.index.wrapping_add(pinc);
    q.excludes = p.excludes.iter().map(|x| x.wrapping_add(pinc)).collect();
    for e in q.entries.iter_mut() {
        if !e.is_gap() && slot_shifts_index(e.index) {
            e.index = e.index.wrapping_add(iinc);
        }
    }
    q.name = format!("Slot {} {}: {}", pos + 1, md.type_name, p.name);
    q
}

/// Default slot configuration of a device: the minimum number of each slot with its default module.
pub fn default_modules(dev: &EsiDevice) -> Vec<ModuleInst> {
    let mut v = Vec::new();
    for (si, sd) in dev.slots.iter().enumerate() {
        let ident = sd
            .default_ident
            .or_else(|| sd.idents.first().copied())
            .or_else(|| dev.modules.iter().find(|m| sd.accepts(m)).map(|m| m.ident));
        if let Some(id) = ident {
            for _ in 0..sd.min {
                v.push(ModuleInst { slot: si, ident: id });
            }
        }
    }
    v
}

/// Spread a list of module idents (e.g. read from an ENI's 0xF030 list) over the slot types.
pub fn modules_from_idents(dev: &EsiDevice, idents: &[u32]) -> Vec<ModuleInst> {
    let mut count = vec![0u32; dev.slots.len()];
    let mut cur = 0usize;
    let mut v = Vec::new();
    let fits = |si: usize, id: u32| {
        let sd = &dev.slots[si];
        sd.idents.contains(&id) || dev.modules.iter().any(|m| m.ident == id && sd.accepts(m))
    };
    for id in idents.iter().copied() {
        let pick = (cur..dev.slots.len()).chain(0..cur).find(|&si| fits(si, id) && count[si] < dev.slots[si].max);
        if let Some(si) = pick {
            count[si] += 1;
            cur = si;
            v.push(ModuleInst { slot: si, ident: id });
        }
    }
    v
}

/// PDO configuration of a device with the given modules, using the ESI default assignment rules.
pub fn build_pdo_cfgs(dev: &EsiDevice, modules: &[ModuleInst]) -> Vec<PdoCfg> {
    let mut rx = dev.rx_pdos.clone();
    let mut tx = dev.tx_pdos.clone();
    for (pos, mi) in modules.iter().enumerate() {
        let (sd, md) = match (dev.slots.get(mi.slot), dev.modules.iter().find(|m| m.ident == mi.ident)) {
            (Some(sd), Some(md)) => (sd, md),
            _ => continue,
        };
        rx.extend(md.rx_pdos.iter().map(|p| slot_pdo(p, pos as u32, sd, md)));
        tx.extend(md.tx_pdos.iter().map(|p| slot_pdo(p, pos as u32, sd, md)));
    }
    let mut pdos = Vec::new();
    let first_sm = |kind: SmKind| dev.sms.iter().position(|s| s.kind == kind).unwrap_or(0) as u8;
    for (list, is_tx, kind) in [(&rx, false, SmKind::Outputs), (&tx, true, SmKind::Inputs)] {
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
    pdos
}

static UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_uid() -> u64 {
    UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Rebuild the wiring from a bus scan: `open[i]` are the downstream ports (1=B, 2=C, 3=D)
/// on which slave i (in scan order) has communication. Returns (parent, port) per slave.
pub fn topology_from_ports(open: &[Vec<u8>]) -> Vec<(Option<usize>, u8)> {
    let rank = |p: u8| match p {
        3 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    };
    let mut stack: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut out = Vec::with_capacity(open.len());
    for (i, ports) in open.iter().enumerate() {
        let link = if i == 0 {
            (None, 1)
        } else {
            match stack.last_mut() {
                Some((p, rest)) => {
                    let parent = *p;
                    let port = rest.remove(0);
                    if rest.is_empty() {
                        stack.pop();
                    }
                    (Some(parent), port)
                }
                None => (Some(i - 1), 1),
            }
        };
        out.push(link);
        let mut down: Vec<u8> = ports.iter().copied().filter(|p| (1..=3).contains(p)).collect();
        down.sort_by_key(|p| rank(*p));
        down.dedup();
        if !down.is_empty() {
            stack.push((i, down));
        }
    }
    out
}

/// Downstream ports with communication, from the DL status register (0x0110).
pub fn open_ports_from_dl_status(dl: u16) -> Vec<u8> {
    (1u8..=3).filter(|p| dl & (1 << (9 + 2 * *p as u16)) != 0).collect()
}

/// For every slave, the index of the head of the hot-connect group it belongs to.
pub fn hot_connect_groups(p: &Project) -> Vec<Option<usize>> {
    let mut v = vec![None; p.slaves.len()];
    for (i, s) in p.slaves.iter().enumerate() {
        if let Some(h) = &s.hot_connect {
            for k in i..(i + h.members.max(1) as usize).min(p.slaves.len()) {
                if v[k].is_none() {
                    v[k] = Some(i);
                }
            }
        }
    }
    v
}

pub fn port_letter(p: u8) -> char {
    (b'A' + p.min(3)) as char
}

impl SlaveCfg {
    pub fn from_device(dev: &EsiDevice, phys: u16) -> SlaveCfg {
        let modules = default_modules(dev);
        let pdos = build_pdo_cfgs(dev, &modules);
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
            modules,
            soe_cmds: Vec::new(),
            eoe: Default::default(),
            hot_connect: None,
        }
    }

    /// Change the plugged modules; keeps the assignment of PDOs that stay the same.
    pub fn set_modules(&mut self, modules: Vec<ModuleInst>) {
        let mut new = build_pdo_cfgs(&self.device, &modules);
        for pc in new.iter_mut() {
            if let Some(old) = self
                .pdos
                .iter()
                .find(|o| o.is_tx == pc.is_tx && o.pdo.index == pc.pdo.index && o.pdo.name == pc.pdo.name)
            {
                pc.assigned = old.assigned || pc.pdo.mandatory;
                pc.sm = old.sm;
                if !pc.pdo.fixed {
                    pc.pdo.entries = old.pdo.entries.clone();
                }
            }
        }
        self.modules = modules;
        self.pdos = new;
    }

    pub fn module_def(&self, ident: u32) -> Option<&ModuleDef> {
        self.device.modules.iter().find(|m| m.ident == ident)
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
        let s = SlaveCfg::from_device(dev, phys);
        self.insert_slave(s, at)
    }

    /// Insert a configured slave after position `at` (None = at the end), keeping the chain linked.
    pub fn insert_slave(&mut self, mut s: SlaveCfg, at: Option<usize>) -> usize {
        let idx = at.map(|i| (i + 1).min(self.slaves.len())).unwrap_or(self.slaves.len());
        s.parent = if idx == 0 { None } else { Some(idx - 1) };
        s.parent_port = 1;
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
        if let Some(r) = self.master.ref_clock {
            if r >= idx {
                self.master.ref_clock = Some(r + 1);
            }
        }
        self.slaves.insert(idx, s);
        idx
    }

    /// Paste a copy of a slave: it gets a new identity and, if needed, a free station address.
    pub fn paste_slave(&mut self, mut s: SlaveCfg, at: Option<usize>) -> usize {
        s.uid = next_uid();
        if s.phys_addr == 0 || self.slaves.iter().any(|o| o.phys_addr == s.phys_addr) {
            let old = s.phys_addr;
            s.phys_addr = self.next_phys_addr();
            let prefix = format!("Slave_{}", old);
            if let Some(rest) = s.name.strip_prefix(&prefix) {
                s.name = format!("Slave_{}{}", s.phys_addr, rest);
            }
        }
        self.insert_slave(s, at)
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

    /// Order in which an EtherCAT frame reaches the slaves: depth first from the
    /// master, and on every device through its ports in the order D, B, C.
    pub fn bus_order(&self) -> Vec<usize> {
        fn rank(port: u8) -> u8 {
            match port {
                3 => 0,
                1 => 1,
                2 => 2,
                _ => 3,
            }
        }
        let n = self.slaves.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
        for (i, s) in self.slaves.iter().enumerate() {
            let p = match s.parent {
                Some(p) if p < n && p != i => p + 1,
                _ => 0,
            };
            children[p].push(i);
        }
        for c in children.iter_mut() {
            c.sort_by_key(|&i| (rank(self.slaves[i].parent_port), i));
        }
        let mut out = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        let mut stack: Vec<usize> = children[0].iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            out.push(i);
            for &c in children[i + 1].iter().rev() {
                stack.push(c);
            }
        }
        // anything unreachable (a loop in the parent links) keeps its list position at the end
        out.extend((0..n).filter(|i| !seen[*i]));
        out
    }

    /// Reorder the slave list to the bus order, so auto-increment addresses match the wiring.
    /// Returns true when the order changed.
    pub fn sort_by_topology(&mut self) -> bool {
        let order = self.bus_order();
        if order.iter().enumerate().all(|(k, &i)| k == i) {
            return false;
        }
        let mut new_index = vec![0usize; order.len()];
        for (k, &i) in order.iter().enumerate() {
            new_index[i] = k;
        }
        let old = std::mem::take(&mut self.slaves);
        let mut slots: Vec<Option<SlaveCfg>> = old.into_iter().map(Some).collect();
        for &i in order.iter() {
            let mut s = slots[i].take().expect("each slave once");
            s.parent = s.parent.map(|p| new_index[p]);
            self.slaves.push(s);
        }
        if let Some(r) = self.master.ref_clock {
            self.master.ref_clock = new_index.get(r).copied();
        }
        true
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
    /// Working counter of a single LRW over the whole image (reference value for tests).
    #[cfg(test)]
    pub fn lrw_wkc(&self) -> u16 {
        // per slave: +2 for a successful write, +1 for a successful read
        self.slaves
            .iter()
            .map(|s| (if s.out_bytes > 0 { 2 } else { 0 }) + (if s.in_bytes > 0 { 1 } else { 0 }))
            .sum()
    }
}

// ---------------------------------------------------------------- cyclic frames

/// Largest Ethernet payload; an EtherCAT frame is 2 header bytes plus datagrams of 12 + data bytes.
pub const ETH_PAYLOAD: u32 = 1500;

#[derive(Clone, Debug, PartialEq)]
pub struct CyclicCmd {
    pub cmd: u8,
    /// Logical address (LRW/LRD/LWR); for BRD the low/high words are ADP/ADO.
    pub addr: u32,
    pub len: u32,
    pub wkc: u16,
    /// Byte offsets in the input and output images (image offset = logical address - start).
    pub in_offs: u32,
    pub out_offs: u32,
    pub comment: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CyclicFrame {
    pub cmds: Vec<CyclicCmd>,
}

impl CyclicFrame {
    /// Bytes of the EtherCAT part of the frame (header + datagrams).
    pub fn bytes(&self) -> u32 {
        2 + self.cmds.iter().map(|c| 12 + c.len).sum::<u32>()
    }
    /// Time on a 100 Mbit/s wire incl. Ethernet header, FCS, preamble and gap, in µs.
    pub fn wire_us(&self) -> f32 {
        ((self.bytes().max(46) + 14 + 4 + 8 + 12) * 8) as f32 / 100.0
    }
}

/// Split the image range [start, end) into pieces of at most `limit` bytes,
/// cutting at slave (FMMU) boundaries where possible.
fn split_range(start: u32, end: u32, limit: u32, cuts: &[u32]) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    let mut a = start;
    let limit = limit.max(1);
    while a < end {
        let max_b = (a + limit).min(end);
        let b = if max_b == end {
            end
        } else {
            cuts.iter().copied().filter(|c| *c > a && *c <= max_b).max().unwrap_or(max_b)
        };
        v.push((a, b));
        a = b;
    }
    v
}

/// Expected working counter of a logical command over image range [a, b).
fn range_wkc(lay: &NetLayout, cmd: u8, a: u32, b: u32) -> u16 {
    let mut w = 0u16;
    for sl in lay.slaves.iter() {
        let hit = |dir: u8| {
            sl.fmmus.iter().any(|f| {
                let fs = f.log_start.wrapping_sub(lay.out_log);
                let fe = fs + f.len as u32;
                f.dir == dir && fs < b && fe > a
            })
        };
        let (o, i) = (hit(2), hit(1));
        w += match cmd {
            CMD_LRW => (if o { 2 } else { 0 }) + (if i { 1 } else { 0 }),
            CMD_LWR => o as u16,
            CMD_LRD => i as u16,
            _ => 0,
        };
    }
    w
}

/// The cyclic frames sent every cycle: process data split to fit the frame size,
/// packed into as few frames as possible, plus a BRD of the AL status.
pub fn cyclic_frames(p: &Project, lay: &NetLayout) -> Vec<CyclicFrame> {
    let limit = p.master.max_frame_data.clamp(16, ETH_PAYLOAD - 14);
    let total = lay.out_size + lay.in_size;
    let mut cuts: Vec<u32> = Vec::new();
    for sl in lay.slaves.iter() {
        for f in sl.fmmus.iter() {
            let fs = f.log_start.wrapping_sub(lay.out_log);
            cuts.push(fs);
            cuts.push(fs + f.len as u32);
        }
    }
    cuts.push(lay.out_size);
    let mut pieces: Vec<(u8, u32, u32, &str)> = Vec::new();
    if total > 0 {
        if p.master.use_lrw {
            for (a, b) in split_range(0, total, limit, &cuts) {
                pieces.push((CMD_LRW, a, b, "process data (LRW)"));
            }
        } else {
            for (a, b) in split_range(0, lay.out_size, limit, &cuts) {
                pieces.push((CMD_LWR, a, b, "outputs (LWR)"));
            }
            for (a, b) in split_range(lay.out_size, total, limit, &cuts) {
                pieces.push((CMD_LRD, a, b, "inputs (LRD)"));
            }
        }
    }
    let n_parts = pieces.len();
    let budget = limit + 14;
    let mut frames: Vec<CyclicFrame> = vec![CyclicFrame::default()];
    for (k, (cmd, a, b, what)) in pieces.into_iter().enumerate() {
        let c = CyclicCmd {
            cmd,
            addr: lay.out_log.wrapping_add(a),
            len: b - a,
            wkc: range_wkc(lay, cmd, a, b),
            in_offs: if cmd == CMD_LWR { 0 } else { a },
            out_offs: if cmd == CMD_LRD { 0 } else { a },
            comment: if n_parts > 1 {
                format!("{} part {} of {}", what, k + 1, n_parts)
            } else {
                what.to_string()
            },
        };
        let need_new = frames.last().map(|f| !f.cmds.is_empty() && f.bytes() + 12 + c.len > budget).unwrap_or(false);
        if need_new {
            frames.push(CyclicFrame::default());
        }
        if let Some(f) = frames.last_mut() {
            f.cmds.push(c);
        }
    }
    let brd = CyclicCmd {
        cmd: CMD_BRD,
        addr: 0x0130u32 << 16,
        len: 2,
        wkc: p.slaves.len() as u16,
        in_offs: total,
        out_offs: 0,
        comment: "read AL status".into(),
    };
    if frames.last().map(|f| f.bytes() + 14 > budget).unwrap_or(false) {
        frames.push(CyclicFrame::default());
    }
    if let Some(f) = frames.last_mut() {
        f.cmds.push(brd);
    }
    frames
}

/// Sum of the expected working counters of all process data commands.
pub fn process_data_wkc(frames: &[CyclicFrame]) -> u32 {
    frames
        .iter()
        .flat_map(|f| f.cmds.iter())
        .filter(|c| c.cmd != CMD_BRD)
        .map(|c| c.wkc as u32)
        .sum()
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
    let gen = |idx: u16, sub: u8, data: Vec<u8>, comment: String| CoeInitCmd {
        transition: "PS".into(),
        index: idx,
        subindex: sub,
        data,
        comment,
        complete_access: false,
        fixed: true,
        enabled: true,
        generated: true,
        timeout_ms: p.master.timeout_mailbox_ms,
    };
    if !s.modules.is_empty() {
        // configured module ident list (ETG.5001 modular device profile)
        v.push(gen(0xF030, 0, vec![0], "clear configured module list (0xF030)".into()));
        for (k, m) in s.modules.iter().enumerate() {
            let name = s.module_def(m.ident).map(|d| d.type_name.clone()).unwrap_or_default();
            v.push(gen(
                0xF030,
                (k + 1) as u8,
                m.ident.to_le_bytes().to_vec(),
                format!("slot {} module ident ({})", k + 1, name),
            ));
        }
        v.push(gen(0xF030, 0, vec![s.modules.len() as u8], "configured module count (0xF030)".into()));
    }
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
    // start-up parameters that come with the plugged modules
    for (pos, m) in s.modules.iter().enumerate() {
        if let (Some(md), Some(sd)) = (s.module_def(m.ident), s.device.slots.get(m.slot)) {
            for c in md.init_cmds.iter() {
                let mut c = c.clone();
                if slot_shifts_index(c.index) {
                    c.index = c.index.wrapping_add((sd.index_inc as u32 * pos as u32) as u16);
                }
                c.comment = format!("slot {} {}: {}", pos + 1, md.type_name, c.comment);
                c.generated = true;
                c.timeout_ms = p.master.timeout_mailbox_ms;
                v.push(c);
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
    let frames = cyclic_frames(p, lay);
    if frames.len() > 1 {
        push(
            Severity::Info,
            None,
            format!(
                "Process data ({} bytes) is sent in {} frames per cycle ({} bytes per frame at most).",
                lay.out_size + lay.in_size,
                frames.len(),
                p.master.max_frame_data
            ),
        );
    }
    let wire: f32 = frames.iter().map(|f| f.wire_us()).sum();
    if p.master.cycle_time_us > 0 && wire > p.master.cycle_time_us as f32 * 0.5 {
        push(
            Severity::Warning,
            None,
            format!(
                "The cyclic frames need {:.0} µs on the wire, {:.0}% of the {} µs cycle; use a longer cycle time.",
                wire,
                wire * 100.0 / p.master.cycle_time_us as f32,
                p.master.cycle_time_us
            ),
        );
    }
    if p.bus_order().iter().enumerate().any(|(k, &i)| k != i) {
        push(
            Severity::Error,
            None,
            "The slave list does not follow the wiring (ports D, B, C), so auto-increment addresses would be wrong. Use Network › Sort slaves by wiring."
                .into(),
        );
    }
    let groups = hot_connect_groups(p);
    for (i, s) in p.slaves.iter().enumerate() {
        if let Some(h) = &s.hot_connect {
            if h.members == 0 {
                push(Severity::Error, Some(i), format!("{}: hot-connect group needs at least one member.", s.name));
            }
            if i + h.members as usize > p.slaves.len() {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: hot-connect group of {} runs past the end of the network.", s.name, h.members),
                );
            }
            if groups[i] != Some(i) {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: starts a hot-connect group inside another group.", s.name),
                );
            }
            if h.alias == 0 {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: hot-connect group needs a station alias other than 0.", s.name),
                );
            } else if let Some(j) = p.slaves.iter().position(|o| o.hot_connect.as_ref().map(|x| x.alias) == Some(h.alias)) {
                if j < i {
                    push(
                        Severity::Error,
                        Some(i),
                        format!("{}: station alias {} is also used by the group of {}.", s.name, h.alias, p.slaves[j].name),
                    );
                }
            }
        }
        if groups[i].is_some() && p.ref_clock_index() == Some(i) {
            push(
                Severity::Warning,
                Some(i),
                format!(
                    "{}: the DC reference clock is in a hot-connect group; when the group is missing there is no reference clock.",
                    s.name
                ),
            );
        }
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
            } else if let Some(j) = p.slaves.iter().position(|o| o.parent == Some(par) && o.parent_port == s.parent_port) {
                if j < i {
                    push(
                        Severity::Error,
                        Some(i),
                        format!(
                            "{}: port {} of {} is already used by {}.",
                            s.name,
                            port_letter(s.parent_port),
                            p.slaves[par].name,
                            p.slaves[j].name
                        ),
                    );
                }
            }
            if par < i && s.parent_port as usize >= p.slaves[par].device.port_count() {
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
        if !s.device.slots.is_empty() {
            for (si, sd) in s.device.slots.iter().enumerate() {
                let n = s.modules.iter().filter(|m| m.slot == si).count() as u32;
                if n < sd.min {
                    push(
                        Severity::Error,
                        Some(i),
                        format!("{}: slot \"{}\" needs at least {} module(s), {} configured.", s.name, sd.name, sd.min, n),
                    );
                }
                if n > sd.max {
                    push(
                        Severity::Error,
                        Some(i),
                        format!("{}: slot \"{}\" allows at most {} module(s), {} configured.", s.name, sd.name, sd.max, n),
                    );
                }
            }
            for (k, m) in s.modules.iter().enumerate() {
                match (s.module_def(m.ident), s.device.slots.get(m.slot)) {
                    (Some(md), Some(sd)) if !sd.accepts(md) => {
                        push(
                            Severity::Error,
                            Some(i),
                            format!("{}: module {} does not fit slot {}.", s.name, md.type_name, k + 1),
                        );
                    }
                    (None, _) => push(
                        Severity::Error,
                        Some(i),
                        format!("{}: slot {} has unknown module ident {:#010x}.", s.name, k + 1, m.ident),
                    ),
                    _ => {}
                }
            }
            if !s.modules.is_empty() && s.device.coe().is_none() {
                push(
                    Severity::Warning,
                    Some(i),
                    format!("{}: modules are configured but the device has no CoE to download the module list.", s.name),
                );
            }
        }
        // start-up SDOs checked against the device's object dictionary
        if !s.device.dictionary.is_empty() {
            for c in slave_coe_cmds(p, i).iter().filter(|c| !c.generated) {
                let at = format!("{:04X}:{:02X}", c.index, c.subindex);
                match crate::od::lookup(&s.device.dictionary, c.index, c.subindex) {
                    None => {
                        if !s.device.dictionary.iter().any(|o| o.index == c.index) {
                            push(
                                Severity::Warning,
                                Some(i),
                                format!("{}: start-up command {} writes an object that is not in the device's dictionary.", s.name, at),
                            );
                        } else if !c.complete_access {
                            push(
                                Severity::Warning,
                                Some(i),
                                format!("{}: start-up command {} writes a subindex the object does not have.", s.name, at),
                            );
                        }
                    }
                    Some((o, e)) => {
                        if !e.writable() {
                            push(
                                Severity::Warning,
                                Some(i),
                                format!("{}: start-up command {} writes \"{}\", which is read-only.", s.name, at, o.entry_name(e)),
                            );
                        }
                        if !c.complete_access && e.bit_size > 0 && e.bit_size <= 64 {
                            let need = ((e.bit_size + 7) / 8) as usize;
                            if c.data.len() != need {
                                push(
                                    Severity::Warning,
                                    Some(i),
                                    format!(
                                        "{}: start-up command {} writes {} byte(s), but \"{}\" is {} ({} bit).",
                                        s.name,
                                        at,
                                        c.data.len(),
                                        o.entry_name(e),
                                        e.data_type,
                                        e.bit_size
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
        let mbx = s.device.mailbox.as_ref();
        if s.soe_cmds.iter().any(|c| c.enabled) && !mbx.map(|m| m.soe).unwrap_or(false) {
            push(
                Severity::Error,
                Some(i),
                format!("{}: SoE start-up commands are set but the device has no SoE mailbox.", s.name),
            );
        }
        for c in s.soe_cmds.iter().filter(|c| c.enabled && c.data.is_empty()) {
            push(
                Severity::Error,
                Some(i),
                format!("{}: SoE command {} has no data.", s.name, crate::proto::format_idn(c.idn)),
            );
        }
        if s.eoe.enabled {
            if !mbx.map(|m| m.eoe).unwrap_or(false) {
                push(
                    Severity::Error,
                    Some(i),
                    format!("{}: EoE IP settings are set but the device has no EoE mailbox.", s.name),
                );
            } else if let Err(e) = crate::proto::eoe_set_ip_data(&s.eoe) {
                push(Severity::Error, Some(i), format!("{}: EoE settings: {}.", s.name, e));
            }
            if !s.eoe.ip.trim().is_empty() {
                if let Some(j) = p.slaves.iter().position(|o| o.eoe.enabled && o.eoe.ip.trim() == s.eoe.ip.trim()) {
                    if j < i {
                        push(
                            Severity::Error,
                            Some(i),
                            format!("{}: EoE IP address {} is also used by {}.", s.name, s.eoe.ip.trim(), p.slaves[j].name),
                        );
                    }
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
    if v.iter().all(|i| i.sev == Severity::Info) {
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
        (0x00200001, 0),
    ] {
        if let Some(d) = lib.devices.iter().find(|d| d.product_code == prod) {
            let d = d.clone();
            p.add_device(&d, Some(p.slaves.len().saturating_sub(1)).filter(|_| !p.slaves.is_empty()));
        }
    }
    p.relinearize();
    if let Some(s) = p.slaves.iter_mut().find(|s| !s.device.slots.is_empty()) {
        s.set_modules(vec![
            ModuleInst { slot: 0, ident: 0x101 },
            ModuleInst { slot: 0, ident: 0x102 },
            ModuleInst { slot: 0, ident: 0x103 },
        ]);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo_lib() -> EsiLibrary {
        let mut lib = EsiLibrary::default();
        lib.add_xml(SAMPLE_IO, "io").unwrap();
        lib.add_xml(SAMPLE_DRIVE, "drv").unwrap();
        lib.add_xml(SAMPLE_MODULAR, "mod").unwrap();
        lib
    }

    #[test]
    fn hot_connect_groups_and_checks() {
        let mut p = demo_project(&demo_lib());
        p.slaves[4].hot_connect = Some(HotConnect { members: 2, alias: 77 });
        let g = hot_connect_groups(&p);
        assert_eq!(g[3], None);
        assert_eq!(g[4], Some(4));
        assert_eq!(g[5], Some(4));
        assert_eq!(g[6], None);
        let lay = compute_layout(&p);
        assert!(!validate(&p, &lay).iter().any(|x| x.sev == Severity::Error));
        p.slaves[5].hot_connect = Some(HotConnect { members: 5, alias: 77 });
        let lay = compute_layout(&p);
        let t: Vec<String> = validate(&p, &lay).into_iter().map(|x| x.text).collect();
        assert!(t.iter().any(|x| x.contains("inside another group")));
        assert!(t.iter().any(|x| x.contains("runs past the end")));
        assert!(t.iter().any(|x| x.contains("also used")));
    }

    #[test]
    fn bus_order_follows_ports_d_b_c() {
        let mut p = demo_project(&demo_lib());
        let uids: Vec<u64> = p.slaves.iter().map(|s| s.uid).collect();
        // hang slave 4 (and with it 5 and 6) on port D of slave 0: D is served before B
        p.slaves[4].parent = Some(0);
        p.slaves[4].parent_port = 3;
        assert_eq!(p.bus_order(), vec![0, 4, 5, 6, 1, 2, 3]);
        let lay = compute_layout(&p);
        assert!(validate(&p, &lay).iter().any(|x| x.text.contains("does not follow the wiring")));
        assert!(p.sort_by_topology());
        let now: Vec<u64> = p.slaves.iter().map(|s| s.uid).collect();
        assert_eq!(now, vec![uids[0], uids[4], uids[5], uids[6], uids[1], uids[2], uids[3]]);
        assert_eq!(p.slaves[1].parent, Some(0));
        assert_eq!(p.slaves[4].parent, Some(0)); // former slave 1, still on port B of slave 0
        assert_eq!(p.slaves[2].parent, Some(1));
        assert_eq!(p.bus_order(), (0..7).collect::<Vec<_>>());
        assert!(!p.sort_by_topology());
        let lay = compute_layout(&p);
        assert!(!validate(&p, &lay).iter().any(|x| x.sev == Severity::Error));
        // two slaves on the same port
        p.slaves[4].parent_port = 3;
        let lay = compute_layout(&p);
        assert!(validate(&p, &lay).iter().any(|x| x.text.contains("already used")));
    }

    #[test]
    fn wiring_rebuilt_from_scanned_ports() {
        // 0: coupler with B and D used; 1: on D, end of branch; 2: on B of 0, forwards on B; 3: end
        let open = vec![vec![1, 3], vec![], vec![1], vec![]];
        let t = topology_from_ports(&open);
        assert_eq!(t, vec![(None, 1), (Some(0), 3), (Some(0), 1), (Some(2), 1)]);
        assert_eq!(open_ports_from_dl_status(1 << 11 | 1 << 15), vec![1, 3]);
    }

    #[test]
    fn paste_keeps_chain_and_unique_identity() {
        let mut p = demo_project(&demo_lib());
        p.master.ref_clock = Some(3);
        let n = p.slaves.len();
        let copy = p.slaves[1].clone();
        let at = p.paste_slave(copy.clone(), Some(1));
        assert_eq!(at, 2);
        assert_eq!(p.slaves.len(), n + 1);
        assert_ne!(p.slaves[2].uid, copy.uid);
        assert_ne!(p.slaves[2].phys_addr, copy.phys_addr);
        assert!(p.slaves[2].name.starts_with(&format!("Slave_{}", p.slaves[2].phys_addr)));
        // linear chain: every slave hangs on the one before it
        for (i, s) in p.slaves.iter().enumerate() {
            assert_eq!(s.parent, if i == 0 { None } else { Some(i - 1) });
        }
        assert_eq!(p.master.ref_clock, Some(4));
        let lay = compute_layout(&p);
        assert!(!validate(&p, &lay).iter().any(|x| x.sev == Severity::Error));
    }

    #[test]
    fn startup_values_checked_against_dictionary() {
        let mut p = demo_project(&demo_lib());
        let k = p.slaves.iter().position(|s| s.device.type_name == "SD200").unwrap();
        let mut bad = |idx: u16, sub: u8, data: Vec<u8>| CoeInitCmd {
            transition: "PS".into(),
            index: idx,
            subindex: sub,
            data,
            enabled: true,
            ..Default::default()
        };
        p.slaves[k].coe_cmds.push(bad(0x6041, 0, vec![0, 0])); // read-only
        p.slaves[k].coe_cmds.push(bad(0x6081, 0, vec![1, 0])); // UDINT given 2 bytes
        p.slaves[k].coe_cmds.push(bad(0x2345, 0, vec![1])); // unknown object
        let lay = compute_layout(&p);
        let text: Vec<String> = validate(&p, &lay).into_iter().filter(|x| x.slave == Some(k)).map(|x| x.text).collect();
        assert!(text.iter().any(|t| t.contains("6041:00") && t.contains("read-only")), "{:?}", text);
        assert!(text.iter().any(|t| t.contains("6081:00") && t.contains("UDINT")));
        assert!(text.iter().any(|t| t.contains("2345:00") && t.contains("not in the device")));
        // the ESI's own start-up value (0x6060 = 8) is fine
        assert!(!text.iter().any(|t| t.contains("6060:00")));
    }

    #[test]
    fn small_image_is_one_frame() {
        let p = demo_project(&demo_lib());
        let lay = compute_layout(&p);
        let f = cyclic_frames(&p, &lay);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].cmds.len(), 2);
        assert_eq!(f[0].cmds[0].len, lay.out_size + lay.in_size);
        assert_eq!(f[0].cmds[0].wkc, lay.lrw_wkc());
    }

    #[test]
    fn large_image_is_split_and_covered() {
        for lrw in [true, false] {
            let mut p = demo_project(&demo_lib());
            p.master.max_frame_data = 20;
            p.master.use_lrw = lrw;
            let lay = compute_layout(&p);
            let frames = cyclic_frames(&p, &lay);
            assert!(frames.len() > 1);
            let cmds: Vec<&CyclicCmd> = frames.iter().flat_map(|f| f.cmds.iter()).filter(|c| c.cmd != CMD_BRD).collect();
            // contiguous, no gaps or overlaps, each piece within the limit
            let mut next = lay.out_log;
            for c in cmds.iter() {
                assert_eq!(c.addr, next);
                assert!(c.len <= 20 && c.len > 0);
                next += c.len;
            }
            assert_eq!(next, lay.out_log + lay.out_size + lay.in_size);
            for f in frames.iter() {
                assert!(f.bytes() <= 20 + 14, "frame of {} bytes", f.bytes());
            }
            // every slave with process data is counted by at least one command
            let w = process_data_wkc(&frames);
            let min = if lrw {
                lay.lrw_wkc() as u32
            } else {
                (lay.n_out_slaves + lay.n_in_slaves) as u32
            };
            assert!(w >= min);
        }
    }

    #[test]
    fn split_frames_round_trip_through_eni() {
        let lib = demo_lib();
        for limit in [20u32, 24, 40, 64] {
            let mut p = demo_project(&lib);
            p.master.max_frame_data = limit;
            let xml = crate::eni::export_eni(&p);
            assert!(xml.matches("<Frame>").count() > 1);
            if let Ok(dir) = std::env::var("ECAT_DUMP_SPLIT_ENI") {
                std::fs::write(format!("{}/split_{}.xml", dir, limit), &xml).unwrap();
            }
            let (q, _) = crate::eni::import_eni(&xml, &lib).unwrap();
            assert_eq!(crate::eni::export_eni(&q), xml, "limit {}", limit);
        }
    }

    #[test]
    fn modules_shift_pdos_and_objects() {
        let d = parse_esi(SAMPLE_MODULAR, "m").unwrap().remove(0);
        let mut s = SlaveCfg::from_device(&d, 1001);
        assert!(s.modules.is_empty());
        s.set_modules(vec![ModuleInst { slot: 0, ident: 0x101 }, ModuleInst { slot: 0, ident: 0x103 }]);
        let tx: Vec<&PdoCfg> = s.pdos.iter().filter(|p| p.is_tx).collect();
        // base status PDO + slot 1 (DI8 at 0x1A01) + slot 2 (AI2 at 0x1A02, objects +0x10)
        assert_eq!(tx.len(), 3);
        assert_eq!(tx[1].pdo.index, 0x1A01);
        assert_eq!(tx[2].pdo.index, 0x1A02);
        assert_eq!(tx[2].pdo.entries[0].index, 0x6010);
        assert!(tx.iter().all(|p| p.assigned));
        let mut p = Project::default();
        p.slaves.push(s);
        let c = slave_coe_cmds(&p, 0);
        let f030: Vec<&CoeInitCmd> = c.iter().filter(|c| c.index == 0xF030).collect();
        assert_eq!(f030.len(), 4);
        assert_eq!(f030[2].data, 0x103u32.to_le_bytes().to_vec());
        // module start-up command moved to slot 2's object range
        assert!(c.iter().any(|c| c.index == 0x8010 && c.subindex == 1));
        assert_eq!(modules_from_idents(&d, &[0x101, 0x103]), p.slaves[0].modules);
    }
}
