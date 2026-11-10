//! ESI (EtherCAT Slave Information, ETG.2000) model and parser.

use crate::util::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmKind {
    MBoxOut,
    MBoxIn,
    Outputs,
    Inputs,
    Unused,
}

impl SmKind {
    pub fn from_text(s: &str) -> SmKind {
        match s.trim() {
            "MBoxOut" => SmKind::MBoxOut,
            "MBoxIn" => SmKind::MBoxIn,
            "Outputs" => SmKind::Outputs,
            "Inputs" => SmKind::Inputs,
            _ => SmKind::Unused,
        }
    }
    pub fn text(&self) -> &'static str {
        match self {
            SmKind::MBoxOut => "MBoxOut",
            SmKind::MBoxIn => "MBoxIn",
            SmKind::Outputs => "Outputs",
            SmKind::Inputs => "Inputs",
            SmKind::Unused => "Unused",
        }
    }
    pub fn sii_code(&self) -> u8 {
        match self {
            SmKind::Unused => 0,
            SmKind::MBoxOut => 1,
            SmKind::MBoxIn => 2,
            SmKind::Outputs => 3,
            SmKind::Inputs => 4,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmDef {
    pub kind: SmKind,
    pub start: u16,
    pub control: u8,
    pub default_size: u16,
    pub min_size: Option<u16>,
    pub max_size: Option<u16>,
    pub enable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PdoEntry {
    pub index: u16,
    pub subindex: u8,
    pub bitlen: u16,
    pub name: String,
    pub data_type: String,
}

impl PdoEntry {
    pub fn is_gap(&self) -> bool {
        self.index == 0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pdo {
    pub index: u16,
    pub name: String,
    /// Sync manager named in the ESI (also means "assigned by default").
    pub sm: Option<u8>,
    pub fixed: bool,
    pub mandatory: bool,
    pub excludes: Vec<u16>,
    pub entries: Vec<PdoEntry>,
}

impl Pdo {
    pub fn bit_len(&self) -> u32 {
        self.entries.iter().map(|e| e.bitlen as u32).sum()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CoeDef {
    pub sdo_info: bool,
    pub pdo_assign: bool,
    pub pdo_config: bool,
    pub complete_access: bool,
    pub segmented_sdo: bool,
    pub init_cmds: Vec<CoeInitCmd>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MailboxDef {
    pub coe: Option<CoeDef>,
    pub foe: bool,
    pub eoe: bool,
    pub soe: bool,
    pub voe: bool,
    pub aoe: bool,
    pub data_link_layer: bool,
}

impl MailboxDef {
    pub fn protocols(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.aoe {
            v.push("AoE");
        }
        if self.eoe {
            v.push("EoE");
        }
        if self.coe.is_some() {
            v.push("CoE");
        }
        if self.foe {
            v.push("FoE");
        }
        if self.soe {
            v.push("SoE");
        }
        if self.voe {
            v.push("VoE");
        }
        v
    }
    pub fn protocol_word(&self) -> u16 {
        (self.aoe as u16) | (self.eoe as u16) << 1 | (self.coe.is_some() as u16) << 2 | (self.foe as u16) << 3 | (self.soe as u16) << 4 | (self.voe as u16) << 5
    }
}

/// A CoE SDO download executed during a state transition.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CoeInitCmd {
    pub transition: String,
    pub index: u16,
    pub subindex: u8,
    pub data: Vec<u8>,
    pub comment: String,
    pub complete_access: bool,
    /// Came from the ESI file (cannot be deleted, only disabled).
    pub fixed: bool,
    pub enabled: bool,
    /// Generated automatically from the PDO assignment / mapping.
    #[serde(default)]
    pub generated: bool,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u32,
}

fn default_timeout() -> u32 {
    3000
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DcOpMode {
    pub name: String,
    pub desc: String,
    pub assign_activate: u16,
    pub cycle_sync0_factor: i32,
    pub shift_sync0: i32,
    pub cycle_sync1_factor: i32,
    pub shift_sync1: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EepromDef {
    pub byte_size: u32,
    pub config_data: Vec<u8>,
    pub boot_strap: Vec<u8>,
}

/// One slot type of a modular device (ESI `Slots/Slot`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SlotDef {
    pub name: String,
    pub min: u32,
    pub max: u32,
    /// Added to the PDO indices of the module in slot position n (times n).
    pub pdo_inc: u16,
    /// Added to the object indices of the module's entries in slot position n (times n).
    pub index_inc: u16,
    pub idents: Vec<u32>,
    pub default_ident: Option<u32>,
    pub classes: Vec<String>,
}

/// A plug-in module (ESI `Modules/Module`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModuleDef {
    pub ident: u32,
    pub type_name: String,
    pub name: String,
    pub class: String,
    pub rx_pdos: Vec<Pdo>,
    pub tx_pdos: Vec<Pdo>,
    pub init_cmds: Vec<CoeInitCmd>,
}

impl SlotDef {
    pub fn accepts(&self, m: &ModuleDef) -> bool {
        self.idents.contains(&m.ident) || (!m.class.is_empty() && self.classes.iter().any(|c| *c == m.class))
    }
}

/// Object indices of the device profile area are shifted per slot; others are global.
pub fn slot_shifts_index(index: u16) -> bool {
    (0x6000..=0xAFFF).contains(&index)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EsiDevice {
    pub vendor_id: u32,
    pub vendor_name: String,
    pub type_name: String,
    pub name: String,
    pub product_code: u32,
    pub revision: u32,
    pub group_type: String,
    pub group_name: String,
    pub physics: String,
    pub fmmus: Vec<String>,
    pub sms: Vec<SmDef>,
    pub rx_pdos: Vec<Pdo>,
    pub tx_pdos: Vec<Pdo>,
    pub mailbox: Option<MailboxDef>,
    pub dc_opmodes: Vec<DcOpMode>,
    pub eeprom: Option<EepromDef>,
    pub profile_no: Option<u32>,
    pub source_file: String,
    #[serde(default)]
    pub slots: Vec<SlotDef>,
    /// Modules that fit into this device's slots.
    #[serde(default)]
    pub modules: Vec<ModuleDef>,
    /// CoE object dictionary (`Profile/Dictionary`), when the ESI provides one.
    #[serde(default)]
    pub dictionary: Vec<crate::od::OdObject>,
    /// Predefined PDO assignments (`AlternativeSmMapping`).
    #[serde(default)]
    pub sm_mappings: Vec<SmMapping>,
}

/// A predefined PDO assignment: which PDOs go into which sync manager.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SmMapping {
    pub name: String,
    pub default: bool,
    pub sms: Vec<(u8, Vec<u16>)>,
}

fn parse_sm_mappings(d: roxmltree::Node) -> Vec<SmMapping> {
    children(d, "AlternativeSmMapping")
        .map(|m| SmMapping {
            name: {
                let n = name_of(m);
                if n.is_empty() {
                    "Predefined assignment".into()
                } else {
                    n
                }
            },
            default: m.attribute("Default").map(parse_bool).unwrap_or(false),
            sms: children(m, "Sm")
                .map(|sm| {
                    (
                        sm.attribute("No").and_then(parse_num).unwrap_or(0) as u8,
                        children(sm, "Pdo").map(|p| parse_u32(p.text().unwrap_or("")) as u16).collect(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// `Profile/Dictionary`, or a dictionary in the file named by `Profile/DictionaryFile`
/// (looked up next to the ESI file).
fn device_dictionary(d: roxmltree::Node, source: &str) -> Vec<crate::od::OdObject> {
    let prof = match child(d, "Profile") {
        Some(p) => p,
        None => return vec![],
    };
    if let Some(dict) = child(prof, "Dictionary") {
        return crate::od::parse_dictionary(dict);
    }
    let file = text_of(child(prof, "DictionaryFile"));
    if file.is_empty() {
        return vec![];
    }
    let base = std::path::Path::new(source).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let path = base.join(file.replace('\\', "/"));
    let text = match std::fs::read(&path) {
        Ok(b) => decode_text(&b),
        Err(_) => return vec![],
    };
    let doc = match roxmltree::Document::parse_with_options(
        &text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    ) {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Dictionary")
        .map(crate::od::parse_dictionary)
        .unwrap_or_default()
}

impl EsiDevice {
    pub fn key(&self) -> (u32, u32, u32) {
        (self.vendor_id, self.product_code, self.revision)
    }
    pub fn coe(&self) -> Option<&CoeDef> {
        self.mailbox.as_ref().and_then(|m| m.coe.as_ref())
    }
    pub fn port_count(&self) -> usize {
        self.physics.chars().filter(|c| *c != ' ').count().max(2)
    }
}

#[derive(Default)]
pub struct EsiLibrary {
    pub files: Vec<String>,
    pub devices: Vec<EsiDevice>,
}

impl EsiLibrary {
    /// Parse ESI XML text and add its devices. Returns number of devices added.
    pub fn add_xml(&mut self, xml: &str, source: &str) -> Result<usize, String> {
        let devs = parse_esi(xml, source)?;
        let n = devs.len();
        for d in devs {
            if let Some(existing) = self.devices.iter_mut().find(|e| e.key() == d.key()) {
                *existing = d;
            } else {
                self.devices.push(d);
            }
        }
        if !self.files.iter().any(|f| f == source) {
            self.files.push(source.to_string());
        }
        Ok(n)
    }

    pub fn add_file(&mut self, path: &std::path::Path) -> Result<usize, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let text = decode_text(&bytes);
        self.add_xml(&text, &path.display().to_string())
    }

    pub fn find(&self, vendor: u32, product: u32, revision: u32) -> Option<&EsiDevice> {
        self.devices
            .iter()
            .find(|d| d.vendor_id == vendor && d.product_code == product && d.revision == revision)
    }

    /// Best match: exact revision, otherwise the highest revision of the same product.
    pub fn find_best(&self, vendor: u32, product: u32, revision: u32) -> Option<&EsiDevice> {
        self.find(vendor, product, revision).or_else(|| {
            self.devices
                .iter()
                .filter(|d| d.vendor_id == vendor && d.product_code == product)
                .max_by_key(|d| d.revision)
        })
    }
}

/// ESI files are often ISO-8859-1; fall back to latin-1 when not UTF-8.
pub fn decode_text(bytes: &[u8]) -> String {
    // UTF-16 files (with byte order mark), as written by some Windows tools
    let utf16 = |b: &[u8], le: bool| -> String {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE][..]) {
        return utf16(b, true).replacen("encoding=\"UTF-16\"", "", 1).replacen("encoding=\"utf-16\"", "", 1);
    }
    if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF][..]) {
        return utf16(b, false).replacen("encoding=\"UTF-16\"", "", 1).replacen("encoding=\"utf-16\"", "", 1);
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> + 'a
where
    'i: 'a,
{
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: Option<roxmltree::Node>) -> String {
    n.and_then(|n| n.text()).unwrap_or("").trim().to_string()
}

/// First `<Name>` preferring English (LcId 1033) when several are given.
fn name_of(n: roxmltree::Node) -> String {
    let mut first = None;
    for c in children(n, "Name") {
        let t = c.text().unwrap_or("").trim().to_string();
        if c.attribute("LcId") == Some("1033") {
            return t;
        }
        if first.is_none() {
            first = Some(t);
        }
    }
    first.unwrap_or_default()
}

fn parse_coe_init(ic: roxmltree::Node) -> CoeInitCmd {
    CoeInitCmd {
        transition: children(ic, "Transition")
            .map(|t| t.text().unwrap_or("").trim().to_string())
            .next()
            .unwrap_or_else(|| "PS".into()),
        index: parse_u32(&text_of(child(ic, "Index"))) as u16,
        subindex: parse_u32(&text_of(child(ic, "SubIndex"))) as u8,
        data: hex_to_bytes(&text_of(child(ic, "Data"))),
        comment: text_of(child(ic, "Comment")),
        complete_access: ic.attribute("CompleteAccess").map(parse_bool).unwrap_or(false),
        fixed: true,
        enabled: true,
        generated: false,
        timeout_ms: 3000,
    }
}

fn parse_slots(d: roxmltree::Node) -> Vec<SlotDef> {
    let sl = match child(d, "Slots") {
        Some(s) => s,
        None => return vec![],
    };
    let num_attr = |n: roxmltree::Node, a: &str| n.attribute(a).and_then(parse_num);
    let g_pdo = num_attr(sl, "SlotPdoIncrement").unwrap_or(0);
    let g_idx = num_attr(sl, "SlotIndexIncrement").unwrap_or(0);
    children(sl, "Slot")
        .map(|s| {
            let min_a = num_attr(s, "MinInstances");
            let max_a = num_attr(s, "MaxInstances");
            let (min, max) = match (min_a, max_a) {
                (None, None) => (1, 1),
                (Some(a), None) => (a as u32, (a as u32).max(1)),
                (None, Some(b)) => (0, b as u32),
                (Some(a), Some(b)) => (a as u32, (b as u32).max(a as u32)),
            };
            let mut name = name_of(s);
            if name.is_empty() {
                name = "Slot".into();
            }
            SlotDef {
                name,
                min,
                max,
                pdo_inc: num_attr(s, "SlotPdoIncrement").unwrap_or(g_pdo) as u16,
                index_inc: num_attr(s, "SlotIndexIncrement").unwrap_or(g_idx) as u16,
                idents: children(s, "ModuleIdent").map(|m| parse_u32(m.text().unwrap_or(""))).collect(),
                default_ident: children(s, "ModuleIdent")
                    .find(|m| m.attribute("Default").map(parse_bool).unwrap_or(false))
                    .map(|m| parse_u32(m.text().unwrap_or(""))),
                classes: children(s, "ModuleClass").map(|m| m.text().unwrap_or("").trim().to_string()).collect(),
            }
        })
        .collect()
}

fn parse_modules(desc: roxmltree::Node) -> Vec<ModuleDef> {
    let ms = match child(desc, "Modules") {
        Some(m) => m,
        None => return vec![],
    };
    children(ms, "Module")
        .filter_map(|m| {
            let ty = child(m, "Type")?;
            let type_name = ty.text().unwrap_or("").trim().to_string();
            let mut name = name_of(m);
            if name.is_empty() {
                name = type_name.clone();
            }
            Some(ModuleDef {
                ident: ty.attribute("ModuleIdent").map(parse_u32).unwrap_or(0),
                type_name,
                name,
                class: ty
                    .attribute("ModuleClass")
                    .map(|c| c.to_string())
                    .or_else(|| child(m, "ModuleClass").map(|c| c.text().unwrap_or("").trim().to_string()))
                    .unwrap_or_default(),
                rx_pdos: children(m, "RxPdo").map(parse_pdo).collect(),
                tx_pdos: children(m, "TxPdo").map(parse_pdo).collect(),
                init_cmds: child(m, "Mailbox")
                    .and_then(|mb| child(mb, "CoE"))
                    .map(|c| children(c, "InitCmd").map(parse_coe_init).collect())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_pdo(n: roxmltree::Node) -> Pdo {
    let entries = children(n, "Entry")
        .map(|e| PdoEntry {
            index: parse_u32(&text_of(child(e, "Index"))) as u16,
            subindex: parse_u32(&text_of(child(e, "SubIndex"))) as u8,
            bitlen: parse_u32(&text_of(child(e, "BitLen"))) as u16,
            name: name_of(e),
            data_type: text_of(child(e, "DataType")),
        })
        .collect();
    Pdo {
        index: parse_u32(&text_of(child(n, "Index"))) as u16,
        name: name_of(n),
        sm: n.attribute("Sm").and_then(parse_num).map(|v| v as u8),
        fixed: n.attribute("Fixed").map(parse_bool).unwrap_or(false),
        mandatory: n.attribute("Mandatory").map(parse_bool).unwrap_or(false),
        excludes: children(n, "Exclude").map(|e| parse_u32(e.text().unwrap_or("")) as u16).collect(),
        entries,
    }
}

pub fn parse_esi(xml: &str, source: &str) -> Result<Vec<EsiDevice>, String> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("XML error: {}", e))?;
    let root = doc.root_element();
    if root.tag_name().name() != "EtherCATInfo" {
        return Err(format!("not an ESI file (root element is <{}>)", root.tag_name().name()));
    }
    let vendor = child(root, "Vendor").ok_or("missing <Vendor>")?;
    let vendor_id = parse_u32(&text_of(child(vendor, "Id")));
    let vendor_name = name_of(vendor);
    let desc = child(root, "Descriptions").ok_or("missing <Descriptions>")?;
    let mut groups: Vec<(String, String)> = Vec::new();
    if let Some(g) = child(desc, "Groups") {
        for grp in children(g, "Group") {
            groups.push((text_of(child(grp, "Type")), name_of(grp)));
        }
    }
    let devices_node = child(desc, "Devices").ok_or("missing <Devices>")?;
    let file_modules = parse_modules(desc);
    let mut out = Vec::new();
    for d in children(devices_node, "Device") {
        let ty = match child(d, "Type") {
            Some(t) => t,
            None => continue,
        };
        let group_type = text_of(child(d, "GroupType"));
        let group_name = groups.iter().find(|g| g.0 == group_type).map(|g| g.1.clone()).unwrap_or_else(|| {
            if group_type.is_empty() {
                "Other".into()
            } else {
                group_type.clone()
            }
        });
        let sms = children(d, "Sm")
            .map(|s| SmDef {
                kind: SmKind::from_text(s.text().unwrap_or("")),
                start: s.attribute("StartAddress").map(parse_u32).unwrap_or(0) as u16,
                control: s.attribute("ControlByte").map(parse_u32).unwrap_or(0) as u8,
                default_size: s.attribute("DefaultSize").map(parse_u32).unwrap_or(0) as u16,
                min_size: s.attribute("MinSize").map(|v| parse_u32(v) as u16),
                max_size: s.attribute("MaxSize").map(|v| parse_u32(v) as u16),
                enable: s.attribute("Enable").map(parse_bool).unwrap_or(true),
            })
            .collect();
        let mailbox = child(d, "Mailbox").map(|m| {
            let coe = child(m, "CoE").map(|c| CoeDef {
                sdo_info: c.attribute("SdoInfo").map(parse_bool).unwrap_or(false),
                pdo_assign: c.attribute("PdoAssign").map(parse_bool).unwrap_or(false),
                pdo_config: c.attribute("PdoConfig").map(parse_bool).unwrap_or(false),
                complete_access: c.attribute("CompleteAccess").map(parse_bool).unwrap_or(false),
                segmented_sdo: c.attribute("SegmentedSdo").map(parse_bool).unwrap_or(false),
                init_cmds: children(c, "InitCmd").map(parse_coe_init).collect(),
            });
            MailboxDef {
                coe,
                foe: child(m, "FoE").is_some(),
                eoe: child(m, "EoE").is_some(),
                soe: child(m, "SoE").is_some(),
                voe: child(m, "VoE").is_some(),
                aoe: child(m, "AoE").is_some(),
                data_link_layer: m.attribute("DataLinkLayer").map(parse_bool).unwrap_or(false),
            }
        });
        let dc_opmodes = child(d, "Dc")
            .map(|dc| {
                children(dc, "OpMode")
                    .map(|o| {
                        let c0 = child(o, "CycleTimeSync0");
                        let c1 = child(o, "CycleTimeSync1");
                        DcOpMode {
                            name: text_of(child(o, "Name")),
                            desc: text_of(child(o, "Desc")),
                            assign_activate: parse_u32(&text_of(child(o, "AssignActivate"))) as u16,
                            cycle_sync0_factor: c0
                                .and_then(|c| c.attribute("Factor"))
                                .and_then(parse_num)
                                .unwrap_or(if c0.is_some() { 1 } else { 0 }) as i32,
                            shift_sync0: parse_num(&text_of(child(o, "ShiftTimeSync0"))).unwrap_or(0) as i32,
                            cycle_sync1_factor: c1.and_then(|c| c.attribute("Factor")).and_then(parse_num).unwrap_or(0) as i32,
                            shift_sync1: parse_num(&text_of(child(o, "ShiftTimeSync1"))).unwrap_or(0) as i32,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let eeprom = child(d, "Eeprom").map(|e| EepromDef {
            byte_size: parse_u32(&text_of(child(e, "ByteSize"))),
            config_data: hex_to_bytes(&text_of(child(e, "ConfigData"))),
            boot_strap: hex_to_bytes(&text_of(child(e, "BootStrap"))),
        });
        let profile_no = child(d, "Profile")
            .and_then(|p| child(p, "ProfileNo").or_else(|| child(p, "ChannelInfo").and_then(|c| child(c, "ProfileNo"))))
            .and_then(|n| parse_num(n.text().unwrap_or("")))
            .map(|v| v as u32);
        let type_name = ty.text().unwrap_or("").trim().to_string();
        let mut name = name_of(d);
        if name.is_empty() {
            name = type_name.clone();
        }
        out.push(EsiDevice {
            vendor_id,
            vendor_name: vendor_name.clone(),
            type_name,
            name,
            product_code: ty.attribute("ProductCode").map(parse_u32).unwrap_or(0),
            revision: ty.attribute("RevisionNo").map(parse_u32).unwrap_or(0),
            group_type,
            group_name,
            physics: d.attribute("Physics").unwrap_or("YY").to_string(),
            fmmus: children(d, "Fmmu").map(|f| f.text().unwrap_or("").trim().to_string()).collect(),
            sms,
            rx_pdos: children(d, "RxPdo").map(parse_pdo).collect(),
            tx_pdos: children(d, "TxPdo").map(parse_pdo).collect(),
            mailbox,
            dc_opmodes,
            eeprom,
            profile_no,
            source_file: source.to_string(),
            modules: {
                let slots = parse_slots(d);
                file_modules.iter().filter(|m| slots.iter().any(|s| s.accepts(m))).cloned().collect()
            },
            slots: parse_slots(d),
            dictionary: device_dictionary(d, source),
            sm_mappings: parse_sm_mappings(d),
        });
    }
    Ok(out)
}

pub const SAMPLE_IO: &str = include_str!("../samples/example_io.xml");
pub const SAMPLE_DRIVE: &str = include_str!("../samples/example_drive.xml");
pub const SAMPLE_MODULAR: &str = include_str!("../samples/example_modular.xml");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_samples() {
        let a = parse_esi(SAMPLE_IO, "io").unwrap();
        assert_eq!(a.len(), 4);
        assert_eq!(a[3].tx_pdos.len(), 8);
        let m = parse_esi(SAMPLE_MODULAR, "mod").unwrap();
        assert_eq!(m[0].slots.len(), 1);
        assert_eq!(m[0].slots[0].max, 8);
        assert_eq!(m[0].slots[0].index_inc, 0x10);
        assert_eq!(m[0].modules.len(), 3);
        assert_eq!(m[0].modules[2].init_cmds.len(), 1);
        let b = parse_esi(SAMPLE_DRIVE, "drv").unwrap();
        assert_eq!(b[0].dc_opmodes.len(), 3);
        assert!(b[0].coe().unwrap().pdo_config);
    }
}
