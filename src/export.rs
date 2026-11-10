//! Process image exports for embedded targets (C header, CSV).

use crate::model::*;

fn ident(s: &str) -> String {
    let mut o = String::new();
    let mut last_us = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            o.push(c.to_ascii_uppercase());
            last_us = false;
        } else if !last_us {
            o.push('_');
            last_us = true;
        }
    }
    o.trim_matches('_').to_string()
}

pub fn c_header(p: &Project, lay: &NetLayout) -> String {
    let mut s = String::new();
    s.push_str("/*\n * EtherCAT process image layout\n");
    s.push_str(&format!(
        " * Master: {}   cycle time: {} us   slaves: {}\n",
        p.master.name,
        p.master.cycle_time_us,
        p.slaves.len()
    ));
    s.push_str(" * Offsets are bit offsets inside the cyclic frame image\n");
    s.push_str(" * (outputs first, inputs follow the outputs).\n */\n");
    s.push_str("#ifndef ECAT_PROCESS_IMAGE_H\n#define ECAT_PROCESS_IMAGE_H\n\n");
    s.push_str(&format!("#define ECAT_CYCLE_TIME_US      {}u\n", p.master.cycle_time_us));
    s.push_str(&format!("#define ECAT_SLAVE_COUNT        {}u\n", p.slaves.len()));
    s.push_str(&format!("#define ECAT_LOGICAL_START      0x{:08X}u\n", lay.out_log));
    s.push_str(&format!("#define ECAT_OUTPUT_BYTES       {}u\n", lay.out_size));
    s.push_str(&format!("#define ECAT_INPUT_OFFSET       {}u\n", lay.out_size));
    s.push_str(&format!("#define ECAT_INPUT_BYTES        {}u\n", lay.in_size));
    let frames = cyclic_frames(p, lay);
    if p.task_count() > 1 {
        for t in 0..p.task_count() {
            let tl = lay.tasks.get(t).cloned().unwrap_or_default();
            s.push_str(&format!(
                "#define ECAT_TASK{}_CYCLE_US     {}u /* {}: image bytes {}..{} */\n",
                t,
                p.task_cycle_us(t),
                p.task_name(t),
                tl.start,
                tl.end()
            ));
        }
    }
    s.push_str(&format!(
        "#define ECAT_EXPECTED_WKC       {}u /* sum over all process data commands */\n",
        process_data_wkc(&frames)
    ));
    s.push_str(&format!("#define ECAT_FRAME_COUNT        {}u\n", frames.len()));
    let mut k = 0;
    for (fi, f) in frames.iter().enumerate() {
        for c in f.cmds.iter().filter(|c| c.cmd != CMD_BRD) {
            s.push_str(&format!(
                "#define ECAT_CMD{}_FRAME {}u /* {} */\n#define ECAT_CMD{}_ADDR  0x{:08X}u\n#define ECAT_CMD{}_LEN   {}u\n#define ECAT_CMD{}_WKC   {}u\n",
                k,
                fi,
                cmd_name(c.cmd),
                k,
                c.addr,
                k,
                c.len,
                k,
                c.wkc
            ));
            k += 1;
        }
    }
    s.push('\n');
    for (dir, inp) in [("Outputs", false), ("Inputs", true)] {
        s.push_str(&format!("/* ---- {} ---- */\n", dir));
        for v in lay.vars.iter().filter(|v| v.is_input == inp) {
            let id = format!("ECAT_{}_{}", if inp { "I" } else { "O" }, ident(&v.name));
            s.push_str(&format!(
                "#define {}_BITOFFS {}u /* byte {} bit {}, {} */\n",
                id,
                v.bit_offs,
                v.bit_offs / 8,
                v.bit_offs % 8,
                v.data_type
            ));
            s.push_str(&format!("#define {}_BITSIZE {}u\n", id, v.bit_size));
        }
        s.push('\n');
    }
    s.push_str("#endif /* ECAT_PROCESS_IMAGE_H */\n");
    s
}

pub fn csv(lay: &NetLayout) -> String {
    let mut s = String::from("Direction;Name;DataType;BitSize;BitOffset;Byte;Bit;PDO;Index;SubIndex\n");
    for v in lay.vars.iter() {
        s.push_str(&format!(
            "{};{};{};{};{};{};{};#x{:04X};#x{:04X};{}\n",
            if v.is_input { "Input" } else { "Output" },
            v.name,
            v.data_type,
            v.bit_size,
            v.bit_offs,
            v.bit_offs / 8,
            v.bit_offs % 8,
            v.pdo,
            v.index,
            v.subindex
        ));
    }
    s
}
