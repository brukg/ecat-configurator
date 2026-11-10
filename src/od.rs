//! CoE object dictionary from the ESI (`Profile/Dictionary`): parsing, lookup and value encoding.

use crate::util::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One addressable entry (index:subindex) of the object dictionary.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OdEntry {
    pub subindex: u8,
    pub name: String,
    pub data_type: String,
    pub bit_size: u16,
    /// "ro", "rw", "wo" (as written in the ESI; empty when not given).
    pub access: String,
    /// "R" (RxPDO), "T" (TxPDO), "RT" or empty.
    pub pdo_mapping: String,
    pub default: Vec<u8>,
}

impl OdEntry {
    pub fn writable(&self) -> bool {
        self.access.is_empty() || self.access.contains('w')
    }
    pub fn mappable(&self, rx: bool) -> bool {
        let m = self.pdo_mapping.to_ascii_uppercase();
        if rx {
            m.contains('R')
        } else {
            m.contains('T')
        }
    }
}

/// An object; simple objects have one entry with subindex 0.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OdObject {
    pub index: u16,
    pub name: String,
    pub data_type: String,
    pub entries: Vec<OdEntry>,
}

impl OdObject {
    pub fn is_simple(&self) -> bool {
        self.entries.len() == 1 && self.entries[0].subindex == 0 && self.entries[0].name.is_empty()
    }
    pub fn entry(&self, sub: u8) -> Option<&OdEntry> {
        self.entries.iter().find(|e| e.subindex == sub)
    }
    /// "Name" for simple objects, "Object: Entry" for records and arrays.
    pub fn entry_name(&self, e: &OdEntry) -> String {
        if e.name.is_empty() {
            self.name.clone()
        } else {
            format!("{}: {}", self.name, e.name)
        }
    }
}

pub fn lookup(dict: &[OdObject], index: u16, sub: u8) -> Option<(&OdObject, &OdEntry)> {
    let o = dict.iter().find(|o| o.index == index)?;
    Some((o, o.entry(sub)?))
}

struct SubDef {
    subidx: Option<u8>,
    name: String,
    ty: String,
    bits: u16,
    access: String,
    pdo: String,
}

struct TypeDef {
    bits: u32,
    base: String,
    subs: Vec<SubDef>,
    array: Option<(u32, u32)>,
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}
fn kids<'a, 'i: 'a>(n: roxmltree::Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}
fn txt(n: Option<roxmltree::Node>) -> String {
    n.and_then(|n| n.text()).unwrap_or("").trim().to_string()
}
fn first_name(n: roxmltree::Node) -> String {
    kids(n, "Name")
        .find(|c| c.attribute("LcId") == Some("1033"))
        .or_else(|| kids(n, "Name").next())
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_string()
}
fn flags(n: roxmltree::Node) -> (String, String) {
    match child(n, "Flags") {
        Some(f) => (txt(child(f, "Access")).to_ascii_lowercase(), txt(child(f, "PdoMapping")).to_ascii_uppercase()),
        None => (String::new(), String::new()),
    }
}
fn default_of(info: Option<roxmltree::Node>) -> Vec<u8> {
    let info = match info {
        Some(i) => i,
        None => return vec![],
    };
    let d = txt(child(info, "DefaultData"));
    if !d.is_empty() {
        return hex_to_bytes(&d);
    }
    vec![]
}

fn type_bits(types: &HashMap<String, TypeDef>, ty: &str) -> u16 {
    types.get(ty).map(|t| t.bits as u16).or_else(|| datatype_bits(ty)).unwrap_or(0)
}

