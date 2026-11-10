//! ENI (EtherCAT Network Information, ETG.2100) writer and reader.

use crate::esi::*;
use crate::model::*;
use crate::util::*;
use std::fmt::Write;

struct X {
    s: String,
    depth: usize,
}

impl X {
    fn open(&mut self, tag: &str) {
        let _ = writeln!(self.s, "{}<{}>", "  ".repeat(self.depth), tag);
        self.depth += 1;
    }
    fn open_attr(&mut self, tag: &str, attrs: &str) {
        let _ = writeln!(self.s, "{}<{} {}>", "  ".repeat(self.depth), tag, attrs);
        self.depth += 1;
    }
    fn close(&mut self, tag: &str) {
        self.depth -= 1;
        let _ = writeln!(self.s, "{}</{}>", "  ".repeat(self.depth), tag);
    }
    fn leaf(&mut self, tag: &str, v: impl std::fmt::Display) {
        let v = xml_escape(&v.to_string());
        let _ = writeln!(self.s, "{}<{}>{}</{}>", "  ".repeat(self.depth), tag, v, tag);
    }
}

fn mac_hex(m: &str) -> String {
    let b = hex_to_bytes(m);
    if b.len() == 6 {
        bytes_to_hex(&b)
    } else {
        "ffffffffffff".into()
    }
}

fn write_init_cmd(x: &mut X, c: &EcInitCmd) {
    x.open("InitCmd");
    for t in c.transitions.iter() {
        x.leaf("Transition", t);
    }
    if c.before_slave {
        x.leaf("BeforeSlave", "true");
    }
    x.leaf("Comment", &c.comment);
    x.leaf("Requires", &c.requires);
    x.leaf("Cmd", c.cmd);
    if matches!(c.cmd, CMD_LRD | CMD_LWR | 12) {
        x.leaf("Addr", c.adp as u32 | (c.ado as u32) << 16);
    } else {
        x.leaf("Adp", c.adp);
        x.leaf("Ado", c.ado);
    }
    if c.data.len() > 16 && c.data.iter().all(|b| *b == 0) {
        x.leaf("DataLength", c.data.len());
    } else {
        x.leaf("Data", bytes_to_hex(&c.data));
    }
    if let Some(n) = c.cnt {
        x.leaf("Cnt", n);
    }
    x.leaf("Retries", c.retries);
    if let Some(v) = &c.validate {
        x.open("Validate");
        x.leaf("Data", bytes_to_hex(&v.data));
        if let Some(m) = &v.mask {
            x.leaf("DataMask", bytes_to_hex(m));
        }
        x.leaf("Timeout", v.timeout_ms);
        x.close("Validate");
    }
    x.close("InitCmd");
}

fn write_pdo(x: &mut X, tag: &str, pc: &PdoCfg) {
    let mut attrs = String::new();
    if pc.assigned {
        let _ = write!(attrs, "Sm=\"{}\" ", pc.sm);
    }
    let _ = write!(attrs, "Fixed=\"{}\"", if pc.pdo.fixed { "true" } else { "false" });
    if pc.pdo.mandatory {
        attrs.push_str(" Mandatory=\"true\"");
    }
    x.open_attr(tag, &attrs);
    x.leaf("Index", hx(pc.pdo.index as u32, 4));
    x.leaf("Name", &pc.pdo.name);
    for e in pc.pdo.excludes.iter() {
        x.leaf("Exclude", hx(*e as u32, 4));
    }
    for e in pc.pdo.entries.iter() {
        x.open("Entry");
        x.leaf("Index", hx(e.index as u32, 4));
        if !e.is_gap() {
            x.leaf("SubIndex", e.subindex);
        }
        x.leaf("BitLen", e.bitlen);
        if !e.is_gap() {
            x.leaf("Name", &e.name);
            x.leaf("DataType", &e.data_type);
        }
        x.close("Entry");
    }
    x.close(tag);
}

