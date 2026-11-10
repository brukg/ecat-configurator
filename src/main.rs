mod eni;
mod esi;
mod export;
mod model;
mod offscreen;
mod scan;
mod sii;
mod ui;
mod util;

use std::path::PathBuf;

fn usage() {
    println!(
        "EtherCAT Configurator {}\n\n\
         Usage: ecat-configurator [options]\n\
           --esi <file|folder>        load ESI files into the catalog (repeatable)\n\
           --project <file.ecproj>    open a configuration project\n\
           --eni <file.xml>           import an ENI file\n\
           --export-eni <out.xml>     write the ENI and exit (no window)\n\
           --export-header <out.h>    write the process image C header and exit\n\
           --screenshot <file.png>    draw about 120 frames off-screen, save the picture and exit\n\
           <files…>                   ENI, ESI or project files to open (each ENI/project as its own document)\n\
           --help                     this text",
        env!("CARGO_PKG_VERSION")
    );
}

/// Write the reason of an unexpected stop to a log file so it can be reported.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        if let Some(b) = base {
            let dir = b.join("ecat-configurator");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), format!("{}\n", info));
        }
        default(info);
    }));
}

fn main() {
    install_crash_log();
    let mut args = std::env::args().skip(1);
    let mut opts = ui::StartOptions {
        screenshot: None,
        esi: vec![],
        project: None,
        eni: None,
        files: vec![],
    };
    let mut export_eni: Option<PathBuf> = None;
    let mut export_h: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        let mut val = || args.next().map(PathBuf::from);
        match a.as_str() {
            "--screenshot" => opts.screenshot = val(),
            "--esi" => {
                if let Some(v) = val() {
                    opts.esi.push(v)
                }
            }
            "--project" => opts.project = val(),
            "--eni" => opts.eni = val(),
            "--export-eni" => export_eni = val(),
            "--export-header" => export_h = val(),
            "-h" | "--help" => {
                usage();
                return;
            }
            other if !other.starts_with("--") => opts.files.push(PathBuf::from(other)),
            other => {
                eprintln!("unknown option {}", other);
                usage();
                std::process::exit(2);
            }
        }
    }

    if export_eni.is_some() || export_h.is_some() {
        std::process::exit(headless(&opts, export_eni, export_h));
    }

    if let Some(path) = opts.screenshot.clone() {
        // picture mode: draw off-screen in memory, no window is ever opened
        match offscreen::screenshot(opts, &path, 1440, 880, 120) {
            Ok(_) => {
                println!("screenshot saved to {}", path.display());
                return;
            }
            Err(e) => {
                eprintln!("screenshot failed: {}", e);
                std::process::exit(1);
            }
        }
    }
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 880.0])
        .with_min_inner_size([900.0, 560.0])
        .with_title("EtherCAT Configurator — ESI / ENI")
        .with_app_id("ecat-configurator");
    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "EtherCAT Configurator",
        native,
        Box::new(move |cc| Ok(Box::new(ui::App::new(&cc.egui_ctx, opts)))),
    ) {
        eprintln!("cannot start window: {}", e);
        std::process::exit(1);
    }
}

/// Command line use for build pipelines and embedded targets.
fn headless(opts: &ui::StartOptions, eni_out: Option<PathBuf>, h_out: Option<PathBuf>) -> i32 {
    let mut lib = esi::EsiLibrary::default();
    let _ = lib.add_xml(esi::SAMPLE_IO, "built-in");
    let _ = lib.add_xml(esi::SAMPLE_DRIVE, "built-in");
    for p in opts.esi.iter() {
        let files: Vec<PathBuf> = if p.is_dir() {
            std::fs::read_dir(p)
                .map(|r| {
                    r.flatten()
                        .map(|e| e.path())
                        .filter(|x| x.extension().map(|e| e.eq_ignore_ascii_case("xml")).unwrap_or(false))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![p.clone()]
        };
        for f in files {
            if let Err(e) = lib.add_file(&f) {
                eprintln!("ESI {}: {}", f.display(), e);
            }
        }
    }
    let project = if let Some(p) = &opts.project {
        match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<model::Project>(&s).map_err(|e| e.to_string()))
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cannot read project: {}", e);
                return 1;
            }
        }
    } else if let Some(p) = &opts.eni {
        match std::fs::read(p)
            .map_err(|e| e.to_string())
            .and_then(|b| eni::import_eni(&esi::decode_text(&b), &lib))
        {
            Ok((p, notes)) => {
                for n in notes {
                    eprintln!("note: {}", n);
                }
                p
            }
            Err(e) => {
                eprintln!("cannot read ENI: {}", e);
                return 1;
            }
        }
    } else {
        model::demo_project(&lib)
    };
    let lay = model::compute_layout(&project);
    for i in model::validate(&project, &lay) {
        eprintln!("{:?}: {}", i.sev, i.text);
    }
    if let Some(o) = eni_out {
        if let Err(e) = std::fs::write(&o, eni::export_eni(&project)) {
            eprintln!("cannot write {}: {}", o.display(), e);
            return 1;
        }
        println!("ENI written to {}", o.display());
    }
    if let Some(o) = h_out {
        if let Err(e) = std::fs::write(&o, export::c_header(&project, &lay)) {
            eprintln!("cannot write {}: {}", o.display(), e);
            return 1;
        }
        println!("C header written to {}", o.display());
    }
    0
}