/// Read `Dictionary` (DataTypes + Objects) into a flat list of objects with their entries.
pub fn parse_dictionary(dict: roxmltree::Node) -> Vec<OdObject> {
    let mut types: HashMap<String, TypeDef> = HashMap::new();
    if let Some(dts) = child(dict, "DataTypes") {
        for dt in kids(dts, "DataType") {
            let name = txt(child(dt, "Name"));
            let subs = kids(dt, "SubItem")
                .map(|si| {
                    let (access, pdo) = flags(si);
                    SubDef {
                        subidx: parse_num(&txt(child(si, "SubIdx"))).map(|v| v as u8),
                        name: txt(child(si, "Name")),
                        ty: txt(child(si, "Type")),
                        bits: parse_num(&txt(child(si, "BitSize"))).unwrap_or(0) as u16,
                        access,
                        pdo,
                    }
                })
                .collect();
            let array = child(dt, "ArrayInfo").map(|a| {
                (
                    parse_num(&txt(child(a, "LBound"))).unwrap_or(0) as u32,
                    parse_num(&txt(child(a, "Elements"))).unwrap_or(0) as u32,
                )
            });
            types.insert(
                name,
                TypeDef {
                    bits: parse_num(&txt(child(dt, "BitSize"))).unwrap_or(0) as u32,
                    base: txt(child(dt, "BaseType")),
                    subs,
                    array,
                },
            );
        }
    }
    let mut out = Vec::new();
    let objs = match child(dict, "Objects") {
        Some(o) => o,
        None => return out,
    };
    for o in kids(objs, "Object") {
        let index = parse_u32(&txt(child(o, "Index"))) as u16;
        let name = first_name(o);
        let ty = txt(child(o, "Type"));
        let (o_access, o_pdo) = flags(o);
        let info = child(o, "Info");
        // per-subindex defaults and names given in the object's Info
        let info_subs: Vec<(String, Vec<u8>)> = info
            .map(|i| kids(i, "SubItem").map(|s| (first_name(s), default_of(child(s, "Info")))).collect())
            .unwrap_or_default();
        let mut entries = Vec::new();
        match types.get(&ty) {
            Some(td) if !td.subs.is_empty() => {
                let mut k: u32 = 0;
                for sd in td.subs.iter() {
                    let elem_array = types.get(&sd.ty).and_then(|t| t.array.map(|a| (a, t)));
                    match elem_array {
                        Some(((lb, n), at)) if n > 0 => {
                            let first = sd.subidx.map(|v| v as u32).unwrap_or(lb.max(1));
                            let elem_bits = if !at.base.is_empty() {
                                type_bits(&types, &at.base)
                            } else {
                                (sd.bits as u32 / n) as u16
                            };
                            for e in 0..n {
                                let sub = (first + e) as u8;
                                entries.push(OdEntry {
                                    subindex: sub,
                                    name: format!("{} {}", if sd.name.is_empty() { "Element" } else { &sd.name }, e + 1),
                                    data_type: at.base.clone(),
                                    bit_size: elem_bits,
                                    access: if sd.access.is_empty() { o_access.clone() } else { sd.access.clone() },
                                    pdo_mapping: if sd.pdo.is_empty() { o_pdo.clone() } else { sd.pdo.clone() },
                                    default: vec![],
                                });
                            }
                            k = first + n;
                        }
                        _ => {
                            let sub = sd.subidx.map(|v| v as u32).unwrap_or(k);
                            entries.push(OdEntry {
                                subindex: sub as u8,
                                name: sd.name.clone(),
                                data_type: sd.ty.clone(),
                                bit_size: if sd.bits > 0 { sd.bits } else { type_bits(&types, &sd.ty) },
                                access: if sd.access.is_empty() { o_access.clone() } else { sd.access.clone() },
                                pdo_mapping: if sd.pdo.is_empty() { o_pdo.clone() } else { sd.pdo.clone() },
                                default: vec![],
                            });
                            k = sub + 1;
                        }
                    }
                }
                // defaults from Info/SubItem, matched by name first, then by position
                for (pos, e) in entries.iter_mut().enumerate() {
                    let by_name = info_subs.iter().find(|(n, _)| !n.is_empty() && *n == e.name);
                    let d = by_name.or_else(|| info_subs.get(pos)).map(|x| x.1.clone()).unwrap_or_default();
                    e.default = d;
                }
            }
            _ => {
                let bits = parse_num(&txt(child(o, "BitSize"))).map(|v| v as u16).unwrap_or_else(|| type_bits(&types, &ty));
                entries.push(OdEntry {
                    subindex: 0,
                    name: String::new(),
                    data_type: types
                        .get(&ty)
                        .filter(|t| !t.base.is_empty())
                        .map(|t| t.base.clone())
                        .unwrap_or_else(|| ty.clone()),
                    bit_size: bits,
                    access: o_access.clone(),
                    pdo_mapping: o_pdo.clone(),
                    default: default_of(info),
                });
            }
        }
        out.push(OdObject {
            index,
            name,
            data_type: ty,
            entries,
        });
    }
    out.sort_by_key(|o| o.index);
    out
}

fn base(dt: &str) -> String {
    dt.to_ascii_uppercase()
}