pub fn export_eni(p: &Project) -> String {
    let lay = compute_layout(p);
    let mut x = X { s: String::new(), depth: 0 };
    x.s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.open_attr(
        "EtherCATConfig",
        "Version=\"1.3\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"EtherCATConfig.xsd\"",
    );
    x.open("Config");
    // ---- master
    x.open("Master");
    x.open("Info");
    x.leaf("Name", &p.master.name);
    x.leaf("Destination", mac_hex(&p.master.dst_mac));
    x.leaf("Source", mac_hex(&p.master.src_mac));
    x.leaf("EtherType", bytes_to_hex(&p.master.ether_type.to_be_bytes()));
    x.close("Info");
    x.open("InitCmds");
    for c in master_init_cmds(p) {
        write_init_cmd(&mut x, &c);
    }
    x.close("InitCmds");
    x.close("Master");
    let refc = p.ref_clock_index();
    // ---- slaves
    for (i, s) in p.slaves.iter().enumerate() {
        let sl = &lay.slaves[i];
        x.open("Slave");
        x.open("Info");
        x.leaf("Name", &s.name);
        x.leaf("PhysAddr", s.phys_addr);
        x.leaf("AutoIncAddr", Project::auto_inc(i));
        x.leaf("Physics", &s.device.physics);
        x.leaf("VendorId", s.device.vendor_id);
        x.leaf("ProductCode", s.device.product_code);
        x.leaf("RevisionNo", s.device.revision);
        x.leaf("SerialNo", s.serial);
        x.leaf("ProductRevision", format!("{} ({})", s.device.type_name, hx(s.device.revision, 8)));
        x.close("Info");
        // process data
        let has_pd = !s.pdos.is_empty() || sl.sms.iter().any(|m| matches!(m.kind, SmKind::Outputs | SmKind::Inputs));
        if has_pd {
            x.open("ProcessData");
            if sl.out_bytes > 0 {
                x.open("Send");
                x.leaf("BitStart", sl.out_offs * 8);
                x.leaf("BitLength", sl.out_bytes * 8);
                x.close("Send");
            }
            if sl.in_bytes > 0 {
                x.open("Recv");
                x.leaf("BitStart", (lay.out_size + sl.in_offs) * 8);
                x.leaf("BitLength", sl.in_bytes * 8);
                x.close("Recv");
            }
            for sm in sl.sms.iter().filter(|m| matches!(m.kind, SmKind::Outputs | SmKind::Inputs)) {
                let tag = format!("Sm{}", sm.index);
                x.open(&tag);
                x.leaf("Type", sm.kind.text());
                x.leaf("StartAddress", hx(sm.start as u32, 4));
                x.leaf("ControlByte", hx(sm.control as u32, 2));
                x.leaf("Enable", if sm.enable && sm.len > 0 { "true" } else { "false" });
                x.leaf("DefaultSize", sm.len);
                for pdo in sm.pdos.iter() {
                    x.leaf("Pdo", hx(*pdo as u32, 4));
                }
                x.close(&tag);
            }
            for pc in s.pdos.iter().filter(|p| p.is_tx) {
                write_pdo(&mut x, "TxPdo", pc);
            }
            for pc in s.pdos.iter().filter(|p| !p.is_tx) {
                write_pdo(&mut x, "RxPdo", pc);
            }
            x.close("ProcessData");
        }
        // mailbox
        if s.has_mailbox() {
            let mb = s.device.mailbox.clone().unwrap_or_default();
            let out = sl.sms.iter().find(|m| m.kind == SmKind::MBoxOut);
            let inn = sl.sms.iter().find(|m| m.kind == SmKind::MBoxIn);
            x.open_attr("Mailbox", &format!("DataLinkLayer=\"{}\"", mb.data_link_layer));
            if let Some(o) = out {
                x.open("Send");
                x.leaf("Start", o.start);
                x.leaf("Length", o.len);
                x.close("Send");
            }
            if let Some(o) = inn {
                x.open("Recv");
                x.leaf("Start", o.start);
                x.leaf("Length", o.len);
                x.leaf("PollTime", 20);
                x.close("Recv");
            }
            for pr in mb.protocols() {
                x.leaf("Protocol", pr);
            }
            if let Some(coe) = &mb.coe {
                x.open("CoE");
                x.open("InitCmds");
                for c in slave_coe_cmds(p, i) {
                    x.open_attr(
                        "InitCmd",
                        &format!(
                            "Fixed=\"{}\" CompleteAccess=\"{}\"{}",
                            c.fixed,
                            c.complete_access,
                            if c.generated { " Generated=\"true\"" } else { "" }
                        ),
                    );
                    x.leaf("Transition", &c.transition);
                    x.leaf("Comment", &c.comment);
                    x.leaf("Timeout", c.timeout_ms);
                    x.leaf("Ccs", 1);
                    x.leaf("Index", c.index);
                    x.leaf("SubIndex", c.subindex);
                    x.leaf("Data", bytes_to_hex(&c.data));
                    x.close("InitCmd");
                }
                x.close("InitCmds");
                let _ = coe;
                x.close("CoE");
            }
            let soe: Vec<&crate::proto::SoeInitCmd> = s.soe_cmds.iter().filter(|c| c.enabled).collect();
            if mb.soe && !soe.is_empty() {
                x.open("SoE");
                x.open("InitCmds");
                for c in soe {
                    x.open("InitCmd");
                    x.leaf("Transition", &c.transition);
                    x.leaf("Comment", &c.comment);
                    x.leaf("Timeout", p.master.timeout_mailbox_ms);
                    x.leaf("OpCode", 3);
                    x.leaf("DriveNo", c.drive_no);
                    x.leaf("IDN", c.idn);
                    x.leaf("Elements", 64);
                    x.leaf("Attribute", 0);
                    x.leaf("Data", bytes_to_hex(&c.data));
                    x.close("InitCmd");
                }
                x.close("InitCmds");
                x.close("SoE");
            }
            if mb.eoe && s.eoe.enabled {
                if let Ok(d) = crate::proto::eoe_set_ip_data(&s.eoe) {
                    x.open("EoE");
                    x.open("InitCmds");
                    x.open("InitCmd");
                    x.leaf("Transition", "PS");
                    x.leaf("Comment", "set IP parameter");
                    x.leaf("Timeout", p.master.timeout_mailbox_ms);
                    x.leaf("Type", 2);
                    x.leaf("Data", bytes_to_hex(&d));
                    x.close("InitCmd");
                    x.close("InitCmds");
                    x.close("EoE");
                }
            }
            x.close("Mailbox");
        }
        x.open("InitCmds");
        for c in slave_init_cmds(p, &lay, i) {
            write_init_cmd(&mut x, &c);
        }
        x.close("InitCmds");
        if i > 0 {
            let par = s.parent.unwrap_or(i - 1).min(i - 1);
            x.open_attr("PreviousPort", "Selected=\"1\"");
            x.leaf("Port", port_letter(s.parent_port));
            x.leaf("PhysAddr", p.slaves[par].phys_addr);
            x.close("PreviousPort");
        }
        if p.master.dc_enabled && (s.dc_active() || refc == Some(i)) {
            x.open("DC");
            if refc == Some(i) {
                x.leaf("ReferenceClock", "true");
            }
            if let Some(m) = s.dc_opmode() {
                x.leaf("OpMode", &m.name);
                x.leaf("AssignActivate", hx(m.assign_activate as u32, 4));
            }
            x.leaf("CycleTime0", s.sync0_cycle(p.master.cycle_time_us));
            x.leaf("CycleTime1", s.sync1_cycle(p.master.cycle_time_us));
            x.leaf("ShiftTime0", s.sync0_shift_ns);
            x.leaf("ShiftTime1", s.sync1_shift_ns);
            x.close("DC");
        }
        x.close("Slave");
    }
    // ---- cyclic
    x.open("Cyclic");
    x.leaf("CycleTime", p.master.cycle_time_us);
    let total = lay.out_size + lay.in_size;
    for f in cyclic_frames(p, &lay) {
        x.open("Frame");
        for c in f.cmds.iter() {
            x.open("Cmd");
            x.leaf("State", "PREOP");
            x.leaf("State", "SAFEOP");
            x.leaf("State", "OP");
            x.leaf("Comment", &c.comment);
            x.leaf("Cmd", c.cmd);
            if c.cmd == CMD_BRD {
                x.leaf("Adp", c.addr & 0xFFFF);
                x.leaf("Ado", c.addr >> 16);
            } else {
                x.leaf("Addr", c.addr);
            }
            x.leaf("DataLength", c.len);
            x.leaf("Cnt", c.wkc);
            x.leaf("InputOffs", c.in_offs);
            if c.cmd != CMD_BRD {
                x.leaf("OutputOffs", c.out_offs);
            }
            x.close("Cmd");
        }
        x.close("Frame");
    }
    x.close("Cyclic");

    // ---- process image
    x.open("ProcessImage");
    x.open("Inputs");
    x.leaf("ByteSize", total + 2);
    for v in lay.vars.iter().filter(|v| v.is_input) {
        write_var(&mut x, v);
    }
    x.close("Inputs");
    x.open("Outputs");
    x.leaf("ByteSize", lay.out_size);
    for v in lay.vars.iter().filter(|v| !v.is_input) {
        write_var(&mut x, v);
    }
    x.close("Outputs");
    x.close("ProcessImage");
    x.close("Config");
    x.close("EtherCATConfig");
    x.s
}