/// Turn a value typed by the user into the little-endian bytes of the entry's type.
pub fn encode_value(text: &str, data_type: &str, bits: u16) -> Result<Vec<u8>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("enter a value".into());
    }
    let dt = base(data_type);
    if dt.starts_with("STRING") || dt.contains("VISIBLE_STRING") {
        return Ok(t.as_bytes().to_vec());
    }
    if dt.contains("OCTET_STRING") || dt.starts_with("ARRAY") {
        let b = hex_to_bytes(t);
        return if b.is_empty() { Err("enter hex bytes".into()) } else { Ok(b) };
    }
    let bytes = ((bits as usize) + 7) / 8;
    let bytes = if bytes == 0 { 1 } else { bytes.min(8) };
    match dt.as_str() {
        "REAL" | "REAL32" => t.parse::<f32>().map(|v| v.to_le_bytes().to_vec()).map_err(|_| "not a number".into()),
        "LREAL" | "REAL64" => t.parse::<f64>().map(|v| v.to_le_bytes().to_vec()).map_err(|_| "not a number".into()),
        "BOOL" | "BIT" => match t.to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Ok(vec![1]),
            "0" | "false" | "off" | "no" => Ok(vec![0]),
            _ => Err("enter 0 or 1".into()),
        },
        _ => {
            let v = parse_num(t).ok_or("not a whole number (use 123, -5 or 0x7B)")?;
            let signed = dt.starts_with("SINT") || dt.starts_with("INT") || dt.starts_with("DINT") || dt.starts_with("LINT");
            let width = bytes as u32 * 8;
            if width < 64 {
                let (lo, hi) = if signed {
                    (-(1i64 << (width - 1)), (1i64 << (width - 1)) - 1)
                } else {
                    (0, (1i64 << width) - 1)
                };
                if v < lo || v > hi {
                    return Err(format!("out of range {}..{}", lo, hi));
                }
            }
            Ok(v.to_le_bytes()[..bytes].to_vec())
        }
    }
}

/// Show bytes as a value of the entry's type ("1000", "-5", "1.5", "text"), hex when unknown.
pub fn decode_value(b: &[u8], data_type: &str) -> String {
    if b.is_empty() {
        return String::new();
    }
    let dt = base(data_type);
    if dt.starts_with("STRING") || dt.contains("VISIBLE_STRING") {
        return format!("\"{}\"", String::from_utf8_lossy(b).trim_end_matches('\0'));
    }
    let mut a = [0u8; 8];
    let n = b.len().min(8);
    a[..n].copy_from_slice(&b[..n]);
    let u = u64::from_le_bytes(a);
    match dt.as_str() {
        "REAL" | "REAL32" if b.len() == 4 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string(),
        "LREAL" | "REAL64" if b.len() == 8 => f64::from_le_bytes(a).to_string(),
        "BOOL" | "BIT" => (u != 0).to_string(),
        "SINT" | "INT" | "DINT" | "LINT" | "INTEGER8" | "INTEGER16" | "INTEGER32" | "INTEGER64" => {
            let sh = 64 - 8 * n as u32;
            (((u << sh) as i64) >> sh).to_string()
        }
        _ if datatype_bits(&dt).is_some() => u.to_string(),
        _ => bytes_to_hex_spaced(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esi::*;

    #[test]
    fn dictionary_and_values() {
        let d = parse_esi(SAMPLE_DRIVE, "drv").unwrap().remove(0);
        let (o, e) = lookup(&d.dictionary, 0x6060, 0).unwrap();
        assert_eq!(o.name, "Modes of operation");
        assert_eq!(e.bit_size, 8);
        assert!(e.writable());
        let (o, e) = lookup(&d.dictionary, 0x60C2, 1).unwrap();
        assert_eq!(o.entry_name(e), "Interpolation time period: Time period value");
        assert_eq!(e.data_type, "USINT");
        let (_, e) = lookup(&d.dictionary, 0x1C12, 2).unwrap();
        assert_eq!(e.data_type, "UINT");
        assert!(!lookup(&d.dictionary, 0x6041, 0).unwrap().1.writable());
        assert_eq!(encode_value("1000", "UDINT", 32).unwrap(), vec![0xE8, 3, 0, 0]);
        assert_eq!(encode_value("-2", "SINT", 8).unwrap(), vec![0xFE]);
        assert!(encode_value("300", "USINT", 8).is_err());
        assert_eq!(decode_value(&[0xFE], "SINT"), "-2");
        assert_eq!(decode_value(&[0xE8, 3, 0, 0], "UDINT"), "1000");
    }
}