fn write_var(x: &mut X, v: &Variable) {
    x.open("Variable");
    x.leaf("Name", &v.name);
    x.leaf("DataType", &v.data_type);
    x.leaf("BitSize", v.bit_size);
    x.leaf("BitOffs", v.bit_offs);
    x.close("Variable");
}

// ------------------------------------------------------------------ import

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}
fn kids<'a, 'i: 'a>(n: roxmltree::Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}
fn txt(n: Option<roxmltree::Node>) -> String {
    n.and_then(|n| n.text()).unwrap_or("").trim().to_string()
}
fn num(n: Option<roxmltree::Node>) -> i64 {
    parse_num(&txt(n)).unwrap_or(0)
}

fn read_pdo(n: roxmltree::Node) -> (Pdo, bool) {
    let assigned = n.attribute("Sm").is_some();
    let pdo = Pdo {
        index: num(child(n, "Index")) as u16,
        name: txt(child(n, "Name")),
        sm: n.attribute("Sm").and_then(parse_num).map(|v| v as u8),
        fixed: n.attribute("Fixed").map(parse_bool).unwrap_or(false),
        mandatory: n.attribute("Mandatory").map(parse_bool).unwrap_or(false),
        excludes: kids(n, "Exclude").map(|e| parse_u32(e.text().unwrap_or("")) as u16).collect(),
        entries: kids(n, "Entry")
            .map(|e| PdoEntry {
                index: num(child(e, "Index")) as u16,
                subindex: num(child(e, "SubIndex")) as u8,
                bitlen: num(child(e, "BitLen")) as u16,
                name: txt(child(e, "Name")),
                data_type: txt(child(e, "DataType")),
            })
            .collect(),
    };
    (pdo, assigned)
}

/// Import guarded against internal errors: the file is read, laid out, validated and
/// re-exported once; any failure is reported as an error message instead of ending the program.
pub fn import_eni_safe(xml: &str, lib: &EsiLibrary) -> Result<(Project, Vec<String>), String> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (p, notes) = import_eni_raw(xml, lib)?;
        let lay = compute_layout(&p);
        let _ = validate(&p, &lay);
        for i in 0..p.slaves.len() {
            let _ = slave_init_cmds(&p, &lay, i);
            let _ = slave_coe_cmds(&p, i);
        }
        let _ = export_eni(&p);
        Ok((p, notes))
    }));
    match r {
        Ok(v) => v,
        Err(e) => {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown error".into());
            Err(format!("the file contains values this version cannot handle ({})", msg))
        }
    }
}

pub fn import_eni(xml: &str, lib: &EsiLibrary) -> Result<(Project, Vec<String>), String> {
    import_eni_safe(xml, lib)
}

/// Read an ENI file back into a project. Devices are matched against the ESI library when possible.
fn import_eni_raw(xml: &str, lib: &EsiLibrary) -> Result<(Project, Vec<String>), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML error: {}", e))?;
    let root = doc.root_element();
    if root.tag_name().name() != "EtherCATConfig" {
        return Err(format!("not an ENI file (root element is <{}>)", root.tag_name().name()));
    }
    let cfg = child(root, "Config").ok_or("missing <Config>")?;
    let mut notes = Vec::new();
    let mut p = Project::default();
    if let Some(m) = child(cfg, "Master") {
        if let Some(info) = child(m, "Info") {
            p.master.name = txt(child(info, "Name"));
            let fmt_mac = |h: String| {
                let b = hex_to_bytes(&h);
                b.iter().map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(":")
            };
            let d = txt(child(info, "Destination"));
            if !d.is_empty() {
                p.master.dst_mac = fmt_mac(d);
            }
            let s = txt(child(info, "Source"));
            if !s.is_empty() {
                p.master.src_mac = fmt_mac(s);
            }
            let et = hex_to_bytes(&txt(child(info, "EtherType")));
            if et.len() == 2 {
                p.master.ether_type = u16::from_be_bytes([et[0], et[1]]);
            }
        }
    }
    if let Some(c) = child(cfg, "Cyclic") {
        let ct = num(child(c, "CycleTime"));
        if ct > 0 {
            // Most tools write microseconds; some write nanoseconds (e.g. 1000000 for 1 ms).
            let us = if ct >= 1_000_000 && ct % 1000 == 0 {
                notes.push(format!("Cycle time {} read as nanoseconds ({} µs)", ct, ct / 1000));
                ct / 1000
            } else {
                ct
            };
            p.master.cycle_time_us = us.clamp(1, 10_000_000) as u32;
        }
        let mut lrw = false;
        let mut first_log: Option<u32> = None;
        let mut frame_sizes: Vec<u32> = Vec::new();
        let mut pd_cmds = 0usize;
        for f in kids(c, "Frame") {
            frame_sizes.push(kids(f, "Cmd").fold(2u32, |acc, cmd| {
                acc.saturating_add(12)
                    .saturating_add(num(child(cmd, "DataLength")).clamp(0, u32::MAX as i64) as u32)
            }));
            pd_cmds += kids(f, "Cmd")
                .filter(|cmd| matches!(num(child(*cmd, "Cmd")) as u8, CMD_LRW | CMD_LWR | CMD_LRD))
                .count();
            for cmd in kids(f, "Cmd") {
                let code = num(child(cmd, "Cmd")) as u8;
                if matches!(code, CMD_LRW | CMD_LWR | CMD_LRD) {
                    if code == CMD_LRW {
                        lrw = true;
                    }
                    let a = num(child(cmd, "Addr")) as u32;
                    first_log = Some(first_log.map(|x: u32| x.min(a)).unwrap_or(a));
                }
            }
        }
        p.master.use_lrw = lrw || first_log.is_none();
        // process data was split: keep the frame size so it is split the same way again
        if frame_sizes.len() > 1 || pd_cmds > 2 {
            if let Some(m) = frame_sizes.iter().copied().max() {
                p.master.max_frame_data = m.saturating_sub(14).clamp(16, ETH_PAYLOAD - 14);
            }
        }
        if let Some(a) = first_log {
            p.master.log_start = a;
        }
    }
    let mut ref_clock = None;
    let mut parents: Vec<Option<(u16, u8)>> = Vec::new();
    for (si, s) in kids(cfg, "Slave").enumerate() {
        let info = child(s, "Info").ok_or("slave without <Info>")?;
        let name = txt(child(info, "Name"));
        let phys = num(child(info, "PhysAddr")) as u16;
        let vendor = num(child(info, "VendorId")) as u32;
        let product = num(child(info, "ProductCode")) as u32;
        let revision = num(child(info, "RevisionNo")) as u32;
        let pd = child(s, "ProcessData");
        let eni_pdos: Vec<(Pdo, bool, bool)> = pd
            .map(|pd| {
                let mut v: Vec<(Pdo, bool, bool)> = kids(pd, "RxPdo")
                    .map(|n| {
                        let (a, b) = read_pdo(n);
                        (a, b, false)
                    })
                    .collect();
                v.extend(kids(pd, "TxPdo").map(|n| {
                    let (a, b) = read_pdo(n);
                    (a, b, true)
                }));
                v
            })
            .unwrap_or_default();
        // PDOs listed inside <SmN><Pdo>
        let mut sm_assign: Vec<(u8, u16)> = Vec::new();
        let mut eni_sms: Vec<(u8, SmDef)> = Vec::new();
        if let Some(pd) = pd {
            for k in 0..16u8 {
                if let Some(sm) = child(pd, &format!("Sm{}", k)) {
                    for pdo in kids(sm, "Pdo") {
                        sm_assign.push((k, parse_u32(pdo.text().unwrap_or("")) as u16));
                    }
                    eni_sms.push((
                        k,
                        SmDef {
                            kind: SmKind::from_text(&txt(child(sm, "Type"))),
                            start: num(child(sm, "StartAddress")) as u16,
                            control: num(child(sm, "ControlByte")) as u8,
                            default_size: num(child(sm, "DefaultSize")) as u16,
                            min_size: None,
                            max_size: None,
                            enable: parse_bool(&txt(child(sm, "Enable"))),
                        },
                    ));
                }
            }
        }
        let mbx = child(s, "Mailbox");
        // DC activation word as written in the ENI (DC block or the 0x0980 init command)
        let eni_dc = child(s, "DC");
        let eni_aa: Option<u16> = eni_dc
            .and_then(|dc| child(dc, "AssignActivate"))
            .and_then(|n| parse_num(n.text().unwrap_or("")))
            .map(|v| v as u16)
            .or_else(|| {
                child(s, "InitCmds").and_then(|ics| {
                    kids(ics, "InitCmd")
                        .find(|c| num(child(*c, "Cmd")) as u8 == CMD_FPWR && num(child(*c, "Ado")) == 0x0980)
                        .map(|c| hex_to_bytes(&txt(child(c, "Data"))))
                        .filter(|d| d.len() >= 2)
                        .map(|d| u16::from_le_bytes([d[0], d[1]]))
                })
            });
        let synth_dc: Vec<DcOpMode> = {
            let mut v = Vec::new();
            if let Some(aa) = eni_aa.filter(|a| *a != 0) {
                v.push(DcOpMode {
                    name: "DC".into(),
                    desc: "DC-Synchron (from ENI)".into(),
                    assign_activate: aa,
                    cycle_sync0_factor: 1,
                    shift_sync0: 0,
                    cycle_sync1_factor: if aa & 0x0400 != 0 { 1 } else { 0 },
                    shift_sync1: 0,
                });
            }
            if !v.is_empty() || eni_dc.is_some() {
                v.push(DcOpMode {
                    name: "FreeRun".into(),
                    desc: "FreeRun".into(),
                    ..Default::default()
                });
            }
            v
        };
        let dev = match lib.find_best(vendor, product, revision) {
            Some(d) => {
                if d.revision != revision {
                    notes.push(format!("{}: revision {} not in library, using {}", name, hx(revision, 8), hx(d.revision, 8)));
                }
                let mut d = d.clone();
                d.revision = revision;
                d
            }
            None => {
                notes.push(format!("{}: device not found in ESI library, rebuilt from ENI data", name));
                // synthesize a device description from the ENI
                let mut sms: Vec<SmDef> = Vec::new();
                let mut put = |k: usize, sm: SmDef| {
                    while sms.len() <= k {
                        sms.push(SmDef {
                            kind: SmKind::Unused,
                            start: 0,
                            control: 0,
                            default_size: 0,
                            min_size: None,
                            max_size: None,
                            enable: false,
                        });
                    }
                    sms[k] = sm;
                };
                let mut mailbox = None;
                if let Some(m) = mbx {
                    let so = child(m, "Send");
                    let ro = child(m, "Recv");
                    put(
                        0,
                        SmDef {
                            kind: SmKind::MBoxOut,
                            start: num(so.and_then(|n| child(n, "Start"))) as u16,
                            control: 0x26,
                            default_size: num(so.and_then(|n| child(n, "Length"))) as u16,
                            min_size: None,
                            max_size: None,
                            enable: true,
                        },
                    );
                    put(
                        1,
                        SmDef {
                            kind: SmKind::MBoxIn,
                            start: num(ro.and_then(|n| child(n, "Start"))) as u16,
                            control: 0x22,
                            default_size: num(ro.and_then(|n| child(n, "Length"))) as u16,
                            min_size: None,
                            max_size: None,
                            enable: true,
                        },
                    );
                    let protos: Vec<String> = kids(m, "Protocol").map(|n| n.text().unwrap_or("").trim().to_string()).collect();
                    let has = |x: &str| protos.iter().any(|p| p == x);
                    mailbox = Some(MailboxDef {
                        coe: if has("CoE") || child(m, "CoE").is_some() {
                            Some(CoeDef {
                                sdo_info: true,
                                pdo_assign: true,
                                pdo_config: true,
                                complete_access: false,
                                segmented_sdo: true,
                                init_cmds: vec![],
                            })
                        } else {
                            None
                        },
                        foe: has("FoE"),
                        eoe: has("EoE"),
                        soe: has("SoE"),
                        voe: has("VoE"),
                        aoe: has("AoE"),
                        data_link_layer: m.attribute("DataLinkLayer").map(parse_bool).unwrap_or(false),
                    });
                }
                for (k, sm) in eni_sms.iter() {
                    put(*k as usize, sm.clone());
                }
                EsiDevice {
                    vendor_id: vendor,
                    vendor_name: format!("Vendor {}", hx(vendor, 8)),
                    type_name: name.clone(),
                    name: name.clone(),
                    product_code: product,
                    revision,
                    group_type: "Imported".into(),
                    group_name: "Imported from ENI".into(),
                    physics: {
                        let ph = txt(child(info, "Physics"));
                        if ph.is_empty() {
                            "YY".into()
                        } else {
                            ph
                        }
                    },
                    fmmus: vec!["Outputs".into(), "Inputs".into()],
                    sms,
                    rx_pdos: eni_pdos.iter().filter(|x| !x.2).map(|x| x.0.clone()).collect(),
                    tx_pdos: eni_pdos.iter().filter(|x| x.2).map(|x| x.0.clone()).collect(),
                    mailbox,
                    dc_opmodes: synth_dc.clone(),
                    eeprom: None,
                    profile_no: None,
                    source_file: "ENI".into(),
                    slots: vec![],
                    modules: vec![],
                    dictionary: vec![],
                }
            }
        };
        let mut sc = SlaveCfg::from_device(&dev, phys);
        sc.name = name.clone();
        sc.serial = num(child(info, "SerialNo")) as u32;
        // modular devices: rebuild the slots from the configured module list (0xF030)
        if !sc.device.slots.is_empty() {
            if let Some(ics) = mbx.and_then(|m| child(m, "CoE")).and_then(|c| child(c, "InitCmds")) {
                let mut list: std::collections::BTreeMap<u8, u32> = std::collections::BTreeMap::new();
                for c in kids(ics, "InitCmd") {
                    if num(child(c, "Index")) as u16 == 0xF030 {
                        let sub = num(child(c, "SubIndex")) as u8;
                        let d = hex_to_bytes(&txt(child(c, "Data")));
                        if sub > 0 && d.len() >= 4 {
                            list.insert(sub, u32::from_le_bytes([d[0], d[1], d[2], d[3]]));
                        }
                    }
                }
                let idents: Vec<u32> = list.values().copied().collect();
                let mods = modules_from_idents(&sc.device, &idents);
                if mods.len() != idents.len() {
                    notes.push(format!(
                        "{}: {} of {} module(s) could not be matched to a slot",
                        name,
                        idents.len() - mods.len(),
                        idents.len()
                    ));
                }
                sc.set_modules(mods);
            }
        }
        // apply assignment / mapping from the ENI
        if !eni_pdos.is_empty() || !sm_assign.is_empty() {
            for pc in sc.pdos.iter_mut() {
                let in_sm = sm_assign.iter().find(|(_, x)| *x == pc.pdo.index);
                let in_list = eni_pdos.iter().find(|x| x.0.index == pc.pdo.index && x.2 == pc.is_tx);
                pc.assigned = in_sm.is_some() || in_list.map(|x| x.1).unwrap_or(false);
                if let Some((k, _)) = in_sm {
                    pc.sm = *k;
                } else if let Some(sm) = in_list.and_then(|x| x.0.sm) {
                    pc.sm = sm;
                }
                if let Some(x) = in_list {
                    if !pc.pdo.fixed && !x.0.entries.is_empty() {
                        pc.pdo.entries = x.0.entries.clone();
                    }
                }
            }
        }
        if let Some(m) = mbx {
            let l_out = num(child(m, "Send").and_then(|n| child(n, "Length"))) as u16;
            let l_in = num(child(m, "Recv").and_then(|n| child(n, "Length"))) as u16;
            if l_out > 0 {
                sc.mbx_sizes.0 = l_out;
            }
            if l_in > 0 {
                sc.mbx_sizes.1 = l_in;
            }
            if let Some(ics) = child(m, "CoE").and_then(|c| child(c, "InitCmds")) {
                let mut cmds = Vec::new();
                for c in kids(ics, "InitCmd") {
                    let index = num(child(c, "Index")) as u16;
                    let generated = c.attribute("Generated").map(parse_bool).unwrap_or(false)
                        || (0x1C10..=0x1C2F).contains(&index)
                        || index == 0xF030
                        || (0x1600..=0x17FF).contains(&index)
                        || (0x1A00..=0x1BFF).contains(&index);
                    if generated {
                        continue;
                    }
                    cmds.push(CoeInitCmd {
                        transition: kids(c, "Transition").next().and_then(|t| t.text()).unwrap_or("PS").trim().to_string(),
                        index,
                        subindex: num(child(c, "SubIndex")) as u8,
                        data: hex_to_bytes(&txt(child(c, "Data"))),
                        comment: txt(child(c, "Comment")),
                        complete_access: c.attribute("CompleteAccess").map(parse_bool).unwrap_or(false),
                        fixed: c.attribute("Fixed").map(parse_bool).unwrap_or(false),
                        enabled: true,
                        generated: false,
                        timeout_ms: {
                            let t = num(child(c, "Timeout")) as u32;
                            if t == 0 {
                                3000
                            } else {
                                t
                            }
                        },
                    });
                }
                sc.coe_cmds = cmds;
            }
            if let Some(ics) = child(m, "SoE").and_then(|c| child(c, "InitCmds")) {
                sc.soe_cmds = kids(ics, "InitCmd")
                    .map(|c| crate::proto::SoeInitCmd {
                        transition: kids(c, "Transition").next().and_then(|t| t.text()).unwrap_or("PS").trim().to_string(),
                        drive_no: num(child(c, "DriveNo")) as u8,
                        idn: num(child(c, "IDN")) as u16,
                        data: hex_to_bytes(&txt(child(c, "Data"))),
                        comment: txt(child(c, "Comment")),
                        enabled: true,
                    })
                    .collect();
            }
            if let Some(ics) = child(m, "EoE").and_then(|c| child(c, "InitCmds")) {
                for c in kids(ics, "InitCmd") {
                    if let Some(cfg) = crate::proto::eoe_parse_set_ip(&hex_to_bytes(&txt(child(c, "Data")))) {
                        sc.eoe = cfg;
                    }
                }
            }
        }
        // identity checks from the init commands
        if let Some(ics) = child(s, "InitCmds") {
            let comments: Vec<String> = kids(ics, "InitCmd").map(|c| txt(child(c, "Comment")).to_lowercase()).collect();
            let has = |w: &str| comments.iter().any(|c| c.starts_with("check") && c.contains(w));
            if comments.iter().any(|c| c.contains("vendor") || c.contains("product")) {
                sc.check_vendor = has("vendor");
                sc.check_product = has("product");
            }
            if has("revision") {
                sc.check_revision = RevCheck::Equal;
            }
            sc.check_serial = has("serial");
            for c in kids(ics, "InitCmd") {
                if num(child(c, "Cmd")) as u8 == CMD_FPWR && num(child(c, "Ado")) == 0x0980 {
                    let d = hex_to_bytes(&txt(child(c, "Data")));
                    if d.len() >= 2 {
                        let aa = u16::from_le_bytes([d[0], d[1]]);
                        if aa != 0 {
                            sc.dc_mode = sc.device.dc_opmodes.iter().position(|m| m.assign_activate == aa).or(sc.dc_mode);
                        }
                    }
                }
                if num(child(c, "Cmd")) as u8 == CMD_FPWR && num(child(c, "Ado")) == 0x0420 {
                    let d = hex_to_bytes(&txt(child(c, "Data")));
                    if d.len() >= 2 {
                        sc.watchdog_enabled = true;
                        sc.watchdog_ms = u16::from_le_bytes([d[0], d[1]]) as u32 / 10;
                    }
                }
            }
        }
        match child(s, "DC") {
            Some(dc) => {
                if parse_bool(&txt(child(dc, "ReferenceClock"))) {
                    ref_clock = Some(si);
                }
                let opm = txt(child(dc, "OpMode"));
                if !opm.is_empty() {
                    if let Some(k) = sc.device.dc_opmodes.iter().position(|m| m.name == opm) {
                        sc.dc_mode = Some(k);
                    }
                }
                let c0 = num(child(dc, "CycleTime0")) as u32;
                if c0 != p.master.cycle_time_us.saturating_mul(1000) {
                    sc.sync0_cycle_ns = c0;
                }
                let c1 = num(child(dc, "CycleTime1")) as u32;
                if c1 != sc.sync1_cycle(p.master.cycle_time_us) {
                    sc.sync1_cycle_ns = c1;
                }
                sc.sync0_shift_ns = num(child(dc, "ShiftTime0").or_else(|| child(dc, "ShiftTime"))) as i32;
                sc.sync1_shift_ns = num(child(dc, "ShiftTime1")) as i32;
            }
            None => {
                if !sc.device.dc_opmodes.is_empty() {
                    sc.dc_mode = sc.device.dc_opmodes.iter().position(|m| m.assign_activate == 0);
                }
            }
        }
        parents.push(child(s, "PreviousPort").map(|pp| {
            let port = txt(child(pp, "Port")).chars().next().unwrap_or('B');
            (num(child(pp, "PhysAddr")) as u16, (port as u8).saturating_sub(b'A').min(3))
        }));
        p.slaves.push(sc);
    }
    for (i, par) in parents.into_iter().enumerate() {
        match par {
            Some((addr, port)) => {
                p.slaves[i].parent = p.slaves.iter().position(|s| s.phys_addr == addr).or(if i > 0 { Some(i - 1) } else { None });
                p.slaves[i].parent_port = port;
            }
            None => {
                p.slaves[i].parent = if i > 0 { Some(i - 1) } else { None };
            }
        }
    }
    p.master.ref_clock = ref_clock;
    p.master.dc_enabled = ref_clock.is_some() || p.slaves.iter().any(|s| s.dc_active());
    Ok((p, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let mut lib = EsiLibrary::default();
        lib.add_xml(SAMPLE_IO, "io").unwrap();
        lib.add_xml(SAMPLE_DRIVE, "drv").unwrap();
        lib.add_xml(SAMPLE_MODULAR, "mod").unwrap();
        let mut p = demo_project(&lib);
        let k = p.slaves.iter().position(|s| s.device.type_name == "SD200").unwrap();
        p.slaves[k].soe_cmds.push(crate::proto::SoeInitCmd {
            transition: "PS".into(),
            drive_no: 0,
            idn: crate::proto::parse_idn("S-0-1000").unwrap(),
            data: vec![0x10, 0x27],
            comment: "test".into(),
            enabled: true,
        });
        p.slaves[k].eoe = crate::proto::EoeCfg {
            enabled: true,
            ip: "192.168.10.5".into(),
            subnet: "255.255.255.0".into(),
            ..Default::default()
        };
        let xml = export_eni(&p);
        assert!(xml.contains("<SoE>") && xml.contains("<EoE>"));
        roxmltree::Document::parse(&xml).unwrap();
        let (q, _) = import_eni(&xml, &lib).unwrap();
        assert_eq!(q.slaves.len(), p.slaves.len());
        let a = compute_layout(&p);
        let b = compute_layout(&q);
        assert_eq!(a.out_size, b.out_size);
        assert_eq!(a.in_size, b.in_size);
        assert_eq!(export_eni(&q), xml);
        assert_eq!(q.slaves[k].soe_cmds, p.slaves[k].soe_cmds);
        assert_eq!(q.slaves[k].eoe, p.slaves[k].eoe);
        // the modular station keeps its three modules through the round trip
        let m = q.slaves.iter().find(|s| !s.device.slots.is_empty()).unwrap();
        assert_eq!(m.modules.iter().map(|x| x.ident).collect::<Vec<_>>(), vec![0x101, 0x102, 0x103]);
    }
}
