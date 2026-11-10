//! Desktop user interface.

use crate::eni;
use crate::esi::*;
use crate::export;
use crate::model::*;
use crate::od;
use crate::scan::{self, ScanMsg, ScannedSlave};
use crate::sii;
use crate::util::*;
use eframe::egui::{self, Color32, RichText, Stroke};
use std::path::PathBuf;

const ACCENT: Color32 = Color32::from_rgb(0, 90, 160);
const OK_GREEN: Color32 = Color32::from_rgb(20, 130, 60);
const WARN: Color32 = Color32::from_rgb(190, 120, 0);
const ERR: Color32 = Color32::from_rgb(190, 30, 30);

#[derive(Clone, Copy, PartialEq)]
pub enum Sel {
    Master,
    Slave(usize),
    Catalog(usize),
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MasterTab {
    General,
    Dc,
    Image,
    Cyclic,
    InitCmds,
    Topology,
    Online,
    Eni,
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SlaveTab {
    General,
    Modules,
    Dictionary,
    ProcessData,
    Mailbox,
    Dc,
    InitCmds,
    Eeprom,
    Esc,
}

/// A view that can live in a dock tab, a split pane, a floating panel or its own window.
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Network,
    Catalog,
    Messages,
    Validation,
    /// An independent editor view; its item, page and inner state live in `App::editors`.
    Editor {
        inst: u64,
    },
}

/// An open configuration (one ENI / project file). Documents are fully independent.
/// Undo/redo of one document. Edits less than 0.8 s apart (typing, dragging) form one step.
#[derive(Default)]
struct History {
    undo: Vec<Project>,
    redo: Vec<Project>,
    /// State after the last recorded change.
    base: Option<Project>,
    last_change: f64,
}

const UNDO_LIMIT: usize = 100;

struct Doc {
    hist: History,
    changed: bool,
    name: String,
    project: Project,
    path: Option<PathBuf>,
    dirty: bool,
    eni_text: String,
    sel: Sel,
    last_sel: Sel,
}

static DOC_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(2);
fn next_doc_id() -> u64 {
    DOC_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// State of one editor view. Every editor is independent of the others.
struct EditorState {
    /// Which open document (configuration) this editor shows.
    doc: u64,
    target: Option<Target>,
    master_page: MasterTab,
    slave_page: SlaveTab,
    sel_pdo: Option<usize>,
    sii_cache: Option<(usize, Vec<u8>)>,
    coe_form: CoeForm,
}

impl Default for EditorState {
    fn default() -> Self {
        EditorState {
            doc: 1,
            target: None,
            master_page: MasterTab::General,
            slave_page: SlaveTab::ProcessData,
            sel_pdo: None,
            sii_cache: None,
            coe_form: CoeForm {
                transition: "PS".into(),
                ..Default::default()
            },
        }
    }
}

/// Something that can be opened in its own view.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Master,
    Slave(u64),
    Device(u32, u32, u32),
}

#[derive(Clone, Copy, PartialEq)]
enum Place {
    NewTab,
    SplitRight,
    SplitBelow,
    Floating,
    Window,
}

/// Changes that alter the slave list are applied between frames, never while views draw.
enum Deferred {
    SortTopology,
    AddDevice(usize),
    ConfigFromScan,
}

static INST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn next_inst() -> u64 {
    INST.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn default_dock(first_editor: u64) -> egui_dock::DockState<Tab> {
    use egui_dock::{DockState, NodeIndex};
    let mut d = DockState::new(vec![Tab::Editor { inst: first_editor }]);
    let tree = d.main_surface_mut();
    let [center, left] = tree.split_left(NodeIndex::root(), 0.23, vec![Tab::Network]);
    let _ = tree.split_below(left, 0.46, vec![Tab::Catalog]);
    let _ = tree.split_below(center, 0.80, vec![Tab::Messages, Tab::Validation]);
    d
}

struct Viewer<'a> {
    app: &'a mut App,
    lay: NetLayout,
    issues: Vec<Issue>,
}

impl egui_dock::TabViewer for Viewer<'_> {
    type Tab = Tab;
    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        self.app.tab_title(tab, &self.issues).into()
    }
    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(tab_key(tab))
    }
    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        if self.lay.slaves.len() != self.app.project.slaves.len() {
            self.lay = compute_layout(&self.app.project);
            self.issues = validate(&self.app.project, &self.lay);
        }
        self.app.tab_ui(ui, tab, &self.lay, &self.issues);
    }
    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        if let Tab::Editor { inst } = tab {
            if response.clicked() || response.drag_started() {
                self.app.pending_active = Some(*inst);
            }
        }
    }
    fn on_close(&mut self, tab: &mut Tab) -> bool {
        if let Tab::Editor { inst } = tab {
            self.app.closed_editors.push(*inst);
        }
        true
    }
    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Tab, _s: egui_dock::SurfaceIndex, _n: egui_dock::NodeIndex) {
        if let Some(t) = self.app.tab_target(tab) {
            open_menu(ui, &mut self.app.open_requests, t);
        } else {
            ui.label(RichText::new("Drag the tab to split or float it").weak());
        }
    }
    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

fn tab_key(t: &Tab) -> (u8, u64) {
    match t {
        Tab::Network => (0, 0),
        Tab::Catalog => (1, 0),
        Tab::Messages => (2, 0),
        Tab::Validation => (3, 0),
        Tab::Editor { inst } => (4, *inst),
    }
}

/// The "open in …" entries shared by the tree, the catalog and tab context menus.
fn open_menu(ui: &mut egui::Ui, req: &mut Vec<(Option<Target>, Place)>, t: Target) {
    for (label, place) in [
        ("➕ Open in new tab", Place::NewTab),
        ("Open split right", Place::SplitRight),
        ("Open split below", Place::SplitBelow),
        ("Open as floating panel", Place::Floating),
        ("Open in new window", Place::Window),
    ] {
        if ui.button(label).clicked() {
            req.push((Some(t), place));
            ui.close_menu();
        }
    }
}

fn master_page_name(p: MasterTab) -> &'static str {
    match p {
        MasterTab::General => "General",
        MasterTab::Dc => "Distributed Clocks",
        MasterTab::Image => "Process Image",
        MasterTab::Cyclic => "Cyclic Frames",
        MasterTab::InitCmds => "Init Commands",
        MasterTab::Topology => "Topology",
        MasterTab::Online => "Online",
        MasterTab::Eni => "ENI",
    }
}

fn slave_page_name(p: SlaveTab) -> &'static str {
    match p {
        SlaveTab::General => "General",
        SlaveTab::Modules => "Modules",
        SlaveTab::Dictionary => "Dictionary",
        SlaveTab::ProcessData => "Process Data",
        SlaveTab::Mailbox => "Mailbox",
        SlaveTab::Dc => "DC",
        SlaveTab::InitCmds => "Init Commands",
        SlaveTab::Eeprom => "EEPROM",
        SlaveTab::Esc => "ESC",
    }
}

/// Entry form for SoE start-up writes.
struct SoeForm {
    transition: String,
    drive: u8,
    idn: String,
    value: String,
    /// 2, 4 or 8 bytes; 0 = hex bytes as typed
    size: u8,
    comment: String,
}

impl Default for SoeForm {
    fn default() -> Self {
        SoeForm {
            transition: "PS".into(),
            drive: 0,
            idn: String::new(),
            value: String::new(),
            size: 2,
            comment: String::new(),
        }
    }
}

#[derive(Default)]
struct CoeForm {
    /// Value as typed (decimal / text); encoded with the dictionary type when known.
    value: String,
    /// Search text of the object picker.
    pick: String,
    transition: String,
    index: String,
    sub: String,
    data: String,
    comment: String,
    ca: bool,
    editing: Option<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Settings {
    esi_paths: Vec<String>,
    last_project: Option<String>,
    #[serde(default)]
    dock: Option<egui_dock::DockState<Tab>>,
}

fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("ecat-configurator").join("settings.json"))
}

pub struct StartOptions {
    pub screenshot: Option<PathBuf>,
    pub esi: Vec<PathBuf>,
    pub project: Option<PathBuf>,
    pub eni: Option<PathBuf>,
    /// Any number of ENI / ESI / project files to open.
    pub files: Vec<PathBuf>,
}

pub struct App {
    lib: EsiLibrary,
    project: Project,
    project_path: Option<PathBuf>,
    /// Name of the active document; the active document's data lives in the fields
    /// `project`, `project_path`, `dirty`, `eni_text`, `sel`, `last_sel`.
    doc_name: String,
    /// Undo history and "changed this frame" flag of the active document.
    hist: History,
    changed: bool,
    /// Copied slaves (also put on the system clipboard as JSON).
    clip: Vec<SlaveCfg>,
    /// Time of the current frame (seconds).
    now: f64,
    /// A text field has focus or a value is being dragged (edits then merge into one undo step).
    continuous: bool,
    active_doc: u64,
    /// The other open documents.
    docs: std::collections::HashMap<u64, Doc>,
    doc_order: Vec<u64>,
    pending_doc: Option<u64>,
    confirm_close: Option<u64>,
    sel: Sel,
    master_tab: MasterTab,
    slave_tab: SlaveTab,
    dock: egui_dock::DockState<Tab>,
    windows: Vec<(u64, Tab)>,
    open_requests: Vec<(Option<Target>, Place)>,
    editors: std::collections::HashMap<u64, EditorState>,
    active_editor: u64,
    pending_active: Option<u64>,
    route_pending: bool,
    closed_editors: Vec<u64>,
    last_sel: Sel,
    deferred: Vec<Deferred>,
    focus_req: Option<Tab>,
    filter: String,
    od_filter: String,
    soe_form: SoeForm,
    /// Scroll the start-up command form into view on the next draw.
    coe_scroll: bool,
    log: Vec<(String, Severity, String)>,
    dirty: bool,
    eni_text: String,
    sii_cache: Option<(usize, Vec<u8>)>,
    sel_pdo: Option<usize>,
    coe_form: CoeForm,
    scan_rx: Option<std::sync::mpsc::Receiver<ScanMsg>>,
    scanned: Vec<ScannedSlave>,
    adapters: Vec<(String, String)>,
    show_about: bool,
    settings: Settings,
    persist: bool,
}

impl App {
    pub fn new(ctx: &egui::Context, opts: StartOptions) -> App {
        setup_style(ctx);
        let persist = opts.screenshot.is_none();
        let settings: Settings = if persist {
            settings_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Settings::default()
        };
        let mut app = App {
            lib: EsiLibrary::default(),
            project: Project::default(),
            project_path: None,
            doc_name: "Demo network".into(),
            hist: History::default(),
            changed: false,
            clip: Vec::new(),
            now: 0.0,
            continuous: false,
            active_doc: 1,
            docs: std::collections::HashMap::new(),
            doc_order: vec![1],
            pending_doc: None,
            confirm_close: None,
            sel: Sel::Slave(4),
            master_tab: MasterTab::General,
            slave_tab: SlaveTab::ProcessData,
            dock: default_dock(1),
            editors: std::collections::HashMap::new(),
            active_editor: 1,
            pending_active: None,
            route_pending: false,
            closed_editors: Vec::new(),
            last_sel: Sel::Master,
            windows: Vec::new(),
            open_requests: Vec::new(),
            deferred: Vec::new(),
            focus_req: None,
            filter: String::new(),
            od_filter: String::new(),
            soe_form: SoeForm::default(),
            coe_scroll: false,
            log: Vec::new(),
            dirty: false,
            eni_text: String::new(),
            sii_cache: None,
            sel_pdo: Some(0),
            coe_form: CoeForm {
                transition: "PS".into(),
                ..Default::default()
            },
            scan_rx: None,
            scanned: Vec::new(),
            adapters: scan::list_adapters(),
            show_about: false,
            settings,
            persist,
        };
        INST.fetch_max(2, std::sync::atomic::Ordering::Relaxed);
        if let Some(d) = app.settings.dock.take() {
            if d.iter_all_tabs().any(|(_, t)| matches!(t, Tab::Editor { .. })) {
                app.dock = d;
            }
        }
        // states for the editors of the (possibly restored) layout; the first one is active
        let insts: Vec<u64> = app
            .dock
            .iter_all_tabs()
            .filter_map(|(_, t)| if let Tab::Editor { inst } = t { Some(*inst) } else { None })
            .collect();
        for i in insts.iter() {
            INST.fetch_max(i + 1, std::sync::atomic::Ordering::Relaxed);
            app.editors.insert(*i, EditorState::default());
        }
        app.active_editor = insts.first().copied().unwrap_or(1);
        app.info("EtherCAT configurator started");
        for (name, xml) in [
            ("built-in: example_io.xml", SAMPLE_IO),
            ("built-in: example_drive.xml", SAMPLE_DRIVE),
            ("built-in: example_modular.xml", SAMPLE_MODULAR),
        ] {
            match app.lib.add_xml(xml, name) {
                Ok(n) => app.info(&format!("Loaded {} device(s) from {}", n, name)),
                Err(e) => app.err(&format!("{}: {}", name, e)),
            }
        }
        for p in app.settings.esi_paths.clone() {
            app.load_esi_path(&PathBuf::from(p), false);
        }
        for p in opts.esi.iter() {
            app.load_esi_path(p, true);
        }
        app.project = demo_project(&app.lib);
        app.info(&format!(
            "Demo network with {} slaves created (File > New for an empty one)",
            app.project.slaves.len()
        ));
        if let Some(i) = std::env::var("ECAT_START_SLAVE").ok().and_then(|v| v.parse::<usize>().ok()) {
            if i < app.project.slaves.len() {
                app.sel = Sel::Slave(i);
            }
        }
        // optional start view (used for documentation pictures)
        if let Ok(v) = std::env::var("ECAT_START_VIEW") {
            match v.as_str() {
                "topology" => {
                    app.sel = Sel::Master;
                    app.master_tab = MasterTab::Topology;
                }
                "image" => {
                    app.sel = Sel::Master;
                    app.master_tab = MasterTab::Image;
                }
                "eni" => {
                    app.sel = Sel::Master;
                    app.master_tab = MasterTab::Eni;
                    app.eni_text = eni::export_eni(&app.project);
                }
                "mailbox" => {
                    app.slave_tab = SlaveTab::Mailbox;
                }
                "dictionary" => {
                    if let Some(k) = app.project.slaves.iter().position(|s| s.device.type_name == "SD200") {
                        app.sel = Sel::Slave(k);
                    }
                    app.slave_tab = SlaveTab::Dictionary;
                }
                "modules" => {
                    if let Some(k) = app.project.slaves.iter().position(|s| !s.device.slots.is_empty()) {
                        app.sel = Sel::Slave(k);
                    }
                    app.slave_tab = SlaveTab::Modules;
                }
                "init" => {
                    app.slave_tab = SlaveTab::InitCmds;
                }
                "eeprom" => {
                    app.slave_tab = SlaveTab::Eeprom;
                    if let Sel::Slave(i) = app.sel {
                        app.sii_cache = Some((i, sii::build_sii(&app.project.slaves[i].device, 0, 0)));
                    }
                }
                _ => {}
            }
        }
        app.project.ensure_uids();
        let first = app.active_editor;
        let target = app.sel_to_target(app.sel);
        let (mp, sp, sc, spdo) = (app.master_tab, app.slave_tab, app.sii_cache.take(), app.sel_pdo.take());
        let st = app.editors.entry(first).or_default();
        if st.target.is_none() {
            st.target = target;
        }
        st.master_page = mp;
        st.slave_page = sp;
        st.sii_cache = sc;
        st.sel_pdo = spdo;
        app.last_sel = app.sel;
        // files given on the command line open as separate documents
        if let Some(p) = opts.project {
            app.open_project(&p);
        }
        if let Some(p) = opts.eni {
            app.import_eni_file(&p);
        }
        if !opts.files.is_empty() {
            app.open_files(opts.files.clone());
        }
        // test hook: build a multi-view layout (splits, floating panel, window)
        if std::env::var("ECAT_TEST_LAYOUT").as_deref() == Ok("split") {
            // new empty split becomes active; a tree selection then goes only there
            app.open_view(None, Place::SplitRight);
            app.select(Sel::Slave(1));
        } else if std::env::var("ECAT_TEST_LAYOUT").is_ok() {
            app.project.ensure_uids();
            let u0 = app.project.slaves.get(1).map(|s| s.uid).unwrap_or(0);
            let u1 = app.project.slaves.get(5).map(|s| s.uid).unwrap_or(0);
            app.open_view(Some(Target::Slave(u0)), Place::SplitRight);
            app.open_view(Some(Target::Master), Place::SplitBelow);
            app.open_view(Some(Target::Slave(u1)), Place::NewTab);
            app.open_view(None, Place::Floating);
            app.open_view(Some(Target::Slave(u0)), Place::Window);
        }
        if let Some(a) = app.adapters.first() {
            if app.project.master.adapter.is_empty() {
                app.project.master.adapter = a.0.clone();
            }
        }
        app
    }

    // ------------------------------------------------------------ messages
    fn info(&mut self, t: &str) {
        self.log.push((now_hms(), Severity::Info, t.to_string()));
    }
    fn warn(&mut self, t: &str) {
        self.log.push((now_hms(), Severity::Warning, t.to_string()));
    }
    fn err(&mut self, t: &str) {
        self.log.push((now_hms(), Severity::Error, t.to_string()));
        self.focus_req = Some(Tab::Messages);
    }

    fn save_settings(&self) {
        if !self.persist {
            return;
        }
        if let Some(p) = settings_path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, serde_json::to_string_pretty(&self.settings).unwrap_or_default());
        }
    }

    // ------------------------------------------------------------ file actions
    fn load_esi_path(&mut self, p: &std::path::Path, remember: bool) {
        let mut files = Vec::new();
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let ep = e.path();
                    if ep.extension().map(|x| x.eq_ignore_ascii_case("xml")).unwrap_or(false) {
                        files.push(ep);
                    }
                }
            }
            files.sort();
        } else {
            files.push(p.to_path_buf());
        }
        let mut total = 0;
        for f in files.iter() {
            match self.lib.add_file(f) {
                Ok(n) => {
                    total += n;
                    self.info(&format!("ESI {}: {} device(s)", f.display(), n));
                }
                Err(e) => self.warn(&format!("ESI {} skipped: {}", f.display(), e)),
            }
        }
        if remember && total > 0 {
            let s = p.display().to_string();
            if !self.settings.esi_paths.contains(&s) {
                self.settings.esi_paths.push(s);
                self.save_settings();
            }
        }
    }

    fn add_esi_dialog(&mut self, folder: bool) {
        if folder {
            if let Some(d) = rfd::FileDialog::new().set_title("Add ESI folder").pick_folder() {
                self.load_esi_path(&d, true);
            }
        } else if let Some(fs) = rfd::FileDialog::new()
            .set_title("Add ESI files")
            .add_filter("ESI (*.xml)", &["xml", "XML"])
            .pick_files()
        {
            for f in fs {
                self.load_esi_path(&f, true);
            }
        }
    }

    // ------------------------------------------------------------ undo / redo / clipboard
    /// Record the changes made this frame as one undo step (or merge them into the last one).
    fn commit_history(&mut self) {
        if !self.changed {
            if self.hist.base.is_none() {
                self.hist.base = Some(self.project.clone());
            }
            return;
        }
        self.changed = false;
        let base = self.hist.base.take().unwrap_or_else(|| self.project.clone());
        // typing in a field or dragging a value merges into one step; every other action is its own step
        let merge = self.continuous && self.now - self.hist.last_change <= 0.8 && !self.hist.undo.is_empty();
        if !merge {
            self.hist.undo.push(base);
            if self.hist.undo.len() > UNDO_LIMIT {
                self.hist.undo.remove(0);
            }
        }
        self.hist.redo.clear();
        self.hist.base = Some(self.project.clone());
        self.hist.last_change = self.now;
    }

    fn undo(&mut self) {
        self.commit_history();
        if let Some(prev) = self.hist.undo.pop() {
            let cur = std::mem::replace(&mut self.project, prev);
            self.hist.redo.push(cur);
            self.hist.base = Some(self.project.clone());
            self.hist.last_change = f64::MIN;
            self.dirty = true;
            self.resync_selection();
            self.info(&format!("Undo ({} more step(s) available)", self.hist.undo.len()));
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.hist.redo.pop() {
            let cur = std::mem::replace(&mut self.project, next);
            self.hist.undo.push(cur);
            self.hist.base = Some(self.project.clone());
            self.hist.last_change = f64::MIN;
            self.dirty = true;
            self.resync_selection();
            self.info("Redo");
        }
    }

    /// After the project was replaced (undo/redo), point the tree at what the active editor shows.
    fn resync_selection(&mut self) {
        match self.editors.get(&self.active_editor).and_then(|e| e.target).and_then(|t| self.target_to_sel(t)) {
            Some(sel) => {
                self.sel = sel;
                self.last_sel = sel;
            }
            None => {
                // the item the editor showed is gone: show the tree selection there instead
                self.fix_selection();
                let t = self.sel_to_target(self.sel);
                if let Some(e) = self.editors.get_mut(&self.active_editor) {
                    if e.target.is_some() {
                        e.target = t;
                        e.sel_pdo = None;
                    }
                }
                self.last_sel = self.sel;
            }
        }
    }

    fn copy_selected(&mut self, ctx: &egui::Context) -> bool {
        if let Sel::Slave(i) = self.sel {
            if let Some(s) = self.project.slaves.get(i) {
                self.clip = vec![s.clone()];
                if let Ok(j) = serde_json::to_string(&serde_json::json!({ "ecat_configurator_slaves": self.clip })) {
                    ctx.copy_text(j);
                }
                let name = s.name.clone();
                self.info(&format!("Copied {}", name));
                return true;
            }
        }
        false
    }

    /// Paste copied slaves after the selected one (from the system clipboard when it holds slaves).
    fn paste(&mut self, text: Option<&str>) {
        let from_text: Option<Vec<SlaveCfg>> = text
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .and_then(|v| v.get("ecat_configurator_slaves").cloned())
            .and_then(|v| serde_json::from_value(v).ok());
        let list = from_text.unwrap_or_else(|| self.clip.clone());
        if list.is_empty() {
            return;
        }
        let mut at = match self.sel {
            Sel::Slave(i) => Some(i),
            _ => self.project.slaves.len().checked_sub(1),
        };
        let mut first = None;
        for s in list.into_iter() {
            let idx = self.project.paste_slave(s, at);
            first.get_or_insert(idx);
            at = Some(idx);
        }
        if let Some(i) = at {
            self.select(Sel::Slave(i));
        }
        self.dirty = true;
        self.changed = true;
        self.info(&format!("Pasted at position {}", first.unwrap_or(0)));
    }

    fn duplicate_selected(&mut self) {
        if let Sel::Slave(i) = self.sel {
            if let Some(s) = self.project.slaves.get(i).cloned() {
                let idx = self.project.paste_slave(s, Some(i));
                self.select(Sel::Slave(idx));
                self.dirty = true;
                self.changed = true;
                self.info(&format!("Duplicated as position {}", idx));
            }
        }
    }

    // ------------------------------------------------------------ documents
    fn swap_doc(&mut self, d: &mut Doc) {
        std::mem::swap(&mut self.hist, &mut d.hist);
        std::mem::swap(&mut self.changed, &mut d.changed);
        std::mem::swap(&mut self.project, &mut d.project);
        std::mem::swap(&mut self.project_path, &mut d.path);
        std::mem::swap(&mut self.dirty, &mut d.dirty);
        std::mem::swap(&mut self.eni_text, &mut d.eni_text);
        std::mem::swap(&mut self.sel, &mut d.sel);
        std::mem::swap(&mut self.last_sel, &mut d.last_sel);
        std::mem::swap(&mut self.doc_name, &mut d.name);
    }

    /// Run `f` with document `id` loaded into the working fields, then put everything back.
    fn with_doc<R>(&mut self, id: u64, f: impl FnOnce(&mut App) -> R) -> Option<R> {
        if id == self.active_doc {
            return Some(f(self));
        }
        let mut d = self.docs.remove(&id)?;
        self.swap_doc(&mut d);
        let saved = self.active_doc;
        self.active_doc = id;
        if self.hist.base.is_none() {
            self.hist.base = Some(self.project.clone());
        }
        let r = f(self);
        self.commit_history();
        self.active_doc = saved;
        self.swap_doc(&mut d);
        self.docs.insert(id, d);
        Some(r)
    }

    /// Make `id` the active document (only between frames, never while views draw).
    fn activate_doc(&mut self, id: u64) {
        if id == self.active_doc {
            return;
        }
        let mut d = match self.docs.remove(&id) {
            Some(d) => d,
            None => return,
        };
        self.swap_doc(&mut d);
        self.docs.insert(self.active_doc, d);
        self.active_doc = id;
    }

    fn doc_name_of(&self, id: u64) -> String {
        if id == self.active_doc {
            self.doc_name.clone()
        } else {
            self.docs.get(&id).map(|d| d.name.clone()).unwrap_or_else(|| "closed".into())
        }
    }

    fn doc_dirty(&self, id: u64) -> bool {
        if id == self.active_doc {
            self.dirty
        } else {
            self.docs.get(&id).map(|d| d.dirty).unwrap_or(false)
        }
    }

    fn project_of(&self, id: u64) -> Option<&Project> {
        if id == self.active_doc {
            Some(&self.project)
        } else {
            self.docs.get(&id).map(|d| &d.project)
        }
    }

    /// Open a configuration as a new, separate document with its own editor tab.
    fn add_document(&mut self, name: String, project: Project, path: Option<PathBuf>, dirty: bool) -> u64 {
        let id = next_doc_id();
        let mut d = Doc {
            hist: History::default(),
            changed: false,
            name,
            project,
            path,
            dirty,
            eni_text: String::new(),
            sel: Sel::Master,
            last_sel: Sel::Master,
        };
        d.project.ensure_uids();
        if d.project.master.adapter.is_empty() {
            d.project.master.adapter = self.project.master.adapter.clone();
        }
        self.docs.insert(id, d);
        self.doc_order.push(id);
        self.activate_doc(id);
        self.open_view(Some(Target::Master), Place::NewTab);
        id
    }

    fn unique_name(&self, base: &str) -> String {
        let names: Vec<String> = self.doc_order.iter().map(|i| self.doc_name_of(*i)).collect();
        if !names.iter().any(|n| n == base) {
            return base.to_string();
        }
        (2..)
            .map(|k| format!("{} ({})", base, k))
            .find(|n| !names.contains(n))
            .unwrap_or_else(|| base.to_string())
    }

    fn new_project(&mut self) {
        let mut p = Project::default();
        p.master.adapter = self.project.master.adapter.clone();
        let name = self.unique_name("Untitled");
        self.add_document(name.clone(), p, None, false);
        self.info(&format!("New empty configuration \"{}\"", name));
    }

    fn open_project(&mut self, p: &std::path::Path) {
        let r = std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Project>(&s).map_err(|e| e.to_string()));
        match r {
            Ok(pr) => {
                let name = self.unique_name(&file_stem(p));
                self.add_document(name, pr, Some(p.to_path_buf()), false);
                self.info(&format!("Opened project {} as a new document", p.display()));
            }
            Err(e) => self.err(&format!("Cannot open {}: {}", p.display(), e)),
        }
    }

    fn save_project(&mut self, ask: bool) {
        let path = if ask || self.project_path.is_none() {
            let suggested = format!("{}.ecproj", self.doc_name.replace(['/', '\\'], "_"));
            rfd::FileDialog::new()
                .set_title("Save configuration project")
                .add_filter("Configuration project (*.ecproj)", &["ecproj"])
                .set_file_name(suggested)
                .save_file()
        } else {
            self.project_path.clone()
        };
        if let Some(p) = path {
            match std::fs::write(&p, serde_json::to_string_pretty(&self.project).unwrap_or_default()) {
                Ok(_) => {
                    self.info(&format!("Saved \"{}\" to {}", self.doc_name, p.display()));
                    self.doc_name = file_stem(&p);
                    self.project_path = Some(p);
                    self.dirty = false;
                }
                Err(e) => self.err(&format!("Cannot save: {}", e)),
            }
        }
    }

    fn import_eni_file(&mut self, p: &std::path::Path) {
        let r = std::fs::read(p)
            .map_err(|e| e.to_string())
            .and_then(|b| eni::import_eni(&decode_text(&b), &self.lib));
        match r {
            Ok((pr, notes)) => {
                let n = pr.slaves.len();
                let name = self.unique_name(&file_stem(p));
                self.add_document(name.clone(), pr, None, true);
                self.info(&format!("Imported ENI {} ({} slaves) as document \"{}\"", p.display(), n, name));
                for nt in notes {
                    self.warn(&nt);
                }
            }
            Err(e) => self.err(&format!("ENI import of {} failed: {}", p.display(), e)),
        }
    }

    /// Open any number of files: ENI and project files become documents, ESI files go to the catalog.
    fn open_files(&mut self, files: Vec<PathBuf>) {
        for f in files {
            let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ext == "ecproj" || ext == "json" {
                self.open_project(&f);
                continue;
            }
            let head = std::fs::read(&f).map(|b| decode_text(&b[..b.len().min(4096)])).unwrap_or_default();
            if head.contains("<EtherCATInfo") {
                self.load_esi_path(&f, true);
            } else if head.contains("<EtherCATConfig") {
                self.import_eni_file(&f);
            } else if f.is_dir() {
                self.load_esi_path(&f, true);
            } else {
                self.warn(&format!("{}: not an ENI, ESI or project file", f.display()));
            }
        }
    }

    fn close_document(&mut self, id: u64) {
        let name = self.doc_name_of(id);
        // drop its editors
        let dead: Vec<u64> = self.editors.iter().filter(|(_, e)| e.doc == id).map(|(k, _)| *k).collect();
        self.dock.retain_tabs(|t| !matches!(t, Tab::Editor { inst } if dead.contains(inst)));
        self.windows.retain(|w| !matches!(w.1, Tab::Editor { inst } if dead.contains(&inst)));
        for k in dead.iter() {
            self.editors.remove(k);
        }
        self.doc_order.retain(|d| *d != id);
        if id == self.active_doc {
            match self.doc_order.first().copied() {
                Some(next) => {
                    let mut d = self.docs.remove(&next).expect("open document");
                    self.swap_doc(&mut d);
                    self.active_doc = next;
                }
                None => {
                    let nid = next_doc_id();
                    self.project = Project::default();
                    self.hist = History::default();
                    self.changed = false;
                    self.project_path = None;
                    self.dirty = false;
                    self.eni_text.clear();
                    self.sel = Sel::Master;
                    self.last_sel = Sel::Master;
                    self.doc_name = "Untitled".into();
                    self.active_doc = nid;
                    self.doc_order.push(nid);
                }
            }
        } else {
            self.docs.remove(&id);
        }
        self.info(&format!("Closed document \"{}\"", name));
        self.focus_doc(self.active_doc);
    }

    /// Bring a document to the front: activate one of its editors or open a new one.
    fn focus_doc(&mut self, id: u64) {
        self.activate_doc(id);
        let ed = self
            .dock
            .iter_all_tabs()
            .filter_map(|(_, t)| if let Tab::Editor { inst } = t { Some(*inst) } else { None })
            .find(|i| self.editors.get(i).map(|e| e.doc == id).unwrap_or(false));
        match ed {
            Some(inst) => {
                self.active_editor = inst;
                if let Some(loc) = self.dock.find_tab(&Tab::Editor { inst }) {
                    self.dock.set_active_tab(loc);
                }
            }
            None => self.open_view(Some(Target::Master), Place::NewTab),
        }
    }

    fn export_eni_dialog(&mut self) {
        let lay = compute_layout(&self.project);
        let errors = validate(&self.project, &lay).into_iter().filter(|i| i.sev == Severity::Error).count();
        if errors > 0 {
            self.warn(&format!("Exporting ENI with {} validation error(s); see the Validation tab", errors));
        }
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Export ENI")
            .add_filter("ENI (*.xml)", &["xml"])
            .set_file_name("eni.xml")
            .save_file()
        {
            match std::fs::write(&p, eni::export_eni(&self.project)) {
                Ok(_) => self.info(&format!("ENI written to {}", p.display())),
                Err(e) => self.err(&format!("Cannot write ENI: {}", e)),
            }
        }
    }

    fn save_text(&mut self, title: &str, name: &str, ext: &str, text: &[u8]) {
        if let Some(p) = rfd::FileDialog::new().set_title(title).add_filter(ext, &[ext]).set_file_name(name).save_file() {
            match std::fs::write(&p, text) {
                Ok(_) => self.info(&format!("Written {}", p.display())),
                Err(e) => self.err(&format!("Cannot write {}: {}", p.display(), e)),
            }
        }
    }

    fn add_catalog_device(&mut self, di: usize) {
        let dev = self.lib.devices[di].clone();
        let after = match self.sel {
            Sel::Slave(i) => Some(i),
            _ => self.project.slaves.len().checked_sub(1),
        };
        let idx = self.project.add_device(&dev, after);
        self.sel = Sel::Slave(idx);
        self.sel_pdo = None;
        self.dirty = true;
        self.changed = true;
        let name = self.project.slaves[idx].name.clone();
        self.info(&format!("Added {} at position {}", name, idx));
    }

    fn remove_selected(&mut self) {
        if let Sel::Slave(i) = self.sel {
            let name = self.project.slaves[i].name.clone();
            self.project.remove(i);
            self.sel = if self.project.slaves.is_empty() {
                Sel::Master
            } else {
                Sel::Slave(i.min(self.project.slaves.len() - 1))
            };
            self.dirty = true;
            self.changed = true;
            self.info(&format!("Removed {}", name));
        }
    }

    fn move_selected(&mut self, up: bool) {
        if let Sel::Slave(i) = self.sel {
            if up && i > 0 {
                self.project.move_down(i - 1);
                self.sel = Sel::Slave(i - 1);
                self.dirty = true;
                self.changed = true;
            } else if !up && i + 1 < self.project.slaves.len() {
                self.project.move_down(i);
                self.sel = Sel::Slave(i + 1);
                self.dirty = true;
                self.changed = true;
            }
        }
    }

    fn start_scan(&mut self) {
        if self.scan_rx.is_some() {
            return;
        }
        self.scanned.clear();
        self.scan_rx = Some(scan::start_scan(self.project.master.adapter.clone()));
    }

    fn poll_scan(&mut self) {
        let mut done = false;
        if let Some(rx) = &self.scan_rx {
            let msgs: Vec<ScanMsg> = rx.try_iter().collect();
            for m in msgs {
                match m {
                    ScanMsg::Log(t) => self.log.push((now_hms(), Severity::Info, t)),
                    ScanMsg::Done(Ok(v)) => {
                        self.log.push((now_hms(), Severity::Info, format!("Scan finished: {} slave(s) found", v.len())));
                        self.scanned = v;
                        done = true;
                    }
                    ScanMsg::Done(Err(e)) => {
                        self.log.push((now_hms(), Severity::Error, format!("Scan failed: {}", e)));
                        self.focus_req = Some(Tab::Messages);
                        done = true;
                    }
                }
            }
        }
        if done {
            self.scan_rx = None;
        }
    }

    fn config_from_scan(&mut self) {
        let mut p = Project {
            master: self.project.master.clone(),
            slaves: vec![],
        };
        let scanned = self.scanned.clone();
        for s in scanned.iter() {
            let dev = match self.lib.find_best(s.vendor, s.product, s.revision) {
                Some(d) => d.clone(),
                None => {
                    self.warn(&format!(
                        "No ESI for vendor {:#010x} product {:#010x}: added as unknown device",
                        s.vendor, s.product
                    ));
                    EsiDevice {
                        vendor_id: s.vendor,
                        vendor_name: format!("Vendor {}", hx(s.vendor, 8)),
                        type_name: format!("Unknown {:08X}", s.product),
                        name: "Unknown device (no ESI)".into(),
                        product_code: s.product,
                        revision: s.revision,
                        group_type: "Unknown".into(),
                        group_name: "Unknown".into(),
                        physics: "YY".into(),
                        fmmus: vec![],
                        sms: vec![],
                        rx_pdos: vec![],
                        tx_pdos: vec![],
                        mailbox: None,
                        dc_opmodes: vec![],
                        eeprom: None,
                        profile_no: None,
                        source_file: "scan".into(),
                        slots: vec![],
                        modules: vec![],
                        dictionary: vec![],
                    }
                }
            };
            let at = p.slaves.len().checked_sub(1);
            let i = p.add_device(&dev, at);
            p.slaves[i].serial = s.serial;
        }
        // wiring from the ports each slave communicates on (DL status 0x0110)
        let open: Vec<Vec<u8>> = scanned.iter().map(|s| open_ports_from_dl_status(s.dl_status)).collect();
        if open.iter().any(|o| !o.is_empty()) {
            for (k, (parent, port)) in topology_from_ports(&open).into_iter().enumerate() {
                if let Some(s) = p.slaves.get_mut(k) {
                    s.parent = parent;
                    s.parent_port = port;
                }
            }
        } else {
            p.relinearize();
        }
        let name = self.unique_name("Scanned network");
        self.add_document(name.clone(), p, None, true);
        self.info(&format!("Configuration created from scanned bus as document \"{}\"", name));
    }

    // ------------------------------------------------------------ UI parts
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New configuration (new document)").clicked() {
                    self.new_project();
                    ui.close_menu();
                }
                if ui.button("Open files… (ENI, ESI, projects; several at once)").clicked() {
                    ui.close_menu();
                    if let Some(fs) = rfd::FileDialog::new()
                        .set_title("Open ENI / ESI / project files")
                        .add_filter("EtherCAT files (*.xml, *.ecproj)", &["xml", "XML", "ecproj", "json"])
                        .pick_files()
                    {
                        self.open_files(fs);
                    }
                }
                if ui.button("Close document").clicked() {
                    self.confirm_close = Some(self.active_doc);
                    ui.close_menu();
                }
                if ui.button("Open project…        Ctrl+O").clicked() {
                    ui.close_menu();
                    if let Some(fs) = rfd::FileDialog::new()
                        .set_title("Open projects (several allowed)")
                        .add_filter("Configuration project (*.ecproj)", &["ecproj", "json"])
                        .pick_files()
                    {
                        self.open_files(fs);
                    }
                }
                if ui.button("Save project          Ctrl+S").clicked() {
                    ui.close_menu();
                    self.save_project(false);
                }
                if ui.button("Save project as…").clicked() {
                    ui.close_menu();
                    self.save_project(true);
                }
                ui.separator();
                if ui.button("Import ENI…").clicked() {
                    ui.close_menu();
                    if let Some(fs) = rfd::FileDialog::new()
                        .set_title("Import ENI files (several allowed)")
                        .add_filter("ENI (*.xml)", &["xml", "XML"])
                        .pick_files()
                    {
                        for f in fs {
                            self.import_eni_file(&f);
                        }
                    }
                }
                if ui.button("Export ENI…            Ctrl+E").clicked() {
                    ui.close_menu();
                    self.export_eni_dialog();
                }
                ui.separator();
                if ui.button("Export process image as C header…").clicked() {
                    ui.close_menu();
                    let lay = compute_layout(&self.project);
                    let t = export::c_header(&self.project, &lay);
                    self.save_text("Export C header", "ecat_process_image.h", "h", t.as_bytes());
                }
                if ui.button("Export process image as CSV…").clicked() {
                    ui.close_menu();
                    let lay = compute_layout(&self.project);
                    let t = export::csv(&lay);
                    self.save_text("Export CSV", "process_image.csv", "csv", t.as_bytes());
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let is_slave = matches!(self.sel, Sel::Slave(_));
                if ui
                    .add_enabled(
                        !self.hist.undo.is_empty(),
                        egui::Button::new(format!("Undo  ({})        Ctrl+Z", self.hist.undo.len())),
                    )
                    .clicked()
                {
                    self.undo();
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        !self.hist.redo.is_empty(),
                        egui::Button::new(format!("Redo  ({})        Ctrl+Y", self.hist.redo.len())),
                    )
                    .clicked()
                {
                    self.redo();
                    ui.close_menu();
                }
                ui.separator();
                if ui.add_enabled(is_slave, egui::Button::new("Cut slave                 Ctrl+X")).clicked() {
                    if self.copy_selected(ui.ctx()) {
                        self.remove_selected();
                    }
                    ui.close_menu();
                }
                if ui.add_enabled(is_slave, egui::Button::new("Copy slave               Ctrl+C")).clicked() {
                    self.copy_selected(ui.ctx());
                    ui.close_menu();
                }
                if ui
                    .add_enabled(!self.clip.is_empty(), egui::Button::new("Paste slave              Ctrl+V"))
                    .clicked()
                {
                    self.paste(None);
                    ui.close_menu();
                }
                if ui.add_enabled(is_slave, egui::Button::new("Duplicate slave        Ctrl+D")).clicked() {
                    self.duplicate_selected();
                    ui.close_menu();
                }
                ui.separator();
                ui.label(
                    RichText::new("Copied slaves can be pasted into another document,\nor into another running copy of the program.")
                        .small()
                        .weak(),
                );
            });
            ui.menu_button("Device Catalog", |ui| {
                if ui.button("Add ESI files…").clicked() {
                    ui.close_menu();
                    self.add_esi_dialog(false);
                }
                if ui.button("Add ESI folder…").clicked() {
                    ui.close_menu();
                    self.add_esi_dialog(true);
                }
                if ui.button("Reload catalog").clicked() {
                    ui.close_menu();
                    self.lib = EsiLibrary::default();
                    let _ = self.lib.add_xml(SAMPLE_IO, "built-in: example_io.xml");
                    let _ = self.lib.add_xml(SAMPLE_DRIVE, "built-in: example_drive.xml");
                    let _ = self.lib.add_xml(SAMPLE_MODULAR, "built-in: example_modular.xml");
                    for p in self.settings.esi_paths.clone() {
                        self.load_esi_path(&PathBuf::from(p), false);
                    }
                    self.sel = Sel::Master;
                    self.info(&format!("Catalog reloaded: {} devices", self.lib.devices.len()));
                }
                if ui.button("Forget remembered ESI paths").clicked() {
                    ui.close_menu();
                    self.settings.esi_paths.clear();
                    self.save_settings();
                    self.info("Remembered ESI paths cleared (built-in samples stay)");
                }
            });
            ui.menu_button("Network", |ui| {
                if ui.button("Remove selected slave      Del").clicked() {
                    ui.close_menu();
                    self.remove_selected();
                }
                if ui.button("Move slave up").clicked() {
                    ui.close_menu();
                    self.move_selected(true);
                }
                if ui.button("Move slave down").clicked() {
                    ui.close_menu();
                    self.move_selected(false);
                }
                if ui.button("Re-number station addresses (1001…)").clicked() {
                    ui.close_menu();
                    for (i, s) in self.project.slaves.iter_mut().enumerate() {
                        s.phys_addr = 1001 + i as u16;
                    }
                    self.dirty = true;
                    self.changed = true;
                    self.info("Station addresses renumbered");
                }
                if ui.button("Sort slaves by wiring (ports D, B, C)").clicked() {
                    ui.close_menu();
                    let uid = if let Sel::Slave(i) = self.sel {
                        self.project.slaves.get(i).map(|s| s.uid)
                    } else {
                        None
                    };
                    if self.project.sort_by_topology() {
                        if let Some(k) = uid.and_then(|u| self.project.index_of_uid(u)) {
                            self.sel = Sel::Slave(k);
                            self.last_sel = self.sel;
                        }
                        self.dirty = true;
                        self.changed = true;
                        self.info("Slaves sorted into bus order");
                    } else {
                        self.info("The slave list already follows the wiring");
                    }
                }
                if ui.button("Make topology linear").clicked() {
                    ui.close_menu();
                    self.project.relinearize();
                    self.dirty = true;
                    self.changed = true;
                }
                ui.separator();
                if ui.button("Scan bus…").clicked() {
                    ui.close_menu();
                    self.select(Sel::Master);
                    self.active_state().master_page = MasterTab::Online;
                }
                if ui.button("Validate configuration").clicked() {
                    ui.close_menu();
                    self.focus_req = Some(Tab::Validation);
                }
            });
            ui.menu_button("View", |ui| {
                for (label, place) in [("➕ New empty tab", Place::NewTab), ("New empty split right", Place::SplitRight), ("New empty split below", Place::SplitBelow), ("New empty window", Place::Window)] {
                    if ui.button(label).clicked() {
                        self.open_requests.push((None, place));
                        ui.close_menu();
                    }
                }
                let target = self.sel_to_target(self.sel);
                ui.add_enabled_ui(target.is_some(), |ui| {
                    ui.label(RichText::new("Copy of the current item").weak());
                    if let Some(t) = target {
                        open_menu(ui, &mut self.open_requests, t);
                    }
                });
                ui.separator();
                ui.label(RichText::new("Panels").weak());
                for (label, t) in [
                    ("🖧 Configuration", Tab::Network),
                    ("📚 Device Catalog", Tab::Catalog),
                    ("💬 Messages", Tab::Messages),
                    ("✔ Validation", Tab::Validation),
                ] {
                    if ui.button(label).clicked() {
                        self.focus_req = Some(t);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("Close all other editors and windows").clicked() {
                    let keep = self.active_editor;
                    self.dock.retain_tabs(|t| match t {
                        Tab::Editor { inst } => *inst == keep,
                        _ => true,
                    });
                    self.windows.retain(|w| w.1 == Tab::Editor { inst: keep });
                    ui.close_menu();
                }
                if ui.button("Reset layout").clicked() {
                    let keep = self.active_editor;
                    self.dock = default_dock(keep);
                    self.windows.clear();
                    ui.close_menu();
                }
                ui.separator();
                ui.label(RichText::new("Each editor is independent. Items you click in the tree open in\nthe active editor (marked ✏). Click inside an editor to make it active.\nDrag a tab onto a pane's edge to split, out of the window to float it.").small().weak());
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About").clicked() {
                    ui.close_menu();
                    self.show_about = true;
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("🗋 New").on_hover_text("New empty configuration").clicked() {
                self.new_project();
            }
            if ui.button("📂 Open").on_hover_text("Open a configuration project").clicked() {
                if let Some(fs) = rfd::FileDialog::new()
                    .set_title("Open projects (several allowed)")
                    .add_filter("Configuration project (*.ecproj)", &["ecproj", "json"])
                    .pick_files()
                {
                    self.open_files(fs);
                }
            }
            if ui.button("💾 Save").clicked() {
                self.save_project(false);
            }
            if ui
                .add_enabled(!self.hist.undo.is_empty(), egui::Button::new("⟲"))
                .on_hover_text("Undo (Ctrl+Z)")
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(!self.hist.redo.is_empty(), egui::Button::new("⟳"))
                .on_hover_text("Redo (Ctrl+Y)")
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            if ui
                .button("➕ Add ESI")
                .on_hover_text("Load ESI device description files into the catalog")
                .clicked()
            {
                self.add_esi_dialog(false);
            }
            if let Ok(f) = std::env::var("ECAT_TEST_IMPORT") {
                if ui.ctx().cumulative_pass_nr() == 60 {
                    self.import_eni_file(std::path::Path::new(&f));
                }
                if std::env::var("ECAT_TEST_EMPTY").is_ok() && ui.ctx().cumulative_pass_nr() == 2 {
                    self.new_project();
                }
            }
            if ui.button("📥 Import ENI").clicked() {
                if let Some(fs) = rfd::FileDialog::new()
                    .set_title("Import ENI files (several allowed)")
                    .add_filter("ENI (*.xml)", &["xml", "XML"])
                    .pick_files()
                {
                    for f in fs {
                        self.import_eni_file(&f);
                    }
                }
            }
            if ui
                .add(egui::Button::new(RichText::new("📤 Export ENI").color(Color32::WHITE)).fill(ACCENT))
                .clicked()
            {
                self.export_eni_dialog();
            }
            ui.separator();
            let is_slave = matches!(self.sel, Sel::Slave(_));
            if ui.add_enabled(is_slave, egui::Button::new("⬆")).on_hover_text("Move slave up").clicked() {
                self.move_selected(true);
            }
            if ui.add_enabled(is_slave, egui::Button::new("⬇")).on_hover_text("Move slave down").clicked() {
                self.move_selected(false);
            }
            if ui.add_enabled(is_slave, egui::Button::new("🗑 Remove")).clicked() {
                self.remove_selected();
            }
            ui.separator();
            let scanning = self.scan_rx.is_some();
            if ui
                .add_enabled(!scanning, egui::Button::new(if scanning { "⏳ Scanning…" } else { "🔍 Scan bus" }))
                .clicked()
            {
                self.select(Sel::Master);
                self.active_state().master_page = MasterTab::Online;
                self.start_scan();
            }
            if ui.button("✔ Validate").clicked() {
                self.focus_req = Some(Tab::Validation);
            }
            ui.separator();
            for (icon, tip, place) in [
                (
                    "➕ Tab",
                    "New independent editor tab (it becomes the active editor: click an item to show it there)",
                    Place::NewTab,
                ),
                ("Split right", "New independent editor to the right", Place::SplitRight),
                ("Split below", "New independent editor below", Place::SplitBelow),
                ("New window", "New independent editor in a separate window", Place::Window),
            ] {
                if ui.button(icon).on_hover_text(tip).clicked() {
                    self.open_requests.push((None, place));
                }
            }
        });
    }

    fn network_view(&mut self, ui: &mut egui::Ui, lay: &NetLayout, issues: &[Issue]) {
        // open documents: each is an independent configuration
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Documents:").small().weak());
            for id in self.doc_order.clone() {
                let label = format!("📄 {}{}", self.doc_name_of(id), if self.doc_dirty(id) { "*" } else { "" });
                let r = ui
                    .selectable_label(id == self.active_doc, label)
                    .on_hover_text("Show this configuration (right-click to close)");
                if r.clicked() && id != self.active_doc {
                    self.pending_doc = Some(id);
                }
                r.context_menu(|ui| {
                    if ui.button("Close document").clicked() {
                        self.confirm_close = Some(id);
                        ui.close_menu();
                    }
                });
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("cfgtree").auto_shrink([false, false]).show(ui, |ui| {
            let mname = format!("🖧 {}  ({} slaves)", self.project.master.name, self.project.slaves.len());
            let r = ui.selectable_label(self.sel == Sel::Master, RichText::new(mname).strong());
            if r.clicked() {
                self.select(Sel::Master);
            }
            if r.double_clicked() {
                self.open_requests.push((Some(Target::Master), Place::NewTab));
            }
            r.context_menu(|ui| open_menu(ui, &mut self.open_requests, Target::Master));
            let mut depth = vec![0usize; self.project.slaves.len()];
            let refc = self.project.ref_clock_index();
            let mut pick: Option<Sel> = None;
            for i in 0..self.project.slaves.len() {
                let s = &self.project.slaves[i];
                depth[i] = match s.parent {
                    Some(p) if p < i => depth[p] + if s.parent_port >= 2 { 1 } else { 0 },
                    _ => 0,
                };
                let has_err = issues.iter().any(|x| x.slave == Some(i) && x.sev == Severity::Error);
                let has_warn = issues.iter().any(|x| x.slave == Some(i) && x.sev == Severity::Warning);
                let empty = SlaveLayout::default();
                let sl = lay.slaves.get(i).unwrap_or(&empty);
                ui.horizontal(|ui| {
                    ui.add_space(14.0 + 14.0 * depth[i] as f32);
                    let mark = if has_err {
                        "❌"
                    } else if has_warn {
                        "⚠"
                    } else {
                        "▪"
                    };
                    let mut text = format!("{} {} {}", mark, s.phys_addr, s.name);
                    if refc == Some(i) {
                        text.push_str("  ⏱");
                    }
                    let col = if has_err {
                        ERR
                    } else if has_warn {
                        WARN
                    } else {
                        ui.visuals().text_color()
                    };
                    let r = ui.selectable_label(self.sel == Sel::Slave(i), RichText::new(text).color(col));
                    let r = r.on_hover_text(format!(
                        "{}\nVendor {}  Product {}  Rev {}\nOutputs {} bytes, inputs {} bytes{}",
                        s.device.name,
                        hx(s.device.vendor_id, 8),
                        hx(s.device.product_code, 8),
                        hx(s.device.revision, 8),
                        sl.out_bytes,
                        sl.in_bytes,
                        if refc == Some(i) { "\nDC reference clock" } else { "" }
                    ));
                    if r.clicked() {
                        pick = Some(Sel::Slave(i));
                    }
                    if r.double_clicked() {
                        self.open_requests.push((Some(Target::Slave(s.uid)), Place::NewTab));
                    }
                    let uid = s.uid;
                    r.context_menu(|ui| open_menu(ui, &mut self.open_requests, Target::Slave(uid)));
                });
            }
            if self.project.slaves.is_empty() {
                ui.label(RichText::new("Double-click a device in the catalog to add it.").italics().weak());
            }
            if let Some(p) = pick {
                self.select(p);
            }
            if !self.project.slaves.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Double-click or right-click an item to open it in a new tab, split or window.")
                        .small()
                        .weak(),
                );
            }
        });
    }

    fn catalog_tree(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("🔎");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter by name, type or product code")
                    .desired_width(f32::INFINITY),
            );
        });
        let f = self.filter.to_lowercase();
        let mut add: Option<usize> = None;
        let mut pick: Option<usize> = None;
        egui::ScrollArea::vertical().id_salt("catalog").auto_shrink([false, false]).show(ui, |ui| {
            let mut vendors: Vec<(u32, String)> = self.lib.devices.iter().map(|d| (d.vendor_id, d.vendor_name.clone())).collect();
            vendors.sort();
            vendors.dedup();
            for (vid, vname) in vendors {
                egui::CollapsingHeader::new(format!("🏭 {} ({})", vname, hx(vid, 8)))
                    .id_salt(("v", vid))
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut groups: Vec<String> = self.lib.devices.iter().filter(|d| d.vendor_id == vid).map(|d| d.group_name.clone()).collect();
                        groups.sort();
                        groups.dedup();
                        for g in groups {
                            let idxs: Vec<usize> = (0..self.lib.devices.len())
                                .filter(|&k| {
                                    let d = &self.lib.devices[k];
                                    d.vendor_id == vid
                                        && d.group_name == g
                                        && (f.is_empty()
                                            || d.name.to_lowercase().contains(&f)
                                            || d.type_name.to_lowercase().contains(&f)
                                            || format!("{:08x}", d.product_code).contains(&f))
                                })
                                .collect();
                            if idxs.is_empty() {
                                continue;
                            }
                            egui::CollapsingHeader::new(format!("📁 {}", g))
                                .id_salt(("g", vid, g.clone()))
                                .default_open(true)
                                .show(ui, |ui| {
                                    for k in idxs {
                                        let d = &self.lib.devices[k];
                                        let r =
                                            ui.selectable_label(self.sel == Sel::Catalog(k), format!("{}  {}", d.type_name, short_name(&d.name, &d.type_name)));
                                        let r = r.on_hover_text(format!(
                                            "{}\nProduct {}  Rev {}\n{}\nDouble-click to add to the configuration",
                                            d.name,
                                            hx(d.product_code, 8),
                                            hx(d.revision, 8),
                                            d.source_file
                                        ));
                                        if r.double_clicked() {
                                            add = Some(k);
                                        } else if r.clicked() {
                                            pick = Some(k);
                                        }
                                        let key = Target::Device(d.vendor_id, d.product_code, d.revision);
                                        r.context_menu(|ui| {
                                            if ui.button("➕ Add to configuration").clicked() {
                                                add = Some(k);
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            open_menu(ui, &mut self.open_requests, key);
                                        });
                                    }
                                });
                        }
                    });
            }
        });
        if let Some(k) = add {
            self.deferred.push(Deferred::AddDevice(k));
        }
        if let Some(k) = pick {
            self.select(Sel::Catalog(k));
        }
    }

    fn messages_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} messages", self.log.len())).weak());
            if ui.small_button("Clear").clicked() {
                self.log.clear();
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("msgs")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (t, sev, m) in self.log.iter() {
                    let (icon, col) = sev_style(sev);
                    ui.label(RichText::new(format!("{}  {} {}", t, icon, m)).monospace().color(col));
                }
            });
    }

    fn validation_view(&mut self, ui: &mut egui::Ui, issues: &[Issue]) {
        egui::ScrollArea::vertical().id_salt("valid").auto_shrink([false, false]).show(ui, |ui| {
            for is in issues.iter() {
                let (icon, col) = sev_style(&is.sev);
                let r = ui.add(egui::Label::new(RichText::new(format!("{} {}", icon, is.text)).color(col)).sense(egui::Sense::click()));
                if let Some(s) = is.slave {
                    if r.on_hover_text("Click to select the slave").clicked() {
                        self.select(Sel::Slave(s));
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------ views / docking
    fn sel_to_target(&self, sel: Sel) -> Option<Target> {
        Some(match sel {
            Sel::Master => Target::Master,
            Sel::Slave(i) => Target::Slave(self.project.slaves.get(i)?.uid),
            Sel::Catalog(k) => {
                let d = self.lib.devices.get(k)?;
                Target::Device(d.vendor_id, d.product_code, d.revision)
            }
        })
    }

    fn target_to_sel(&self, t: Target) -> Option<Sel> {
        match t {
            Target::Master => Some(Sel::Master),
            Target::Slave(uid) => self.project.index_of_uid(uid).map(Sel::Slave),
            Target::Device(v, p, r) => self.lib.devices.iter().position(|d| d.key() == (v, p, r)).map(Sel::Catalog),
        }
    }

    fn active_state(&mut self) -> &mut EditorState {
        self.editors.entry(self.active_editor).or_default()
    }

    fn editor_exists(&self, inst: u64) -> bool {
        let t = Tab::Editor { inst };
        self.dock.find_tab(&t).is_some() || self.windows.iter().any(|w| w.1 == t)
    }

    /// Show `sel` in the active editor (creating one if every editor was closed).
    /// Called from inside views while the workspace is drawing; the selection is
    /// delivered to the active editor afterwards (see `apply_pending`), never mid-draw.
    fn select(&mut self, sel: Sel) {
        self.sel = sel;
        self.route_pending = true;
    }

    /// Send the tree selection to the active editor; called whenever `sel` changed.
    fn route_selection(&mut self) {
        self.last_sel = self.sel;
        let target = self.sel_to_target(self.sel);
        let same_doc = self.editors.get(&self.active_editor).map(|e| e.doc == self.active_doc).unwrap_or(false);
        if !self.editor_exists(self.active_editor) || !same_doc {
            self.open_view(target, Place::NewTab);
            return;
        }
        let st = self.active_state();
        if st.target != target {
            st.target = target;
            st.sel_pdo = None;
            st.coe_form = CoeForm {
                transition: "PS".into(),
                ..Default::default()
            };
        }
        if let Some(loc) = self.dock.find_tab(&Tab::Editor { inst: self.active_editor }) {
            self.dock.set_active_tab(loc);
        }
    }

    fn activate_editor(&mut self, inst: u64) {
        if self.active_editor == inst {
            return;
        }
        if let Some(d) = self.editors.get(&inst).map(|e| e.doc) {
            self.activate_doc(d);
        }
        self.active_editor = inst;
        if let Some(t) = self.editors.get(&inst).and_then(|e| e.target) {
            if let Some(sel) = self.target_to_sel(t) {
                self.sel = sel;
                self.last_sel = sel;
            }
        }
    }

    fn tab_target(&self, tab: &Tab) -> Option<Target> {
        match tab {
            Tab::Editor { inst } => self.editors.get(inst).and_then(|e| e.target),
            _ => None,
        }
    }

    fn target_title(&self, t: Option<Target>, st: Option<&EditorState>) -> String {
        let (mp, sp) = st.map(|e| (e.master_page, e.slave_page)).unwrap_or((MasterTab::General, SlaveTab::ProcessData));
        match t {
            None => "Empty editor".into(),
            Some(Target::Master) => format!("🖧 Master · {}", master_page_name(mp)),
            Some(Target::Slave(uid)) => {
                let pr = st.and_then(|e| self.project_of(e.doc)).unwrap_or(&self.project);
                match pr.index_of_uid(uid) {
                    Some(i) => format!("▪ {} {} · {}", pr.slaves[i].phys_addr, pr.slaves[i].device.type_name, slave_page_name(sp)),
                    None => "removed slave".into(),
                }
            }
            Some(Target::Device(v, p, r)) => format!("📦 {}", self.lib.find(v, p, r).map(|d| d.type_name.clone()).unwrap_or_else(|| "device".into())),
        }
    }

    fn tab_title(&self, tab: &Tab, issues: &[Issue]) -> String {
        match *tab {
            Tab::Network => "🖧 Configuration".into(),
            Tab::Catalog => "📚 Device Catalog".into(),
            Tab::Messages => format!("💬 Messages ({})", self.log.len()),
            Tab::Validation => {
                let ne = issues.iter().filter(|i| i.sev == Severity::Error).count();
                let nw = issues.iter().filter(|i| i.sev == Severity::Warning).count();
                format!("✔ Validation  ❌{} ⚠{}", ne, nw)
            }
            Tab::Editor { inst } => {
                let st = self.editors.get(&inst);
                let mut t = self.target_title(st.and_then(|e| e.target), st);
                if self.doc_order.len() > 1 {
                    if let Some(e) = st {
                        t = format!("[{}{}] {}", self.doc_name_of(e.doc), if self.doc_dirty(e.doc) { "*" } else { "" }, t);
                    }
                }
                if inst == self.active_editor {
                    format!("✏ {}", t)
                } else {
                    t
                }
            }
        }
    }

    fn editor_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new().fill(Color32::WHITE).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            add(ui);
        });
    }

    fn tab_ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab, lay: &NetLayout, issues: &[Issue]) {
        match *tab {
            Tab::Network => self.network_view(ui, lay, issues),
            Tab::Catalog => self.catalog_tree(ui),
            Tab::Messages => self.messages_view(ui),
            Tab::Validation => self.validation_view(ui, issues),
            Tab::Editor { inst } => self.editor_ui(ui, inst, lay),
        }
    }

    /// One independent editor. Its private state is swapped in while it draws.
    fn editor_ui(&mut self, ui: &mut egui::Ui, inst: u64, lay: &NetLayout) {
        if ui.ui_contains_pointer() && ui.input(|i| i.pointer.any_pressed()) {
            self.pending_active = Some(inst);
        }
        let doc = self.editors.get(&inst).map(|e| e.doc).unwrap_or(self.active_doc);
        if doc == self.active_doc {
            self.editor_body(ui, inst, lay);
        } else if self.docs.contains_key(&doc) {
            // another document: load it for the duration of this view only
            self.with_doc(doc, |me| {
                let l = compute_layout(&me.project);
                me.editor_body(ui, inst, &l);
            });
        } else {
            ui.label(RichText::new("The document of this view was closed.").italics().weak());
        }
    }

    fn editor_body(&mut self, ui: &mut egui::Ui, inst: u64, lay: &NetLayout) {
        let mut st = self.editors.remove(&inst).unwrap_or_default();
        std::mem::swap(&mut self.sel_pdo, &mut st.sel_pdo);
        std::mem::swap(&mut self.sii_cache, &mut st.sii_cache);
        std::mem::swap(&mut self.coe_form, &mut st.coe_form);
        let saved_sel = self.sel;
        let active = inst == self.active_editor;
        let shown = st.target.and_then(|t| self.target_to_sel(t));
        let mut picked: Option<Target> = None;
        ui.push_id(("editor", inst), |ui| {
            Self::editor_frame(ui, |ui| {
                let (txt, col) = if active {
                    ("✏ Active editor: items you click in Configuration or the Device Catalog open here", ACCENT)
                } else {
                    ("Independent editor. Click inside it to make it the active one.", Color32::from_gray(130))
                };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("📄 {}{}", self.doc_name, if self.dirty { " *" } else { "" }))
                            .small()
                            .strong()
                            .color(Color32::from_rgb(20, 40, 70)),
                    );
                    ui.label(RichText::new(txt).small().color(col));
                });
                match shown {
                    Some(sel) => {
                        self.sel = sel;
                        match sel {
                            Sel::Master => self.master_view(ui, lay, &mut st.master_page),
                            Sel::Slave(i) => self.slave_view(ui, i, lay, &mut st.slave_page),
                            Sel::Catalog(k) => self.catalog_view(ui, k),
                        }
                    }
                    None => {
                        if st.target.is_some() {
                            ui.label(RichText::new("The item shown here was removed. Pick another one:").italics().color(WARN));
                        }
                        picked = self.picker(ui);
                    }
                }
            });
        });
        self.sel = saved_sel;
        std::mem::swap(&mut self.sel_pdo, &mut st.sel_pdo);
        std::mem::swap(&mut self.sii_cache, &mut st.sii_cache);
        std::mem::swap(&mut self.coe_form, &mut st.coe_form);
        if let Some(t) = picked {
            st.target = Some(t);
            st.sel_pdo = None;
            self.pending_active = Some(inst);
        }
        self.editors.insert(inst, st);
    }

    /// Content of an empty editor: choose what to show.
    fn picker(&mut self, ui: &mut egui::Ui) -> Option<Target> {
        let mut out = None;
        title(ui, "Empty editor", "Pick an item, or click one in Configuration or the Device Catalog");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            sub_header(ui, "Network");
            if ui.button(format!("🖧 {}", self.project.master.name)).clicked() {
                out = Some(Target::Master);
            }
            for s in self.project.slaves.iter() {
                if ui.button(format!("▪ {}  {}", s.phys_addr, s.name)).clicked() {
                    out = Some(Target::Slave(s.uid));
                }
            }
            ui.add_space(8.0);
            sub_header(ui, "Device catalog");
            for d in self.lib.devices.iter() {
                if ui.button(format!("📦 {}  {}", d.type_name, short_name(&d.name, &d.type_name))).clicked() {
                    out = Some(Target::Device(d.vendor_id, d.product_code, d.revision));
                }
            }
        });
        out
    }

    /// Where new tabs and splits go: next to the active editor, else any editor, else the focused pane.
    fn editor_node(&self) -> (egui_dock::SurfaceIndex, egui_dock::NodeIndex) {
        if let Some((s, n, _)) = self.dock.find_tab(&Tab::Editor { inst: self.active_editor }) {
            return (s, n);
        }
        if let Some(t) = self.dock.iter_all_tabs().map(|(_, t)| *t).find(|t| matches!(t, Tab::Editor { .. })) {
            if let Some((s, n, _)) = self.dock.find_tab(&t) {
                return (s, n);
            }
        }
        self.dock
            .focused_leaf()
            .unwrap_or((egui_dock::SurfaceIndex::main(), egui_dock::NodeIndex::root()))
    }

    /// Open a new independent editor (empty, or showing `t`) and make it the active one.
    fn open_view(&mut self, t: Option<Target>, place: Place) {
        let inst = next_inst();
        let mut st = EditorState {
            doc: self.active_doc,
            target: t,
            ..Default::default()
        };
        if let Some(cur) = self.editors.get(&self.active_editor) {
            st.master_page = cur.master_page;
            st.slave_page = cur.slave_page;
        }
        self.editors.insert(inst, st);
        let tab = Tab::Editor { inst };
        match place {
            Place::NewTab => {
                let (s, n) = self.editor_node();
                self.dock.set_focused_node_and_surface((s, n));
                self.dock.push_to_focused_leaf(tab);
            }
            Place::SplitRight | Place::SplitBelow => {
                let (s, n) = self.editor_node();
                let tree = &mut self.dock[s];
                if tree.is_empty() {
                    self.dock.push_to_first_leaf(tab);
                } else if place == Place::SplitRight {
                    tree.split_right(n, 0.5, vec![tab]);
                } else {
                    tree.split_below(n, 0.5, vec![tab]);
                }
            }
            Place::Floating => {
                self.dock.add_window(vec![tab]);
            }
            Place::Window => self.windows.push((inst, tab)),
        }
        self.active_editor = inst;
        if let Some(sel) = t.and_then(|t| self.target_to_sel(t)) {
            self.sel = sel;
        }
        self.last_sel = self.sel;
    }

    fn reopen_panel(&mut self, t: Tab) {
        if let Some(loc) = self.dock.find_tab(&t) {
            self.dock.set_active_tab(loc);
            return;
        }
        if self.windows.iter().any(|w| w.1 == t) {
            return;
        }
        let (s, n) = match t {
            Tab::Messages | Tab::Validation => {
                let other = if t == Tab::Messages { Tab::Validation } else { Tab::Messages };
                match self.dock.find_tab(&other) {
                    Some((s, n, _)) => (s, n),
                    None => {
                        self.dock.main_surface_mut().split_below(egui_dock::NodeIndex::root(), 0.8, vec![t]);
                        return;
                    }
                }
            }
            Tab::Network | Tab::Catalog => {
                let other = if t == Tab::Network { Tab::Catalog } else { Tab::Network };
                match self.dock.find_tab(&other) {
                    Some((s, n, _)) => (s, n),
                    None => {
                        self.dock.main_surface_mut().split_left(egui_dock::NodeIndex::root(), 0.77, vec![t]);
                        return;
                    }
                }
            }
            _ => self.editor_node(),
        };
        self.dock.set_focused_node_and_surface((s, n));
        self.dock.push_to_focused_leaf(t);
    }

    fn apply_pending(&mut self) {
        if let Some(i) = self.pending_active.take() {
            self.activate_editor(i);
        }
        if let Some(d) = self.pending_doc.take() {
            self.focus_doc(d);
        }
        for d in std::mem::take(&mut self.deferred) {
            match d {
                Deferred::AddDevice(k) => {
                    if k < self.lib.devices.len() {
                        self.add_catalog_device(k);
                    }
                }
                Deferred::ConfigFromScan => self.config_from_scan(),
                Deferred::SortTopology => {
                    if self.project.sort_by_topology() {
                        self.resync_selection();
                        self.info("Slave list re-sorted to follow the wiring (ports D, B, C)");
                    }
                }
            }
        }
        for (t, p) in std::mem::take(&mut self.open_requests) {
            self.open_view(t, p);
        }
        if let Some(i) = self.pending_active.take() {
            self.activate_editor(i);
        }
        for i in std::mem::take(&mut self.closed_editors) {
            if !self.editor_exists(i) {
                self.editors.remove(&i);
            }
        }
        if self.route_pending || self.sel != self.last_sel {
            self.route_pending = false;
            self.route_selection();
        }
        if let Some(t) = self.focus_req.take() {
            self.reopen_panel(t);
        }
    }

    /// Views that were popped out into their own operating-system windows.
    fn show_windows(&mut self, ctx: &egui::Context) {
        let mut close: Vec<u64> = Vec::new();
        let mut dock_back: Vec<u64> = Vec::new();
        let wins = self.windows.clone();
        for (inst, mut tab) in wins {
            self.fix_selection();
            let lay = compute_layout(&self.project);
            let issues = validate(&self.project, &lay);
            let title = format!("{} — EtherCAT Configurator", self.tab_title(&tab, &issues));
            let vid = egui::ViewportId::from_hash_of(("view-window", inst));
            let builder = egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([980.0, 680.0])
                .with_min_inner_size([360.0, 240.0]);
            let win_title = self.tab_title(&tab, &issues);
            ctx.show_viewport_immediate(vid, builder, |ctx, class| {
                if class == egui::ViewportClass::Embedded {
                    // the platform cannot open another window: show it as a movable panel instead
                    let mut open = true;
                    egui::Window::new(win_title.clone())
                        .id(egui::Id::new(("wemb", inst)))
                        .open(&mut open)
                        .default_size([760.0, 520.0])
                        .show(ctx, |ui| {
                            if ui.button("⬇ Dock back into main window").clicked() {
                                dock_back.push(inst);
                            }
                            self.tab_ui(ui, &mut tab, &lay, &issues);
                        });
                    if !open {
                        close.push(inst);
                    }
                    return;
                }
                egui::TopBottomPanel::top(egui::Id::new(("wbar", inst))).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("⬇ Dock back into main window").clicked() {
                            dock_back.push(inst);
                        }
                        ui.label(RichText::new("Separate window: closing it does not change the configuration").weak());
                    });
                });
                egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::WHITE)).show(ctx, |ui| {
                    self.tab_ui(ui, &mut tab, &lay, &issues);
                });
                if ctx.input(|i| i.viewport().close_requested()) {
                    close.push(inst);
                }
            });
            if let Some(w) = self.windows.iter_mut().find(|w| w.0 == inst) {
                w.1 = tab;
            }
        }
        for inst in dock_back.iter() {
            if let Some(pos) = self.windows.iter().position(|w| w.0 == *inst) {
                let (_, tab) = self.windows.remove(pos);
                let (s, n) = self.editor_node();
                self.dock.set_focused_node_and_surface((s, n));
                self.dock.push_to_focused_leaf(tab);
            }
        }
        self.windows.retain(|w| !close.contains(&w.0));
    }

    pub fn save_layout(&mut self) {
        if !self.persist {
            return;
        }
        let mut d = self.dock.clone();
        // only the fixed panels and the selection editor are remembered; item views are per session
        let keep = self.active_editor;
        // the panels and the active editor are remembered; extra editors are per session
        d.retain_tabs(|t| match t {
            Tab::Editor { inst } => *inst == keep,
            _ => true,
        });
        self.settings.dock = if d.iter_all_tabs().next().is_some() { Some(d) } else { None };
        self.save_settings();
        self.settings.dock = None;
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, lay: &NetLayout) {
        ui.horizontal(|ui| {
            let p = &self.project;
            ui.label(format!("Slaves: {}", p.slaves.len()));
            ui.separator();
            ui.label(format!("Outputs: {} bytes", lay.out_size));
            ui.separator();
            ui.label(format!("Inputs: {} bytes", lay.in_size));
            ui.separator();
            ui.label(format!("Cycle: {} µs", p.master.cycle_time_us));
            ui.separator();
            let frames = cyclic_frames(p, lay);
            ui.label(format!("Expected WKC: {}", process_data_wkc(&frames)));
            if frames.len() > 1 {
                ui.separator();
                ui.label(format!("Frames: {}", frames.len()));
            }
            ui.separator();
            ui.label(format!("Catalog: {} devices", self.lib.devices.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let name = format!(
                    "📄 {} — {} ({} open)",
                    self.doc_name,
                    self.project_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "not saved yet".into()),
                    self.doc_order.len()
                );
                ui.label(RichText::new(format!("{}{}", name, if self.dirty { " *" } else { "" })).weak());
            });
        });
    }

    // ------------------------------------------------------------ catalog view
    fn catalog_view(&mut self, ui: &mut egui::Ui, k: usize) {
        let d = match self.lib.devices.get(k) {
            Some(d) => d.clone(),
            None => return,
        };
        title(ui, &d.name, &format!("{} · {} · catalog device", d.vendor_name, d.group_name));
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("➕ Add to configuration").color(Color32::WHITE)).fill(ACCENT))
                .clicked()
            {
                self.deferred.push(Deferred::AddDevice(k));
            }
            ui.menu_button("Open in…", |ui| {
                open_menu(ui, &mut self.open_requests, Target::Device(d.vendor_id, d.product_code, d.revision))
            });
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            kv_grid(
                ui,
                "catinfo",
                &[
                    ("Type", d.type_name.clone()),
                    ("Vendor ID", hx(d.vendor_id, 8)),
                    ("Product code", hx(d.product_code, 8)),
                    ("Revision", hx(d.revision, 8)),
                    ("Physics (ports)", d.physics.clone()),
                    ("Mailbox", d.mailbox.as_ref().map(|m| m.protocols().join(", ")).unwrap_or_else(|| "none".into())),
                    (
                        "CoE",
                        d.coe()
                            .map(|c| {
                                format!(
                                    "SDO info {}, PDO assign {}, PDO config {}, complete access {}",
                                    yn(c.sdo_info),
                                    yn(c.pdo_assign),
                                    yn(c.pdo_config),
                                    yn(c.complete_access)
                                )
                            })
                            .unwrap_or_else(|| "–".into()),
                    ),
                    (
                        "DC op modes",
                        if d.dc_opmodes.is_empty() {
                            "none".into()
                        } else {
                            d.dc_opmodes.iter().map(|m| m.name.clone()).collect::<Vec<_>>().join(", ")
                        },
                    ),
                    ("FMMUs", d.fmmus.join(", ")),
                    (
                        "EEPROM",
                        d.eeprom.as_ref().map(|e| format!("{} bytes", e.byte_size)).unwrap_or_else(|| "–".into()),
                    ),
                    ("Profile", d.profile_no.map(|p| p.to_string()).unwrap_or_else(|| "–".into())),
                    (
                        "Slots",
                        if d.slots.is_empty() {
                            "none".into()
                        } else {
                            d.slots
                                .iter()
                                .map(|s| format!("{} ({}–{})", s.name, s.min, s.max))
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                    ),
                    (
                        "Modules",
                        if d.modules.is_empty() {
                            "–".into()
                        } else {
                            d.modules.iter().map(|m| m.type_name.clone()).collect::<Vec<_>>().join(", ")
                        },
                    ),
                    ("Source", d.source_file.clone()),
                ],
            );
            ui.add_space(8.0);
            sub_header(ui, "Sync managers");
            sm_def_grid(ui, &d.sms);
            ui.add_space(8.0);
            sub_header(ui, "PDOs");
            egui::Grid::new("catpdos").striped(true).num_columns(5).show(ui, |ui| {
                for h in ["Dir", "Index", "Name", "Bits", "Flags"] {
                    ui.strong(h);
                }
                ui.end_row();
                for (dir, list) in [("Rx", &d.rx_pdos), ("Tx", &d.tx_pdos)] {
                    for p in list.iter() {
                        ui.label(dir);
                        ui.monospace(hx(p.index as u32, 4));
                        ui.label(&p.name);
                        ui.label(p.bit_len().to_string());
                        ui.label(pdo_flags(p));
                        ui.end_row();
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------ master view
    fn master_view(&mut self, ui: &mut egui::Ui, lay: &NetLayout, page: &mut MasterTab) {
        let sub = format!(
            "{} slaves · {} µs cycle · {}",
            self.project.slaves.len(),
            self.project.master.cycle_time_us,
            if self.project.master.adapter.is_empty() {
                "no adapter".to_string()
            } else {
                self.project.master.adapter.clone()
            }
        );
        title(ui, &format!("🖧 {}", self.project.master.name), &sub);
        ui.horizontal_wrapped(|ui| {
            for (t, l) in [
                (MasterTab::General, "General"),
                (MasterTab::Dc, "Distributed Clocks"),
                (MasterTab::Image, "Process Image"),
                (MasterTab::Cyclic, "Cyclic Frames"),
                (MasterTab::InitCmds, "Init Commands"),
                (MasterTab::Topology, "Topology"),
                (MasterTab::Online, "Online / Scan"),
                (MasterTab::Eni, "ENI Preview"),
            ] {
                if tab_button(ui, *page == t, l) {
                    *page = t;
                    if t == MasterTab::Eni {
                        self.eni_text = eni::export_eni(&self.project);
                    }
                }
            }
        });
        ui.separator();
        match *page {
            MasterTab::General => self.master_general(ui, lay),
            MasterTab::Dc => self.master_dc(ui),
            MasterTab::Image => self.master_image(ui, lay),
            MasterTab::Cyclic => self.master_cyclic(ui, lay),
            MasterTab::InitCmds => {
                let cmds = master_init_cmds(&self.project);
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| init_cmd_grid(ui, "minit", &cmds));
            }
            MasterTab::Topology => self.topology(ui, lay),
            MasterTab::Online => self.online(ui),
            MasterTab::Eni => {
                ui.horizontal(|ui| {
                    if ui.button("⟳ Refresh").clicked() {
                        self.eni_text = eni::export_eni(&self.project);
                    }
                    if ui.button("📋 Copy").clicked() {
                        ui.ctx().copy_text(self.eni_text.clone());
                        self.info("ENI copied to clipboard");
                    }
                    if ui.button("📤 Export…").clicked() {
                        self.export_eni_dialog();
                    }
                    ui.label(RichText::new(format!("{} lines", self.eni_text.lines().count())).weak());
                });
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    let mut t = self.eni_text.as_str();
                    ui.add(egui::TextEdit::multiline(&mut t).code_editor().desired_width(f32::INFINITY));
                });
            }
        }
    }

    fn master_general(&mut self, ui: &mut egui::Ui, lay: &NetLayout) {
        let adapters = self.adapters.clone();
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let m = &mut self.project.master;
            sub_header(ui, "Master");
            egui::Grid::new("mgen").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Name");
                changed |= ui.text_edit_singleline(&mut m.name).changed();
                ui.end_row();
                ui.label("Network adapter");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("adapter")
                        .selected_text(if m.adapter.is_empty() { "(none)".to_string() } else { m.adapter.clone() })
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (a, mac) in adapters.iter() {
                                if ui.selectable_label(m.adapter == *a, format!("{}  {}", a, mac)).clicked() {
                                    m.adapter = a.clone();
                                    if !mac.is_empty() {
                                        m.src_mac = mac.clone();
                                    }
                                    changed = true;
                                }
                            }
                        });
                });
                ui.end_row();
                ui.label("Source MAC");
                changed |= ui.text_edit_singleline(&mut m.src_mac).changed();
                ui.end_row();
                ui.label("Destination MAC");
                changed |= ui.text_edit_singleline(&mut m.dst_mac).changed();
                ui.end_row();
                ui.label("EtherType");
                changed |= ui
                    .add(egui::DragValue::new(&mut m.ether_type).hexadecimal(4, false, true).prefix("0x"))
                    .changed();
                ui.end_row();
            });
            ui.add_space(8.0);
            sub_header(ui, "Cyclic process data");
            egui::Grid::new("mcyc").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Cycle time");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut m.cycle_time_us)
                            .range(50..=1_000_000)
                            .clamp_existing_to_range(false)
                            .suffix(" µs")
                            .speed(10),
                    )
                    .changed();
                ui.end_row();
                ui.label("Frame commands");
                ui.horizontal(|ui| {
                    changed |= ui.radio_value(&mut m.use_lrw, true, "LRW (read/write combined)").changed();
                    changed |= ui.radio_value(&mut m.use_lrw, false, "LRD + LWR").changed();
                });
                ui.end_row();
                ui.label("Logical start address");
                changed |= ui
                    .add(egui::DragValue::new(&mut m.log_start).hexadecimal(8, false, true).prefix("0x"))
                    .changed();
                ui.end_row();
                ui.label("Max. data per frame");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut m.max_frame_data)
                            .range(16..=1486)
                            .clamp_existing_to_range(false)
                            .suffix(" bytes"),
                    )
                    .changed();
                ui.end_row();
            });
            ui.add_space(8.0);
            sub_header(ui, "State machine timeouts");
            egui::Grid::new("mto").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                for (l, v) in [
                    ("INIT to PREOP", &mut m.timeout_init_preop_ms),
                    ("PREOP to SAFEOP", &mut m.timeout_preop_safeop_ms),
                    ("SAFEOP to OP", &mut m.timeout_safeop_op_ms),
                    ("Mailbox (SDO)", &mut m.timeout_mailbox_ms),
                ] {
                    ui.label(l);
                    changed |= ui
                        .add(egui::DragValue::new(v).range(10..=600_000).clamp_existing_to_range(false).suffix(" ms"))
                        .changed();
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            sub_header(ui, "Summary");
            kv_grid(
                ui,
                "msum",
                &[
                    ("Output image", format!("{} bytes at {}", lay.out_size, hx(lay.out_log, 8))),
                    ("Input image", format!("{} bytes at {}", lay.in_size, hx(lay.in_log, 8))),
                    ("Slaves with outputs / inputs", format!("{} / {}", lay.n_out_slaves, lay.n_in_slaves)),
                    ("Expected working counter", format!("{}", process_data_wkc(&cyclic_frames(&self.project, lay)))),
                    ("Process variables", format!("{}", lay.vars.len())),
                ],
            );
        });
        if changed {
            self.dirty = true;
            self.changed = true;
        }
    }

    fn master_dc(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.project;
        let mut changed = false;
        changed |= ui.checkbox(&mut p.master.dc_enabled, "Use distributed clocks").changed();
        let refc = p.ref_clock_index();
        ui.horizontal(|ui| {
            ui.label("Reference clock");
            let txt = refc
                .map(|i| format!("{} {}", p.slaves[i].phys_addr, p.slaves[i].name))
                .unwrap_or_else(|| "(none)".into());
            egui::ComboBox::from_id_salt("refclk").selected_text(txt).width(300.0).show_ui(ui, |ui| {
                if ui
                    .selectable_label(p.master.ref_clock.is_none(), "Automatic (first DC capable slave)")
                    .clicked()
                {
                    p.master.ref_clock = None;
                    changed = true;
                }
                for (i, s) in p.slaves.iter().enumerate() {
                    if !s.device.dc_opmodes.is_empty()
                        && ui
                            .selectable_label(p.master.ref_clock == Some(i), format!("{} {}", s.phys_addr, s.name))
                            .clicked()
                    {
                        p.master.ref_clock = Some(i);
                        changed = true;
                    }
                }
            });
        });
        ui.add_space(6.0);
        sub_header(ui, "DC capable slaves");
        let cyc = p.master.cycle_time_us;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("dcs").striped(true).num_columns(7).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["Addr", "Name", "Operation mode", "AssignActivate", "SYNC0 cycle", "SYNC0 shift", "SYNC1 cycle"] {
                    ui.strong(h);
                }
                ui.end_row();
                for (i, s) in p.slaves.iter_mut().enumerate() {
                    if s.device.dc_opmodes.is_empty() {
                        continue;
                    }
                    ui.label(s.phys_addr.to_string());
                    ui.label(format!("{}{}", s.name, if refc == Some(i) { "  ⏱ ref" } else { "" }));
                    changed |= dc_mode_combo(ui, ("dcm", i), s);
                    ui.monospace(hx(s.dc_opmode().map(|m| m.assign_activate as u32).unwrap_or(0), 4));
                    ui.label(format!("{} ns", s.sync0_cycle(cyc)));
                    ui.label(format!("{} ns", s.sync0_shift_ns));
                    ui.label(format!("{} ns", s.sync1_cycle(cyc)));
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "The DC start time and the propagation delays are measured by the master at start-up; the ENI carries cycle, shift and activation values.",
                )
                .weak(),
            );
        });
        if changed {
            self.dirty = true;
            self.changed = true;
        }
    }

    fn master_image(&mut self, ui: &mut egui::Ui, lay: &NetLayout) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "Outputs: {} bytes   Inputs: {} bytes (inputs start at byte {})",
                lay.out_size, lay.in_size, lay.out_size
            ));
            if ui.button("Export C header…").clicked() {
                let t = export::c_header(&self.project, lay);
                self.save_text("Export C header", "ecat_process_image.h", "h", t.as_bytes());
            }
            if ui.button("Export CSV…").clicked() {
                let t = export::csv(lay);
                self.save_text("Export CSV", "process_image.csv", "csv", t.as_bytes());
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            var_grid(ui, "pimg", lay.vars.iter().collect(), lay.out_log);
        });
    }

    fn master_cyclic(&mut self, ui: &mut egui::Ui, lay: &NetLayout) {
        let p = &self.project;
        let frames = cyclic_frames(p, lay);
        let wire: f32 = frames.iter().map(|f| f.wire_us()).sum();
        ui.label(format!(
            "{} frame(s) per {} µs cycle · {:.0} µs on the wire ({:.0}% bus load) · up to {} data bytes per frame (change on the General page)",
            frames.len(),
            p.master.cycle_time_us,
            wire,
            if p.master.cycle_time_us > 0 {
                wire * 100.0 / p.master.cycle_time_us as f32
            } else {
                0.0
            },
            p.master.max_frame_data
        ));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("cycfr")
            .max_height(ui.available_height() * 0.55)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (fi, f) in frames.iter().enumerate() {
                    sub_header(ui, &format!("Frame {} — {} of {} bytes", fi + 1, f.bytes(), ETH_PAYLOAD));
                    egui::Grid::new(("cyc", fi)).striped(true).num_columns(7).spacing([16.0, 4.0]).show(ui, |ui| {
                        for h in ["Cmd", "Address", "Length", "Expected WKC", "Input offs", "Output offs", "Comment"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for c in f.cmds.iter() {
                            ui.monospace(cmd_name(c.cmd));
                            if c.cmd == CMD_BRD {
                                ui.monospace(format!("{:#06x}:{:#06x}", c.addr & 0xFFFF, c.addr >> 16));
                            } else {
                                ui.monospace(hx(c.addr, 8));
                            }
                            ui.label(c.len.to_string());
                            ui.label(c.wkc.to_string());
                            ui.label(c.in_offs.to_string());
                            ui.label(if c.cmd == CMD_BRD { "–".to_string() } else { c.out_offs.to_string() });
                            ui.label(&c.comment);
                            ui.end_row();
                        }
                    });
                    ui.add(
                        egui::ProgressBar::new((f.bytes() as f32 / ETH_PAYLOAD as f32).min(1.0))
                            .text(format!("{} of {} bytes", f.bytes(), ETH_PAYLOAD))
                            .desired_width(400.0),
                    );
                    ui.add_space(8.0);
                }
            });
        ui.add_space(6.0);
        sub_header(ui, "Logical address map");
        egui::Grid::new("fmap").striped(true).num_columns(6).spacing([16.0, 4.0]).show(ui, |ui| {
            for h in ["Slave", "Direction", "SM", "Logical", "Length", "Physical"] {
                ui.strong(h);
            }
            ui.end_row();
            for (i, sl) in lay.slaves.iter().enumerate() {
                for f in sl.fmmus.iter() {
                    ui.label(format!("{} {}", p.slaves[i].phys_addr, p.slaves[i].name));
                    ui.label(if f.dir == 2 { "Outputs" } else { "Inputs" });
                    ui.label(f.sm.to_string());
                    ui.monospace(hx(f.log_start, 8));
                    ui.label(f.len.to_string());
                    ui.monospace(hx(f.phys_start as u32, 4));
                    ui.end_row();
                }
            }
        });
    }

    fn topology(&mut self, ui: &mut egui::Ui, lay: &NetLayout) {
        let p = &self.project;
        let n = p.slaves.len();
        if lay.slaves.len() != n {
            return;
        }
        let mut depth = vec![0usize; n];
        for i in 0..n {
            depth[i] = match p.slaves[i].parent {
                Some(par) if par < i => depth[par] + if p.slaves[i].parent_port >= 2 { 1 } else { 0 },
                _ => 0,
            };
        }
        let row_h = 46.0;
        let box_w = 250.0;
        let ind = 60.0;
        let maxd = depth.iter().copied().max().unwrap_or(0) as f32;
        let mut clicked = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let size = egui::vec2(40.0 + box_w + ind * (maxd + 1.0) + 260.0, 20.0 + row_h * (n as f32 + 1.0) + 20.0);
            let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
            let o = resp.rect.min + egui::vec2(20.0, 10.0);
            let rect_of = |i: Option<usize>| -> egui::Rect {
                match i {
                    None => egui::Rect::from_min_size(o, egui::vec2(box_w, 34.0)),
                    Some(i) => egui::Rect::from_min_size(
                        o + egui::vec2(ind * 0.5 + ind * depth[i] as f32, row_h * (i as f32 + 1.0)),
                        egui::vec2(box_w, 34.0),
                    ),
                }
            };
            let line = Stroke::new(1.5, Color32::from_rgb(110, 125, 145));
            for i in 0..n {
                let r = rect_of(Some(i));
                let pr = rect_of(p.slaves[i].parent);
                let x = pr.left() + 14.0 + if p.slaves[i].parent_port >= 2 { 20.0 } else { 0.0 };
                painter.line_segment([egui::pos2(x, pr.bottom()), egui::pos2(x, r.center().y)], line);
                painter.line_segment([egui::pos2(x, r.center().y), egui::pos2(r.left(), r.center().y)], line);
                painter.text(
                    egui::pos2(x + 3.0, r.center().y - 9.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{}", port_letter(p.slaves[i].parent_port)),
                    egui::FontId::proportional(10.0),
                    Color32::GRAY,
                );
            }
            let draw = |r: egui::Rect, t1: String, t2: String, fill: Color32, sel: bool| {
                painter.rect_filled(r, 4.0, fill);
                painter.rect_stroke(
                    r,
                    4.0,
                    Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { ACCENT } else { Color32::from_gray(150) }),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    r.left_top() + egui::vec2(8.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    t1,
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(20),
                );
                painter.text(
                    r.left_top() + egui::vec2(8.0, 19.0),
                    egui::Align2::LEFT_TOP,
                    t2,
                    egui::FontId::proportional(10.5),
                    Color32::from_gray(90),
                );
            };
            draw(
                rect_of(None),
                format!("🖧 {}", p.master.name),
                format!(
                    "{}  ·  {} µs",
                    if p.master.adapter.is_empty() { "no adapter" } else { &p.master.adapter },
                    p.master.cycle_time_us
                ),
                Color32::from_rgb(214, 228, 245),
                self.sel == Sel::Master,
            );
            let refc = p.ref_clock_index();
            for i in 0..n {
                let s = &p.slaves[i];
                let sl = &lay.slaves[i];
                let r = rect_of(Some(i));
                let fill = if s.dc_active() { Color32::from_rgb(232, 244, 234) } else { Color32::WHITE };
                draw(
                    r,
                    format!("{}  {}{}", s.phys_addr, s.device.type_name, if refc == Some(i) { "  ⏱" } else { "" }),
                    format!("out {} B · in {} B · {}", sl.out_bytes, sl.in_bytes, s.device.physics),
                    fill,
                    self.sel == Sel::Slave(i),
                );
                painter.text(
                    r.right_center() + egui::vec2(10.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &s.name,
                    egui::FontId::proportional(12.0),
                    Color32::from_gray(70),
                );
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if r.contains(pos) {
                            clicked = Some(i);
                        }
                    }
                }
            }
        });
        if let Some(i) = clicked {
            self.sel = Sel::Slave(i);
        }
    }

    fn online(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Adapter");
            let adapters = self.adapters.clone();
            let m = &mut self.project.master;
            egui::ComboBox::from_id_salt("adapter2")
                .selected_text(if m.adapter.is_empty() { "(none)".to_string() } else { m.adapter.clone() })
                .width(200.0)
                .show_ui(ui, |ui| {
                    for (a, mac) in adapters.iter() {
                        if ui.selectable_label(m.adapter == *a, format!("{}  {}", a, mac)).clicked() {
                            m.adapter = a.clone();
                        }
                    }
                });
            if ui.button("⟳").on_hover_text("Refresh adapter list").clicked() {
                self.adapters = scan::list_adapters();
            }
            let scanning = self.scan_rx.is_some();
            if ui
                .add_enabled(
                    !scanning,
                    egui::Button::new(RichText::new(if scanning { "⏳ Scanning…" } else { "🔍 Scan bus" }).color(Color32::WHITE)).fill(ACCENT),
                )
                .clicked()
            {
                self.start_scan();
            }
            if ui
                .add_enabled(!self.scanned.is_empty(), egui::Button::new("Create configuration from scan"))
                .clicked()
            {
                self.deferred.push(Deferred::ConfigFromScan);
            }
            if ui.add_enabled(!self.scanned.is_empty(), egui::Button::new("Copy serial numbers")).clicked() {
                let sc = self.scanned.clone();
                for (s, x) in self.project.slaves.iter_mut().zip(sc.iter()) {
                    s.serial = x.serial;
                }
                self.dirty = true;
                self.changed = true;
                self.info("Serial numbers taken from scanned slaves");
            }
        });
        ui.label(RichText::new("The scan sends raw EtherCAT frames on the selected adapter; it needs root rights or the CAP_NET_RAW capability.").weak());
        ui.add_space(6.0);
        sub_header(ui, "Scanned slaves compared with the configuration");
        if self.scanned.is_empty() {
            ui.label(RichText::new("No scan result yet.").italics().weak());
            return;
        }
        let n = self.scanned.len().max(self.project.slaves.len());
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("scan").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["Pos", "Vendor", "Product", "Revision", "Serial", "State", "ESI match", "Configuration"] {
                    ui.strong(h);
                }
                ui.end_row();
                for i in 0..n {
                    let sc = self.scanned.get(i);
                    let cf = self.project.slaves.get(i);
                    ui.label(i.to_string());
                    match sc {
                        Some(s) => {
                            ui.monospace(hx(s.vendor, 8));
                            ui.monospace(hx(s.product, 8));
                            ui.monospace(hx(s.revision, 8));
                            ui.label(s.serial.to_string());
                            ui.label(scan::al_state_name(s.al_status));
                            ui.label(
                                self.lib
                                    .find_best(s.vendor, s.product, s.revision)
                                    .map(|d| d.type_name.clone())
                                    .unwrap_or_else(|| "–".into()),
                            );
                        }
                        None => {
                            for _ in 0..6 {
                                ui.label("–");
                            }
                        }
                    }
                    let (txt, col) = match (sc, cf) {
                        (Some(s), Some(c)) if s.vendor == c.device.vendor_id && s.product == c.device.product_code && s.revision == c.device.revision => {
                            ("✔ matches".to_string(), OK_GREEN)
                        }
                        (Some(s), Some(c)) if s.vendor == c.device.vendor_id && s.product == c.device.product_code => {
                            (format!("⚠ revision differs ({})", c.device.type_name), WARN)
                        }
                        (Some(_), Some(c)) => (format!("❌ configured: {}", c.device.type_name), ERR),
                        (Some(_), None) => ("⚠ not configured".to_string(), WARN),
                        (None, Some(c)) => (format!("❌ missing: {}", c.device.type_name), ERR),
                        _ => ("".into(), Color32::GRAY),
                    };
                    ui.label(RichText::new(txt).color(col));
                    ui.end_row();
                }
            });
        });
    }

    // ------------------------------------------------------------ slave view
    fn slave_view(&mut self, ui: &mut egui::Ui, i: usize, lay: &NetLayout, page: &mut SlaveTab) {
        if i >= self.project.slaves.len() || i >= lay.slaves.len() {
            return;
        }
        let s = &self.project.slaves[i];
        title(
            ui,
            &format!("{}  {}", s.phys_addr, s.name),
            &format!(
                "{} · {} · position {} (auto-inc {:#06x})",
                s.device.name,
                s.device.vendor_name,
                i,
                Project::auto_inc(i)
            ),
        );
        ui.horizontal_wrapped(|ui| {
            let has_mbx = s.has_mailbox();
            let has_dc = !s.device.dc_opmodes.is_empty();
            let has_slots = !s.device.slots.is_empty();
            let has_dict = !s.device.dictionary.is_empty();
            for (t, l, en) in [
                (SlaveTab::General, "General", true),
                (SlaveTab::Modules, "Modules", has_slots),
                (SlaveTab::ProcessData, "Process Data", true),
                (SlaveTab::Mailbox, "Mailbox / CoE", has_mbx),
                (SlaveTab::Dc, "Distributed Clocks", has_dc),
                (SlaveTab::InitCmds, "Init Commands", true),
                (SlaveTab::Eeprom, "EEPROM (SII)", true),
                (SlaveTab::Esc, "ESC Registers", true),
                (SlaveTab::Dictionary, "Object Dictionary", has_dict),
            ] {
                if tab_button_en(ui, en, *page == t, l) {
                    *page = t;
                }
            }
        });
        if (*page == SlaveTab::Mailbox && !s.has_mailbox())
            || (*page == SlaveTab::Dc && s.device.dc_opmodes.is_empty())
            || (*page == SlaveTab::Modules && s.device.slots.is_empty())
            || (*page == SlaveTab::Dictionary && s.device.dictionary.is_empty())
        {
            *page = SlaveTab::General;
        }
        ui.separator();
        match *page {
            SlaveTab::General => self.slave_general(ui, i),
            SlaveTab::Modules => self.slave_modules(ui, i),
            SlaveTab::Dictionary => self.slave_dictionary(ui, i, page),
            SlaveTab::ProcessData => self.slave_pd(ui, i, lay),
            SlaveTab::Mailbox => self.slave_mailbox(ui, i),
            SlaveTab::Dc => self.slave_dc(ui, i),
            SlaveTab::InitCmds => {
                let cmds = slave_init_cmds(&self.project, lay, i);
                let coe = slave_coe_cmds(&self.project, i);
                let dict = self.project.slaves[i].device.dictionary.clone();
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    sub_header(ui, &format!("EtherCAT init commands ({})", cmds.len()));
                    init_cmd_grid(ui, "sinit", &cmds);
                    if !coe.is_empty() {
                        ui.add_space(8.0);
                        sub_header(ui, &format!("CoE init commands ({})", coe.len()));
                        coe_grid_ro(ui, "coero", &coe, &dict);
                    }
                });
            }
            SlaveTab::Eeprom => self.slave_eeprom(ui, i),
            SlaveTab::Esc => self.slave_esc(ui, i, lay),
        }
    }

    /// The device's CoE object dictionary, with a shortcut to set start-up values.
    fn slave_dictionary(&mut self, ui: &mut egui::Ui, i: usize, page: &mut SlaveTab) {
        let dict = self.project.slaves[i].device.dictionary.clone();
        let has_coe = self.project.slaves[i].device.coe().is_some();
        ui.horizontal(|ui| {
            ui.label("🔎");
            ui.add(
                egui::TextEdit::singleline(&mut self.od_filter)
                    .hint_text("Filter by index or name")
                    .desired_width(280.0),
            );
            ui.label(RichText::new(format!("{} objects", dict.len())).weak());
        });
        let q = self.od_filter.to_lowercase();
        let mut chosen: Option<(u16, u8, String, String)> = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("odgrid").striped(true).num_columns(8).spacing([12.0, 3.0]).show(ui, |ui| {
                for h in ["Index:Sub", "Name", "Type", "Bits", "Access", "PDO", "Default", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for o in dict.iter() {
                    let hit = q.is_empty()
                        || format!("{:04x}", o.index).contains(&q)
                        || o.name.to_lowercase().contains(&q)
                        || o.entries.iter().any(|e| e.name.to_lowercase().contains(&q));
                    if !hit {
                        continue;
                    }
                    if !o.is_simple() {
                        ui.monospace(RichText::new(format!("{:04X}", o.index)).strong());
                        ui.label(RichText::new(&o.name).strong());
                        ui.label(&o.data_type);
                        for _ in 0..5 {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                    for e in o.entries.iter() {
                        if o.is_simple() {
                            ui.monospace(format!("{:04X}:{:02X}", o.index, e.subindex));
                            ui.label(&o.name);
                        } else {
                            ui.monospace(format!("    :{:02X}", e.subindex));
                            ui.label(format!("    {}", e.name));
                        }
                        ui.label(&e.data_type);
                        ui.label(e.bit_size.to_string());
                        ui.label(if e.access.is_empty() { "–".to_string() } else { e.access.clone() });
                        ui.label(if e.pdo_mapping.is_empty() { "–".to_string() } else { e.pdo_mapping.clone() });
                        ui.monospace(od::decode_value(&e.default, &e.data_type));
                        if has_coe && e.writable() && e.subindex != 0 || has_coe && e.writable() && o.is_simple() {
                            if ui
                                .small_button("Set at start-up…")
                                .on_hover_text("Prepare a start-up SDO for this entry on the Mailbox / CoE page")
                                .clicked()
                            {
                                chosen = Some((o.index, e.subindex, o.entry_name(e), od::decode_value(&e.default, &e.data_type)));
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                }
            });
        });
        if let Some((index, sub, name, value)) = chosen {
            self.coe_form = CoeForm {
                value,
                transition: "PS".into(),
                index: format!("{:04X}", index),
                sub: sub.to_string(),
                comment: name,
                ..Default::default()
            };
            *page = SlaveTab::Mailbox;
            self.coe_scroll = true;
        }
    }

    /// Slot configuration of a modular device.
    fn slave_modules(&mut self, ui: &mut egui::Ui, i: usize) {
        let dev = self.project.slaves[i].device.clone();
        let mut mods = self.project.slaves[i].modules.clone();
        let mut changed = false;
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mod_name = |id: u32| {
            dev.modules
                .iter()
                .find(|m| m.ident == id)
                .map(|m| format!("{}  {}", m.type_name, short_name(&m.name, &m.type_name)))
                .unwrap_or_else(|| format!("unknown {:#010x}", id))
        };
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            sub_header(ui, &format!("Plugged modules ({} configured)", mods.len()));
            if mods.is_empty() {
                ui.label(RichText::new("No modules yet. Add them below in the order they are plugged into the device.").italics().weak());
            } else {
                egui::Grid::new("mods").striped(true).num_columns(6).spacing([14.0, 4.0]).show(ui, |ui| {
                    for h in ["Position", "Slot", "Module", "Ident", "PDOs", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    let n = mods.len();
                    for (k, m) in mods.iter_mut().enumerate() {
                        let sd = dev.slots.get(m.slot);
                        ui.label(format!("{}", k + 1));
                        ui.label(sd.map(|s| s.name.clone()).unwrap_or_else(|| "?".into()));
                        egui::ComboBox::from_id_salt(("modsel", k)).selected_text(mod_name(m.ident)).width(300.0).show_ui(ui, |ui| {
                            for md in dev.modules.iter().filter(|md| sd.map(|s| s.accepts(md)).unwrap_or(false)) {
                                if ui.selectable_label(m.ident == md.ident, format!("{}  {}", md.type_name, short_name(&md.name, &md.type_name))).clicked() {
                                    m.ident = md.ident;
                                    changed = true;
                                }
                            }
                        });
                        ui.monospace(hx(m.ident, 8));
                        let np = dev.modules.iter().find(|md| md.ident == m.ident).map(|md| md.rx_pdos.len() + md.tx_pdos.len()).unwrap_or(0);
                        ui.label(np.to_string());
                        ui.horizontal(|ui| {
                            if k > 0 && ui.small_button("⬆").on_hover_text("Move to the previous position").clicked() {
                                swap = Some((k, k - 1));
                            }
                            if k + 1 < n && ui.small_button("⬇").on_hover_text("Move to the next position").clicked() {
                                swap = Some((k, k + 1));
                            }
                            if ui.small_button("🗑").on_hover_text("Remove this module").clicked() {
                                remove = Some(k);
                            }
                        });
                        ui.end_row();
                    }
                });
            }
            ui.add_space(8.0);
            sub_header(ui, "Add a module");
            for (si, sd) in dev.slots.iter().enumerate() {
                let count = mods.iter().filter(|m| m.slot == si).count() as u32;
                ui.horizontal(|ui| {
                    ui.label(format!("{}  ({} of {} used, at least {})", sd.name, count, sd.max, sd.min));
                    ui.add_enabled_ui(count < sd.max, |ui| {
                        egui::ComboBox::from_id_salt(("modadd", si)).selected_text("➕ Add…").width(300.0).show_ui(ui, |ui| {
                            for md in dev.modules.iter().filter(|md| sd.accepts(md)) {
                                if ui.selectable_label(false, format!("{}  {}", md.type_name, short_name(&md.name, &md.type_name))).clicked() {
                                    // keep modules of the same slot type together
                                    let at = mods.iter().rposition(|m| m.slot <= si).map(|p| p + 1).unwrap_or(0);
                                    mods.insert(at, ModuleInst { slot: si, ident: md.ident });
                                    changed = true;
                                }
                            }
                        });
                    });
                });
            }
            ui.add_space(8.0);
            let sd0 = dev.slots.first();
            ui.label(
                RichText::new(format!(
                    "Each position shifts the module's PDO indices by {} and its object indices (0x6000–0xAFFF) by {:#x}. The module list is downloaded to object 0xF030 in the PREOP to SAFEOP transition, together with the modules' start-up parameters.",
                    sd0.map(|s| s.pdo_inc).unwrap_or(0),
                    sd0.map(|s| s.index_inc).unwrap_or(0)
                ))
                .small()
                .weak(),
            );
        });
        if let Some(k) = remove {
            mods.remove(k);
            changed = true;
        }
        if let Some((a, b)) = swap {
            mods.swap(a, b);
            changed = true;
        }
        if changed {
            self.project.slaves[i].set_modules(mods);
            self.sel_pdo = None;
            self.dirty = true;
            self.changed = true;
        }
    }

    fn slave_general(&mut self, ui: &mut egui::Ui, i: usize) {
        let wiring_before = (self.project.slaves[i].parent, self.project.slaves[i].parent_port);
        let names: Vec<(u16, String, usize)> = self
            .project
            .slaves
            .iter()
            .map(|s| (s.phys_addr, s.name.clone(), s.device.port_count()))
            .collect();
        let mut changed = false;
        let s = &mut self.project.slaves[i];
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            sub_header(ui, "Slave");
            egui::Grid::new("sgen").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Name");
                changed |= ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(320.0)).changed();
                ui.end_row();
                ui.label("Station address");
                changed |= ui
                    .add(egui::DragValue::new(&mut s.phys_addr).range(1..=65535).clamp_existing_to_range(false))
                    .changed();
                ui.end_row();
                ui.label("Auto-increment address");
                ui.monospace(format!("{:#06x} ({})", Project::auto_inc(i), -(i as i32)));
                ui.end_row();
                ui.label("Serial number");
                changed |= ui.add(egui::DragValue::new(&mut s.serial)).changed();
                ui.end_row();
                ui.label("Connected to");
                ui.horizontal(|ui| {
                    let cur = match s.parent {
                        None => "Master".to_string(),
                        Some(p) => format!("{} {}", names[p].0, names[p].1),
                    };
                    egui::ComboBox::from_id_salt("parent").selected_text(cur).width(240.0).show_ui(ui, |ui| {
                        if ui.selectable_label(s.parent.is_none(), "Master").clicked() {
                            s.parent = None;
                            changed = true;
                        }
                        for (k, n) in names.iter().enumerate().take(i) {
                            if ui.selectable_label(s.parent == Some(k), format!("{} {}", n.0, n.1)).clicked() {
                                s.parent = Some(k);
                                changed = true;
                            }
                        }
                    });
                    ui.label("port");
                    let ports = s.parent.map(|p| names[p].2).unwrap_or(2).max(2);
                    egui::ComboBox::from_id_salt("pport")
                        .selected_text(port_letter(s.parent_port).to_string())
                        .width(50.0)
                        .show_ui(ui, |ui| {
                            for k in 1..ports.max(2) as u8 {
                                changed |= ui.selectable_value(&mut s.parent_port, k, port_letter(k).to_string()).changed();
                            }
                        });
                });
                ui.end_row();
            });
            ui.add_space(8.0);
            sub_header(ui, "Identity check at start-up");
            changed |= ui.checkbox(&mut s.check_vendor, "Check vendor ID").changed();
            changed |= ui.checkbox(&mut s.check_product, "Check product code").changed();
            ui.horizontal(|ui| {
                ui.label("Check revision");
                changed |= ui.radio_value(&mut s.check_revision, RevCheck::None, "no").changed();
                changed |= ui.radio_value(&mut s.check_revision, RevCheck::Equal, "equal").changed();
                changed |= ui.radio_value(&mut s.check_revision, RevCheck::GreaterOrEqual, "equal or higher").changed();
            });
            changed |= ui.checkbox(&mut s.check_serial, "Check serial number").changed();
            ui.add_space(8.0);
            sub_header(ui, "Watchdog");
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut s.watchdog_enabled, "Set process data watchdog").changed();
                ui.add_enabled_ui(s.watchdog_enabled, |ui| {
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut s.watchdog_ms)
                                .range(1..=6000)
                                .clamp_existing_to_range(false)
                                .suffix(" ms"),
                        )
                        .changed();
                });
            });
            ui.add_space(8.0);
            sub_header(ui, "Device");
            let d = &s.device;
            kv_grid(
                ui,
                "sdev",
                &[
                    ("Type", d.type_name.clone()),
                    ("Description", d.name.clone()),
                    ("Vendor", format!("{} ({})", d.vendor_name, hx(d.vendor_id, 8))),
                    ("Product code", hx(d.product_code, 8)),
                    ("Revision", hx(d.revision, 8)),
                    ("Group", d.group_name.clone()),
                    ("Physics (ports)", d.physics.clone()),
                    ("Mailbox", d.mailbox.as_ref().map(|m| m.protocols().join(", ")).unwrap_or_else(|| "none".into())),
                    ("ESI source", d.source_file.clone()),
                ],
            );
        });
        if (self.project.slaves[i].parent, self.project.slaves[i].parent_port) != wiring_before {
            // keep the list in bus order; done between frames so open views stay on their items
            self.deferred.push(Deferred::SortTopology);
        }
        if changed {
            self.dirty = true;
            self.changed = true;
        }
    }

    fn slave_pd(&mut self, ui: &mut egui::Ui, i: usize, lay: &NetLayout) {
        let sl = lay.slaves[i].clone();
        let mut changed = false;
        let can_assign;
        let can_config;
        {
            let s = &self.project.slaves[i];
            can_assign = s.device.coe().map(|c| c.pdo_assign).unwrap_or(false);
            can_config = s.device.coe().map(|c| c.pdo_config).unwrap_or(false);
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            sub_header(ui, "Sync managers");
            egui::Grid::new("sms").striped(true).num_columns(7).spacing([16.0, 4.0]).show(ui, |ui| {
                for h in ["SM", "Type", "Start", "Size", "Control", "Enable", "PDOs"] {
                    ui.strong(h);
                }
                ui.end_row();
                for sm in sl.sms.iter() {
                    ui.label(sm.index.to_string());
                    ui.label(sm.kind.text());
                    ui.monospace(hx(sm.start as u32, 4));
                    ui.label(sm.len.to_string());
                    ui.monospace(hx(sm.control as u32, 2));
                    ui.label(yn(sm.enable && sm.len > 0));
                    ui.monospace(sm.pdos.iter().map(|p| hx(*p as u32, 4)).collect::<Vec<_>>().join(" "));
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            let s = &mut self.project.slaves[i];
            ui.horizontal(|ui| {
                if !can_assign {
                    ui.label(RichText::new("ℹ PDO assignment is fixed by the device (no CoE PDO assignment).").color(WARN));
                } else {
                    changed |= ui
                        .checkbox(&mut s.generate_pdo_cmds, "Download PDO assignment/mapping at start-up (CoE)")
                        .changed();
                }
            });
            let sm_list: Vec<(u8, SmKind)> = s.device.sms.iter().enumerate().map(|(k, x)| (k as u8, x.kind)).collect();
            // side by side when there is room, stacked in narrow panes
            let narrow = ui.available_width() < 900.0;
            let mut pdo_list = |ui: &mut egui::Ui, is_tx: bool| {
                sub_header(
                    ui,
                    if is_tx {
                        "Inputs — TxPDO (slave to master)"
                    } else {
                        "Outputs — RxPDO (master to slave)"
                    },
                );
                egui::Grid::new(("pdos", is_tx))
                    .striped(true)
                    .num_columns(5)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for h in ["", "Index", "Name", "SM", "Bits"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        let n = s.pdos.len();
                        for k in 0..n {
                            if s.pdos[k].is_tx != is_tx {
                                continue;
                            }
                            let mut on = s.pdos[k].assigned;
                            let locked = s.pdos[k].pdo.mandatory || !can_assign;
                            let r = ui.add_enabled(!locked || !on, egui::Checkbox::without_text(&mut on));
                            if r.changed() {
                                toggle_pdo(s, k, on);
                                changed = true;
                            }
                            let pc = &s.pdos[k];
                            ui.monospace(hx(pc.pdo.index as u32, 4));
                            let lbl = ui.selectable_label(self.sel_pdo == Some(k), format!("{}{}", pc.pdo.name, pdo_flag_icons(&pc.pdo)));
                            let tip = format!(
                                "{}\n{}",
                                pdo_flags(&pc.pdo),
                                if pc.pdo.excludes.is_empty() {
                                    String::new()
                                } else {
                                    format!("Excludes {}", pc.pdo.excludes.iter().map(|x| hx(*x as u32, 4)).collect::<Vec<_>>().join(", "))
                                }
                            );
                            if lbl.on_hover_text(tip).clicked() {
                                self.sel_pdo = Some(k);
                            }
                            let cur_sm = s.pdos[k].sm;
                            egui::ComboBox::from_id_salt(("pdosm", k))
                                .selected_text(cur_sm.to_string())
                                .width(40.0)
                                .show_ui(ui, |ui| {
                                    for (smi, kind) in sm_list.iter() {
                                        let fits = if is_tx { *kind == SmKind::Inputs } else { *kind == SmKind::Outputs };
                                        if fits && ui.selectable_label(cur_sm == *smi, smi.to_string()).clicked() {
                                            s.pdos[k].sm = *smi;
                                            changed = true;
                                        }
                                    }
                                });
                            ui.label(s.pdos[k].pdo.bit_len().to_string());
                            ui.end_row();
                        }
                    });
                if can_config && ui.small_button("➕ New PDO").clicked() {
                    let base: u16 = if is_tx { 0x1A00 } else { 0x1600 };
                    let mut idx = base;
                    while s.pdos.iter().any(|p| p.pdo.index == idx) {
                        idx += 1;
                    }
                    let sm = sm_list
                        .iter()
                        .find(|(_, kd)| *kd == if is_tx { SmKind::Inputs } else { SmKind::Outputs })
                        .map(|x| x.0)
                        .unwrap_or(0);
                    s.pdos.push(PdoCfg {
                        pdo: Pdo {
                            index: idx,
                            name: format!("User PDO {}", hx(idx as u32, 4)),
                            sm: None,
                            fixed: false,
                            mandatory: false,
                            excludes: vec![],
                            entries: vec![],
                        },
                        is_tx,
                        assigned: false,
                        sm,
                    });
                    self.sel_pdo = Some(s.pdos.len() - 1);
                    changed = true;
                }
            };
            if narrow {
                pdo_list(ui, false);
                ui.add_space(6.0);
                pdo_list(ui, true);
            } else {
                ui.columns(2, |cols| {
                    let (a, b) = cols.split_at_mut(1);
                    pdo_list(&mut a[0], false);
                    pdo_list(&mut b[0], true);
                });
            }
            ui.add_space(8.0);
            // PDO entries editor
            if let Some(k) = self.sel_pdo.filter(|k| *k < s.pdos.len()) {
                let editable = can_config && !s.pdos[k].pdo.fixed;
                let user_pdo = s.device.rx_pdos.iter().chain(s.device.tx_pdos.iter()).all(|p| p.index != s.pdos[k].pdo.index);
                let mut del_pdo = false;
                let od_mappable: Vec<(u16, u8, u16, String, String)> = s
                    .device
                    .dictionary
                    .iter()
                    .flat_map(|o| {
                        o.entries
                            .iter()
                            .filter(|e| e.mappable(!s.pdos[k].is_tx))
                            .map(move |e| (o.index, e.subindex, e.bit_size, o.entry_name(e), e.data_type.clone()))
                    })
                    .collect();
                let pc = &mut s.pdos[k];
                sub_header(
                    ui,
                    &format!(
                        "PDO {} — {} {}",
                        hx(pc.pdo.index as u32, 4),
                        pc.pdo.name,
                        if editable { "(editable mapping)" } else { "(fixed mapping)" }
                    ),
                );
                if editable {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        changed |= ui.text_edit_singleline(&mut pc.pdo.name).changed();
                        ui.label("Index");
                        changed |= ui
                            .add(egui::DragValue::new(&mut pc.pdo.index).hexadecimal(4, false, true).prefix("#x"))
                            .changed();
                    });
                }
                let mut del: Option<usize> = None;
                let mut up: Option<usize> = None;
                egui::Grid::new("entries").striped(true).num_columns(7).spacing([12.0, 4.0]).show(ui, |ui| {
                    for h in ["#", "Index", "Sub", "Bits", "Name", "Data type", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    let n = pc.pdo.entries.len();
                    for (ei, e) in pc.pdo.entries.iter_mut().enumerate() {
                        ui.label(ei.to_string());
                        if editable {
                            changed |= ui.add(egui::DragValue::new(&mut e.index).hexadecimal(4, false, true).prefix("#x")).changed();
                            changed |= ui.add(egui::DragValue::new(&mut e.subindex)).changed();
                            changed |= ui
                                .add(egui::DragValue::new(&mut e.bitlen).range(1..=64).clamp_existing_to_range(false))
                                .changed();
                            changed |= ui.add_sized([200.0, 18.0], egui::TextEdit::singleline(&mut e.name)).changed();
                            egui::ComboBox::from_id_salt(("dt", ei))
                                .selected_text(e.data_type.clone())
                                .width(80.0)
                                .show_ui(ui, |ui| {
                                    for t in ["BOOL", "SINT", "USINT", "INT", "UINT", "DINT", "UDINT", "REAL", "LINT", "ULINT", "LREAL"] {
                                        if ui.selectable_label(e.data_type == t, t).clicked() {
                                            e.data_type = t.to_string();
                                            if let Some(b) = datatype_bits(t) {
                                                e.bitlen = b;
                                            }
                                            changed = true;
                                        }
                                    }
                                });
                            ui.horizontal(|ui| {
                                if ei > 0 && ui.small_button("⬆").clicked() {
                                    up = Some(ei);
                                }
                                if ui.small_button("🗑").clicked() {
                                    del = Some(ei);
                                }
                            });
                        } else {
                            ui.monospace(hx(e.index as u32, 4));
                            ui.label(if e.is_gap() { "–".into() } else { e.subindex.to_string() });
                            ui.label(e.bitlen.to_string());
                            ui.label(if e.is_gap() { "(gap)".to_string() } else { e.name.clone() });
                            ui.label(&e.data_type);
                            ui.label("");
                        }
                        ui.end_row();
                    }
                    let _ = n;
                });
                if let Some(d) = del {
                    pc.pdo.entries.remove(d);
                    changed = true;
                }
                if let Some(u) = up {
                    pc.pdo.entries.swap(u, u - 1);
                    changed = true;
                }
                if editable {
                    ui.horizontal(|ui| {
                        if ui.small_button("➕ Add entry").clicked() {
                            pc.pdo.entries.push(PdoEntry {
                                index: 0x7000,
                                subindex: 1,
                                bitlen: 16,
                                name: "New entry".into(),
                                data_type: "UINT".into(),
                            });
                            changed = true;
                        }
                        if !od_mappable.is_empty() {
                            egui::ComboBox::from_id_salt("odmap")
                                .selected_text("➕ From dictionary…")
                                .width(260.0)
                                .show_ui(ui, |ui| {
                                    for (idx, sub, bits, name, dt) in od_mappable.iter() {
                                        if ui.selectable_label(false, format!("{:04X}:{:02X}  {}  ({})", idx, sub, name, dt)).clicked() {
                                            pc.pdo.entries.push(PdoEntry {
                                                index: *idx,
                                                subindex: *sub,
                                                bitlen: *bits,
                                                name: name.clone(),
                                                data_type: dt.clone(),
                                            });
                                            changed = true;
                                        }
                                    }
                                });
                        }
                        if ui.small_button("➕ Add gap").clicked() {
                            pc.pdo.entries.push(PdoEntry {
                                index: 0,
                                subindex: 0,
                                bitlen: 8,
                                name: String::new(),
                                data_type: String::new(),
                            });
                            changed = true;
                        }
                        if user_pdo && ui.small_button("🗑 Delete PDO").clicked() {
                            del_pdo = true;
                        }
                    });
                }
                if del_pdo {
                    s.pdos.remove(k);
                    self.sel_pdo = None;
                    changed = true;
                }
            } else {
                ui.label(RichText::new("Select a PDO name to see its entries.").italics().weak());
            }
            ui.add_space(8.0);
            sub_header(ui, "Variables of this slave in the process image");
            let vars: Vec<&Variable> = lay.vars.iter().filter(|v| v.slave == i).collect();
            var_grid(ui, "svars", vars, lay.out_log);
        });
        if changed {
            self.dirty = true;
            self.changed = true;
        }
    }

    fn slave_mailbox(&mut self, ui: &mut egui::Ui, i: usize) {
        let generated: Vec<CoeInitCmd> = slave_coe_cmds(&self.project, i).into_iter().filter(|c| c.generated).collect();
        let mut changed = false;
        let s = &mut self.project.slaves[i];
        let mb = s.device.mailbox.clone().unwrap_or_default();
        let dict = s.device.dictionary.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            sub_header(ui, "Mailbox");
            egui::Grid::new("mbx").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("Protocols");
                ui.label(mb.protocols().join(", "));
                ui.end_row();
                let lim = |k: SmKind| {
                    s.device
                        .sms
                        .iter()
                        .find(|x| x.kind == k)
                        .map(|x| (x.min_size.unwrap_or(34), x.max_size.unwrap_or(1024)))
                        .unwrap_or((34, 1024))
                };
                let (a0, a1) = lim(SmKind::MBoxOut);
                let (b0, b1) = lim(SmKind::MBoxIn);
                ui.label("Mailbox out size (SM0)");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut s.mbx_sizes.0)
                            .range(a0..=a1)
                            .clamp_existing_to_range(false)
                            .suffix(" bytes"),
                    )
                    .changed();
                ui.end_row();
                ui.label("Mailbox in size (SM1)");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut s.mbx_sizes.1)
                            .range(b0..=b1)
                            .clamp_existing_to_range(false)
                            .suffix(" bytes"),
                    )
                    .changed();
                ui.end_row();
                if let Some(c) = &mb.coe {
                    ui.label("CoE features");
                    ui.label(format!(
                        "SDO info {} · PDO assign {} · PDO config {} · complete access {} · segmented SDO {}",
                        yn(c.sdo_info),
                        yn(c.pdo_assign),
                        yn(c.pdo_config),
                        yn(c.complete_access),
                        yn(c.segmented_sdo)
                    ));
                    ui.end_row();
                }
            });
            if mb.eoe {
                ui.add_space(8.0);
                sub_header(ui, "EoE: IP settings sent at start-up");
                changed |= ui.checkbox(&mut s.eoe.enabled, "Set IP parameters in the PREOP to SAFEOP transition").changed();
                ui.add_enabled_ui(s.eoe.enabled, |ui| {
                    egui::Grid::new("eoe").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        for (label, field, hint) in [
                            ("MAC address", &mut s.eoe.mac, "02:00:00:00:10:01"),
                            ("IP address", &mut s.eoe.ip, "192.168.10.5"),
                            ("Subnet mask", &mut s.eoe.subnet, "255.255.255.0"),
                            ("Default gateway", &mut s.eoe.gateway, "192.168.10.1"),
                            ("DNS server", &mut s.eoe.dns, ""),
                            ("DNS name", &mut s.eoe.dns_name, "drive5"),
                        ] {
                            ui.label(label);
                            changed |= ui.add(egui::TextEdit::singleline(field).desired_width(200.0).hint_text(hint)).changed();
                            ui.end_row();
                        }
                    });
                });
                if s.eoe.enabled {
                    match crate::proto::eoe_set_ip_data(&s.eoe) {
                        Ok(d) => {
                            ui.label(
                                RichText::new(format!("Sent as EoE Set IP Parameter: {}", bytes_to_hex_spaced(&d)))
                                    .small()
                                    .monospace()
                                    .weak(),
                            );
                        }
                        Err(e) => {
                            ui.label(RichText::new(e).color(WARN));
                        }
                    }
                }
            }
            if mb.soe {
                ui.add_space(8.0);
                sub_header(ui, "SoE start-up commands (IDN writes)");
                let mut del = None;
                if s.soe_cmds.is_empty() {
                    ui.label(RichText::new("No SoE commands yet.").italics().weak());
                } else {
                    egui::Grid::new("soe").striped(true).num_columns(7).spacing([12.0, 4.0]).show(ui, |ui| {
                        for h in ["On", "Transition", "Drive", "IDN", "Data", "Comment", ""] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for (k, c) in s.soe_cmds.iter_mut().enumerate() {
                            changed |= ui.checkbox(&mut c.enabled, "").changed();
                            ui.label(&c.transition);
                            ui.label(c.drive_no.to_string());
                            ui.monospace(format!("{}  ({})", crate::proto::format_idn(c.idn), c.idn));
                            ui.monospace(bytes_to_hex_spaced(&c.data));
                            ui.label(&c.comment);
                            if ui.small_button("🗑").clicked() {
                                del = Some(k);
                            }
                            ui.end_row();
                        }
                    });
                }
                if let Some(k) = del {
                    s.soe_cmds.remove(k);
                    changed = true;
                }
                let sf = &mut self.soe_form;
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("soetr")
                        .selected_text(sf.transition.clone())
                        .width(50.0)
                        .show_ui(ui, |ui| {
                            for t in ["PS", "SO", "SP", "OP", "OS"] {
                                ui.selectable_value(&mut sf.transition, t.to_string(), t);
                            }
                        });
                    ui.label("drive");
                    ui.add(egui::DragValue::new(&mut sf.drive).range(0..=7));
                    ui.label("IDN");
                    ui.add(egui::TextEdit::singleline(&mut sf.idn).desired_width(90.0).hint_text("S-0-1000"));
                    ui.label("value");
                    ui.add(egui::TextEdit::singleline(&mut sf.value).desired_width(110.0).hint_text("10000"));
                    egui::ComboBox::from_id_salt("soesz")
                        .selected_text(match sf.size {
                            0 => "hex bytes".to_string(),
                            n => format!("{} bytes", n),
                        })
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for (n, l) in [(2u8, "2 bytes"), (4, "4 bytes"), (8, "8 bytes"), (0, "hex bytes")] {
                                ui.selectable_value(&mut sf.size, n, l);
                            }
                        });
                    ui.label("comment");
                    ui.add(egui::TextEdit::singleline(&mut sf.comment).desired_width(160.0));
                    let idn = crate::proto::parse_idn(&sf.idn);
                    let data: Option<Vec<u8>> = if sf.size == 0 {
                        Some(hex_to_bytes(&sf.value)).filter(|d| !d.is_empty())
                    } else {
                        parse_num(&sf.value).map(|v| v.to_le_bytes()[..sf.size as usize].to_vec())
                    };
                    if ui.add_enabled(idn.is_some() && data.is_some(), egui::Button::new("➕ Add")).clicked() {
                        s.soe_cmds.push(crate::proto::SoeInitCmd {
                            transition: sf.transition.clone(),
                            drive_no: sf.drive,
                            idn: idn.unwrap_or(0),
                            data: data.unwrap_or_default(),
                            comment: sf.comment.clone(),
                            enabled: true,
                        });
                        sf.idn.clear();
                        sf.value.clear();
                        sf.comment.clear();
                        changed = true;
                    }
                    if !sf.idn.trim().is_empty() && idn.is_none() {
                        ui.label(RichText::new("IDN like S-0-1000 or P-0-0001").color(WARN));
                    }
                });
            }
            if mb.coe.is_none() {
                return;
            }
            ui.add_space(8.0);
            sub_header(ui, "CoE init commands (start-up SDO downloads)");
            let mut del = None;
            let mut edit = None;
            let mut mv: Option<(usize, bool)> = None;
            egui::Grid::new("coe").striped(true).num_columns(8).spacing([12.0, 4.0]).show(ui, |ui| {
                for h in ["On", "Transition", "Index:Sub", "Data", "CA", "Comment", "Source", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for c in generated.iter() {
                    ui.label("✔");
                    ui.label(&c.transition);
                    ui.monospace(od_index_label(&dict, c.index, c.subindex));
                    ui.monospace(od_data_label(&dict, c));
                    ui.label(yn(c.complete_access));
                    ui.label(RichText::new(&c.comment).weak());
                    ui.label(RichText::new("generated").weak());
                    ui.label("");
                    ui.end_row();
                }
                let n = s.coe_cmds.len();
                for (k, c) in s.coe_cmds.iter_mut().enumerate() {
                    changed |= ui.checkbox(&mut c.enabled, "").changed();
                    ui.label(&c.transition);
                    ui.monospace(od_index_label(&dict, c.index, c.subindex));
                    ui.monospace(od_data_label(&dict, c));
                    ui.label(yn(c.complete_access));
                    ui.label(&c.comment);
                    ui.label(if c.fixed { "ESI" } else { "user" });
                    ui.horizontal(|ui| {
                        if k > 0 && ui.small_button("⬆").clicked() {
                            mv = Some((k, true));
                        }
                        if k + 1 < n && ui.small_button("⬇").clicked() {
                            mv = Some((k, false));
                        }
                        if ui.small_button("✏").on_hover_text("Edit").clicked() {
                            edit = Some(k);
                        }
                        if !c.fixed && ui.small_button("🗑").clicked() {
                            del = Some(k);
                        }
                    });
                    ui.end_row();
                }
            });
            if let Some(k) = del {
                s.coe_cmds.remove(k);
                self.coe_form.editing = None;
                changed = true;
            }
            if let Some((k, up)) = mv {
                let j = if up { k - 1 } else { k + 1 };
                s.coe_cmds.swap(k, j);
                changed = true;
            }
            if let Some(k) = edit {
                let c = &s.coe_cmds[k];
                self.coe_form = CoeForm {
                    value: od::lookup(&dict, c.index, c.subindex)
                        .map(|(_, e)| od::decode_value(&c.data, &e.data_type))
                        .unwrap_or_default(),
                    pick: String::new(),
                    transition: c.transition.clone(),
                    index: format!("{:04X}", c.index),
                    sub: c.subindex.to_string(),
                    data: bytes_to_hex_spaced(&c.data),
                    comment: c.comment.clone(),
                    ca: c.complete_access,
                    editing: Some(k),
                };
            }
            ui.add_space(8.0);
            let f = &mut self.coe_form;
            let hdr = sub_header_resp(ui, if f.editing.is_some() { "Edit command" } else { "Add command" });
            if self.coe_scroll {
                hdr.scroll_to_me(Some(egui::Align::Min));
                self.coe_scroll = false;
            }
            egui::Grid::new("coeform").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Transition");
                egui::ComboBox::from_id_salt("coetr").selected_text(f.transition.clone()).show_ui(ui, |ui| {
                    for t in ["IP", "PS", "SO", "SP", "OP", "OS"] {
                        ui.selectable_value(&mut f.transition, t.to_string(), t);
                    }
                });
                ui.end_row();
                ui.label("Index (hex)");
                ui.add(egui::TextEdit::singleline(&mut f.index).desired_width(80.0).hint_text("8000"));
                ui.end_row();
                ui.label("Subindex");
                ui.add(egui::TextEdit::singleline(&mut f.sub).desired_width(80.0).hint_text("1"));
                ui.end_row();
                let idx_now = u16::from_str_radix(f.index.trim().trim_start_matches("0x").trim_start_matches("#x"), 16).ok();
                let sub_now = parse_num(&f.sub).map(|v| v as u8);
                let found = match (idx_now, sub_now) {
                    (Some(a), Some(b)) => od::lookup(&dict, a, b),
                    _ => None,
                };
                if !dict.is_empty() {
                    ui.label("Object");
                    let shown = found
                        .map(|(o, e)| format!("{} ({}, {} bit)", o.entry_name(e), e.data_type, e.bit_size))
                        .unwrap_or_else(|| "Pick from the object dictionary…".into());
                    egui::ComboBox::from_id_salt("coeobj").selected_text(shown).width(380.0).show_ui(ui, |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.pick).hint_text("search index or name").desired_width(360.0));
                        let q = f.pick.to_lowercase();
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for o in dict.iter() {
                                for e in o.entries.iter().filter(|e| e.writable()) {
                                    let label = format!("{:04X}:{:02X}  {}  ({})", o.index, e.subindex, o.entry_name(e), e.data_type);
                                    if !q.is_empty() && !label.to_lowercase().contains(&q) {
                                        continue;
                                    }
                                    if ui
                                        .selectable_label(found.map(|(fo, fe)| fo.index == o.index && fe.subindex == e.subindex).unwrap_or(false), label)
                                        .clicked()
                                    {
                                        f.index = format!("{:04X}", o.index);
                                        f.sub = e.subindex.to_string();
                                        f.value = od::decode_value(&e.default, &e.data_type);
                                        f.data.clear();
                                        if f.comment.is_empty() {
                                            f.comment = o.entry_name(e);
                                        }
                                    }
                                }
                            }
                        });
                    });
                    ui.end_row();
                }
                if let Some((_, e)) = found {
                    ui.label(format!("Value ({})", e.data_type));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.value).desired_width(160.0).hint_text("e.g. 1000"));
                        if !f.value.trim().is_empty() {
                            match od::encode_value(&f.value, &e.data_type, e.bit_size) {
                                Ok(b) => {
                                    ui.label(RichText::new(format!("= {}", bytes_to_hex_spaced(&b))).monospace().weak());
                                }
                                Err(m) => {
                                    ui.label(RichText::new(m).color(WARN));
                                }
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label(if found.is_some() {
                    "or raw data (hex, little endian)"
                } else {
                    "Data (hex bytes, little endian)"
                });
                ui.add(egui::TextEdit::singleline(&mut f.data).desired_width(240.0).hint_text("e8 03"));
                ui.end_row();
                ui.label("Comment");
                ui.add(egui::TextEdit::singleline(&mut f.comment).desired_width(240.0));
                ui.end_row();
                ui.label("Complete access");
                ui.checkbox(&mut f.ca, "");
                ui.end_row();
            });
            let idx = u16::from_str_radix(f.index.trim().trim_start_matches("0x").trim_start_matches("#x"), 16).ok();
            let sub = parse_num(&f.sub).map(|v| v as u8);
            let od_e = match (idx, sub) {
                (Some(a), Some(b)) => od::lookup(&dict, a, b).map(|(_, e)| e.clone()),
                _ => None,
            };
            let data = match &od_e {
                Some(e) if !f.value.trim().is_empty() => od::encode_value(&f.value, &e.data_type, e.bit_size).unwrap_or_default(),
                _ => hex_to_bytes(&f.data),
            };
            let ok = idx.is_some() && sub.is_some() && !data.is_empty();
            ui.horizontal(|ui| {
                let label = if f.editing.is_some() { "✔ Apply" } else { "➕ Add" };
                if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                    let mut cmd = CoeInitCmd {
                        transition: f.transition.clone(),
                        index: idx.unwrap_or(0),
                        subindex: sub.unwrap_or(0),
                        data: data.clone(),
                        comment: f.comment.clone(),
                        complete_access: f.ca,
                        fixed: false,
                        enabled: true,
                        generated: false,
                        timeout_ms: 3000,
                    };
                    match f.editing {
                        Some(k) if k < s.coe_cmds.len() => {
                            cmd.fixed = s.coe_cmds[k].fixed;
                            cmd.enabled = s.coe_cmds[k].enabled;
                            s.coe_cmds[k] = cmd;
                        }
                        _ => s.coe_cmds.push(cmd),
                    }
                    *f = CoeForm {
                        transition: "PS".into(),
                        ..Default::default()
                    };
                    changed = true;
                }
                if f.editing.is_some() && ui.button("Cancel").clicked() {
                    *f = CoeForm {
                        transition: "PS".into(),
                        ..Default::default()
                    };
                }
                if !ok {
                    ui.label(RichText::new("Enter index, subindex and a value (or at least one data byte)").weak());
                }
            });
        });
        if changed {
            self.dirty = true;
            self.changed = true;
        }
    }

    fn slave_dc(&mut self, ui: &mut egui::Ui, i: usize) {
        let cyc = self.project.master.cycle_time_us;
        let refc = self.project.ref_clock_index();
        let dc_on = self.project.master.dc_enabled;
        let mut changed = false;
        let mut make_ref = false;
        let s = &mut self.project.slaves[i];
        egui::Grid::new("sdc").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Operation mode");
            changed |= dc_mode_combo(ui, "sdcm", s);
            ui.end_row();
            if let Some(m) = s.dc_opmode() {
                ui.label("Description");
                ui.label(&m.desc);
                ui.end_row();
                ui.label("AssignActivate");
                ui.monospace(format!(
                    "{}  (SYNC0 {}, SYNC1 {})",
                    hx(m.assign_activate as u32, 4),
                    yn(m.assign_activate & 0x0200 != 0),
                    yn(m.assign_activate & 0x0400 != 0)
                ));
                ui.end_row();
            }
            ui.label("Reference clock");
            ui.horizontal(|ui| {
                ui.label(if refc == Some(i) { "yes ⏱" } else { "no" });
                if refc != Some(i) && ui.small_button("Use as reference clock").clicked() {
                    make_ref = true;
                }
            });
            ui.end_row();
            ui.label("SYNC0 cycle time");
            ui.horizontal(|ui| {
                let mut auto = s.sync0_cycle_ns == 0;
                if ui.checkbox(&mut auto, "from master cycle").changed() {
                    s.sync0_cycle_ns = if auto { 0 } else { cyc.saturating_mul(1000) };
                    changed = true;
                }
                if auto {
                    ui.label(format!("{} ns", s.sync0_cycle(cyc)));
                } else {
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut s.sync0_cycle_ns)
                                .range(1000..=100_000_000)
                                .clamp_existing_to_range(false)
                                .suffix(" ns")
                                .speed(1000),
                        )
                        .changed();
                }
            });
            ui.end_row();
            ui.label("SYNC0 shift time");
            changed |= ui.add(egui::DragValue::new(&mut s.sync0_shift_ns).suffix(" ns").speed(100)).changed();
            ui.end_row();
            ui.label("SYNC1 cycle time");
            ui.horizontal(|ui| {
                let mut auto = s.sync1_cycle_ns == 0;
                if ui.checkbox(&mut auto, "from op mode").changed() {
                    s.sync1_cycle_ns = if auto { 0 } else { cyc.saturating_mul(1000) };
                    changed = true;
                }
                if auto {
                    ui.label(format!("{} ns", s.sync1_cycle(cyc)));
                } else {
                    changed |= ui.add(egui::DragValue::new(&mut s.sync1_cycle_ns).suffix(" ns").speed(1000)).changed();
                }
            });
            ui.end_row();
            ui.label("SYNC1 shift time");
            changed |= ui.add(egui::DragValue::new(&mut s.sync1_shift_ns).suffix(" ns").speed(100)).changed();
            ui.end_row();
        });
        if !dc_on {
            ui.label(RichText::new("⚠ Distributed clocks are switched off in the master settings.").color(WARN));
        }
        if make_ref {
            self.project.master.ref_clock = Some(i);
            changed = true;
        }
        if changed {
            self.dirty = true;
            self.changed = true;
        }
    }

    fn slave_eeprom(&mut self, ui: &mut egui::Ui, i: usize) {
        let dev = self.project.slaves[i].device.clone();
        let serial = self.project.slaves[i].serial;
        ui.horizontal(|ui| {
            if ui.button("⚙ Generate from ESI").clicked() {
                self.sii_cache = Some((i, sii::build_sii(&dev, serial, 0)));
            }
            let have = self.sii_cache.as_ref().map(|c| c.0 == i).unwrap_or(false);
            if ui.add_enabled(have, egui::Button::new("Save as .bin…")).clicked() {
                let b = self.sii_cache.as_ref().unwrap().1.clone();
                let n = format!("{}.bin", dev.type_name);
                self.save_text("Save EEPROM image", &n, "bin", &b);
            }
            if ui.add_enabled(have, egui::Button::new("Save as Intel HEX…")).clicked() {
                let b = sii::intel_hex(&self.sii_cache.as_ref().unwrap().1);
                let n = format!("{}.hex", dev.type_name);
                self.save_text("Save EEPROM image", &n, "hex", b.as_bytes());
            }
        });
        ui.label(RichText::new("Builds the slave information interface (SII) content: config area with checksum, identity, mailbox, and the strings, general, FMMU, sync manager and PDO categories.").weak());
        ui.add_space(4.0);
        match &self.sii_cache {
            Some((k, b)) if *k == i => {
                let s = &self.project.slaves[i];
                kv_grid(
                    ui,
                    "siisum",
                    &[
                        ("Size", format!("{} bytes", b.len())),
                        ("Config checksum (word 7)", format!("{:#04x}", b[14])),
                        ("Vendor / product", format!("{} / {}", hx(s.device.vendor_id, 8), hx(s.device.product_code, 8))),
                    ],
                );
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    let mut t = sii::hex_dump(&b[..b.len().min(1024)]);
                    if b.len() > 1024 {
                        t.push_str(&format!("… {} more bytes (0xFF filled)\n", b.len() - 1024));
                    }
                    let mut tt = t.as_str();
                    ui.add(egui::TextEdit::multiline(&mut tt).code_editor().desired_width(f32::INFINITY));
                });
            }
            _ => {
                ui.label(RichText::new("Press “Generate from ESI” to build the EEPROM image.").italics().weak());
            }
        }
    }

    fn slave_esc(&mut self, ui: &mut egui::Ui, i: usize, lay: &NetLayout) {
        let sl = &lay.slaves[i];
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            sub_header(ui, "Sync manager registers (0x0800 + 8·n)");
            egui::Grid::new("smreg").striped(true).num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                for h in ["Register", "SM", "Type", "Bytes (start, length, control, status, activate, PDI)"] {
                    ui.strong(h);
                }
                ui.end_row();
                for sm in sl.sms.iter() {
                    ui.monospace(hx(0x0800 + 8 * sm.index as u32, 4));
                    ui.label(sm.index.to_string());
                    ui.label(sm.kind.text());
                    ui.monospace(bytes_to_hex_spaced(&sm_bytes(sm.start, sm.len, sm.control, sm.enable && sm.len > 0)));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            sub_header(ui, "FMMU registers (0x0600 + 16·n)");
            egui::Grid::new("fmreg").striped(true).num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                for h in ["Register", "FMMU", "Use", "Bytes (logical start, length, bits, physical start, type, activate)"] {
                    ui.strong(h);
                }
                ui.end_row();
                for (k, f) in sl.fmmus.iter().enumerate() {
                    ui.monospace(hx(0x0600 + 16 * k as u32, 4));
                    ui.label(k.to_string());
                    ui.label(if f.dir == 2 { "Outputs (write)" } else { "Inputs (read)" });
                    ui.monospace(bytes_to_hex_spaced(&f.bytes()));
                    ui.end_row();
                }
            });
            if sl.fmmus.is_empty() {
                ui.label(RichText::new("No FMMUs used (no process data).").weak());
            }
        });
    }
}

// ---------------------------------------------------------------- helpers

fn toggle_pdo(s: &mut SlaveCfg, k: usize, on: bool) {
    s.pdos[k].assigned = on;
    if on {
        let ex = s.pdos[k].pdo.excludes.clone();
        let idx = s.pdos[k].pdo.index;
        for (j, p) in s.pdos.iter_mut().enumerate() {
            if j != k && !p.pdo.mandatory && (ex.contains(&p.pdo.index) || p.pdo.excludes.contains(&idx)) {
                p.assigned = false;
            }
        }
    }
}

fn dc_mode_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, s: &mut SlaveCfg) -> bool {
    let mut changed = false;
    let cur = s
        .dc_opmode()
        .map(|m| if m.desc.is_empty() { m.name.clone() } else { m.desc.clone() })
        .unwrap_or_else(|| "(none)".into());
    egui::ComboBox::from_id_salt(id).selected_text(cur).width(220.0).show_ui(ui, |ui| {
        for (k, m) in s.device.dc_opmodes.iter().enumerate() {
            let t = if m.desc.is_empty() { m.name.clone() } else { m.desc.clone() };
            if ui.selectable_label(s.dc_mode == Some(k), t).clicked() {
                s.dc_mode = Some(k);
                changed = true;
            }
        }
    });
    changed
}

fn sev_style(s: &Severity) -> (&'static str, Color32) {
    match s {
        Severity::Error => ("❌", ERR),
        Severity::Warning => ("⚠", WARN),
        Severity::Info => ("ℹ", Color32::from_gray(60)),
    }
}

fn file_stem(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "document".into())
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn short_name(name: &str, ty: &str) -> String {
    name.strip_prefix(ty).map(|s| s.trim().to_string()).unwrap_or_else(|| name.to_string())
}

fn pdo_flags(p: &Pdo) -> String {
    let mut v = Vec::new();
    if p.fixed {
        v.push("fixed");
    }
    if p.mandatory {
        v.push("mandatory");
    }
    if p.sm.is_some() {
        v.push("default");
    }
    if v.is_empty() {
        "optional".into()
    } else {
        v.join(", ")
    }
}

fn pdo_flag_icons(p: &Pdo) -> String {
    let mut s = String::new();
    if p.mandatory {
        s.push_str("  🔒");
    }
    if !p.fixed {
        s.push_str("  ✏");
    }
    s
}

fn tab_button(ui: &mut egui::Ui, active: bool, label: &str) -> bool {
    tab_button_en(ui, true, active, label)
}

fn tab_button_en(ui: &mut egui::Ui, enabled: bool, active: bool, label: &str) -> bool {
    let text = if active {
        RichText::new(label).strong().color(Color32::WHITE)
    } else {
        RichText::new(label)
    };
    let b = egui::Button::new(text)
        .wrap_mode(egui::TextWrapMode::Extend)
        .fill(if active { ACCENT } else { Color32::from_rgb(232, 236, 242) })
        .corner_radius(3.0);
    ui.add_enabled(enabled, b).clicked()
}

fn title(ui: &mut egui::Ui, t: &str, sub: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(t).size(19.0).strong().color(Color32::from_rgb(20, 40, 70)));
    ui.label(RichText::new(sub).weak());
    ui.add_space(4.0);
}

fn sub_header_resp(ui: &mut egui::Ui, t: &str) -> egui::Response {
    ui.label(RichText::new(t).strong().color(ACCENT))
}

fn sub_header(ui: &mut egui::Ui, t: &str) {
    ui.label(RichText::new(t).strong().color(ACCENT));
}

fn kv_grid(ui: &mut egui::Ui, id: &str, rows: &[(&str, String)]) {
    egui::Grid::new(id).num_columns(2).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(RichText::new(*k).color(Color32::from_gray(90)));
            ui.label(v);
            ui.end_row();
        }
    });
}

fn sm_def_grid(ui: &mut egui::Ui, sms: &[SmDef]) {
    egui::Grid::new("smdef").striped(true).num_columns(6).spacing([16.0, 4.0]).show(ui, |ui| {
        for h in ["SM", "Type", "Start", "Default size", "Control", "Enable"] {
            ui.strong(h);
        }
        ui.end_row();
        for (k, s) in sms.iter().enumerate() {
            ui.label(k.to_string());
            ui.label(s.kind.text());
            ui.monospace(hx(s.start as u32, 4));
            ui.label(s.default_size.to_string());
            ui.monospace(hx(s.control as u32, 2));
            ui.label(yn(s.enable));
            ui.end_row();
        }
    });
}

fn var_grid(ui: &mut egui::Ui, id: &str, vars: Vec<&Variable>, log_start: u32) {
    egui::Grid::new(id).striped(true).num_columns(7).spacing([14.0, 3.0]).show(ui, |ui| {
        for h in ["Dir", "Name", "Type", "Bits", "Offset (byte.bit)", "Logical address", "Object"] {
            ui.strong(h);
        }
        ui.end_row();
        for v in vars {
            ui.label(RichText::new(if v.is_input { "In" } else { "Out" }).color(if v.is_input { OK_GREEN } else { ACCENT }));
            ui.label(&v.name);
            ui.label(&v.data_type);
            ui.label(v.bit_size.to_string());
            ui.monospace(format!("{}.{}", v.bit_offs / 8, v.bit_offs % 8));
            ui.monospace(format!("{}.{}", hx(log_start.wrapping_add(v.bit_offs / 8), 8), v.bit_offs % 8));
            ui.monospace(format!("{:04X}:{:02X}", v.index, v.subindex));
            ui.end_row();
        }
    });
}

fn init_cmd_grid(ui: &mut egui::Ui, id: &str, cmds: &[EcInitCmd]) {
    egui::Grid::new(id).striped(true).num_columns(8).spacing([12.0, 3.0]).show(ui, |ui| {
        for h in ["Transition", "Cmd", "Adp", "Ado", "Data", "WKC", "Validate", "Comment"] {
            ui.strong(h);
        }
        ui.end_row();
        for c in cmds {
            ui.label(c.transitions.join(","));
            ui.monospace(cmd_name(c.cmd));
            ui.monospace(format!("{:#06x}", c.adp));
            ui.monospace(format!("{:#06x}", c.ado));
            let d = if c.data.len() > 12 {
                format!("{} … ({} bytes)", bytes_to_hex_spaced(&c.data[..8]), c.data.len())
            } else {
                bytes_to_hex_spaced(&c.data)
            };
            ui.monospace(d);
            ui.label(c.cnt.map(|x| x.to_string()).unwrap_or_default());
            ui.monospace(
                c.validate
                    .as_ref()
                    .map(|v| format!("{} ({} ms)", bytes_to_hex_spaced(&v.data), v.timeout_ms))
                    .unwrap_or_default(),
            );
            ui.label(&c.comment);
            ui.end_row();
        }
    });
}

/// "6060:00  Modes of operation" when the dictionary knows the object.
fn od_index_label(dict: &[od::OdObject], index: u16, sub: u8) -> String {
    match od::lookup(dict, index, sub) {
        Some((o, e)) => format!("{:04X}:{:02X}  {}", index, sub, o.entry_name(e)),
        None => format!("{:04X}:{:02X}", index, sub),
    }
}

/// "8  (08)" — the value in the object's type plus the raw bytes.
fn od_data_label(dict: &[od::OdObject], c: &CoeInitCmd) -> String {
    let hex = bytes_to_hex_spaced(&c.data);
    match od::lookup(dict, c.index, c.subindex) {
        Some((_, e)) if !c.complete_access => {
            let v = od::decode_value(&c.data, &e.data_type);
            if v == hex {
                hex
            } else {
                format!("{}  ({})", v, hex)
            }
        }
        _ => hex,
    }
}

fn coe_grid_ro(ui: &mut egui::Ui, id: &str, cmds: &[CoeInitCmd], dict: &[od::OdObject]) {
    egui::Grid::new(id).striped(true).num_columns(5).spacing([12.0, 3.0]).show(ui, |ui| {
        for h in ["Transition", "Index:Sub", "Data", "CA", "Comment"] {
            ui.strong(h);
        }
        ui.end_row();
        for c in cmds {
            ui.label(&c.transition);
            ui.monospace(od_index_label(dict, c.index, c.subindex));
            ui.monospace(od_data_label(dict, c));
            ui.label(yn(c.complete_access));
            ui.label(&c.comment);
            ui.end_row();
        }
    });
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::light();
    v.selection.bg_fill = Color32::from_rgb(200, 222, 245);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0, 70, 140));
    v.panel_fill = Color32::from_rgb(246, 247, 249);
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(238, 242, 247);
    v.hyperlink_color = ACCENT;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(7.0, 3.0);
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_layout();
    }
}

impl App {
    /// Drop a selection that no longer exists (after removing, importing or opening).
    fn fix_selection(&mut self) {
        self.project.ensure_uids();
        if let Sel::Slave(i) = self.sel {
            if i >= self.project.slaves.len() {
                self.sel = Sel::Master;
            }
        }
        if let Sel::Catalog(k) = self.sel {
            if k >= self.lib.devices.len() {
                self.sel = Sel::Master;
            }
        }
        if let Some(k) = self.sel_pdo {
            if let Sel::Slave(i) = self.sel {
                if k >= self.project.slaves[i].pdos.len() {
                    self.sel_pdo = None;
                }
            }
        }
    }

    /// Draw one frame (used by the window and by the off-screen picture mode).
    pub fn ui(&mut self, ctx: &egui::Context) {
        self.poll_scan();
        if self.scan_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // keyboard shortcuts
        self.now = ctx.input(|i| i.time);
        self.continuous = ctx.wants_keyboard_input() || ctx.input(|i| i.pointer.is_decidedly_dragging());
        if self.hist.base.is_none() {
            self.hist.base = Some(self.project.clone());
        }
        if !ctx.wants_keyboard_input() {
            let (u, r, d, ev) = ctx.input(|i| {
                (
                    i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    i.modifiers.command && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                    i.modifiers.command && i.key_pressed(egui::Key::D),
                    i.events.clone(),
                )
            });
            if u {
                self.undo();
            }
            if r {
                self.redo();
            }
            if d {
                self.duplicate_selected();
            }
            for e in ev {
                match e {
                    egui::Event::Copy => {
                        self.copy_selected(ctx);
                    }
                    egui::Event::Cut => {
                        if self.copy_selected(ctx) {
                            self.remove_selected();
                        }
                    }
                    egui::Event::Paste(t) => self.paste(Some(&t)),
                    _ => {}
                }
            }
        }
        let (save, open, export, del) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::E),
                i.key_pressed(egui::Key::Delete),
            )
        });
        if save {
            self.save_project(false);
        }
        if open {
            if let Some(fs) = rfd::FileDialog::new()
                .set_title("Open projects (several allowed)")
                .add_filter("Configuration project (*.ecproj)", &["ecproj", "json"])
                .pick_files()
            {
                self.open_files(fs);
            }
        }
        if export {
            self.export_eni_dialog();
        }
        if del && !ctx.wants_keyboard_input() {
            self.remove_selected();
        }
        // Menu and toolbar may replace the whole network (New, Open, Import ENI, scan),
        // so the layout is computed after them, and again after the tree (which can add
        // devices). Every panel is drawn with numbers that match the current network.
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(236, 240, 245))
                    .inner_margin(egui::Margin::symmetric(8, 5)),
            )
            .show(ctx, |ui| self.toolbar(ui));
        self.fix_selection();
        self.apply_pending();
        self.fix_selection();
        let lay = compute_layout(&self.project);
        let issues = validate(&self.project, &lay);
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(226, 232, 240))
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ctx, |ui| self.status_bar(ui, &lay));
        // dockable workspace: tabs, splits (drag a tab to an edge), floating panels
        let mut dock = std::mem::replace(&mut self.dock, egui_dock::DockState::new(vec![]));
        let mut style = egui_dock::Style::from_egui(ctx.style().as_ref());
        style.tab_bar.bg_fill = Color32::from_rgb(226, 232, 240);
        style.tab.active.bg_fill = Color32::WHITE;
        style.tab.focused.bg_fill = Color32::WHITE;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(214, 221, 230)).inner_margin(egui::Margin::same(2)))
            .show(ctx, |ui| {
                let mut v = Viewer { app: &mut *self, lay, issues };
                egui_dock::DockArea::new(&mut dock)
                    .id(egui::Id::new("workspace"))
                    .style(style)
                    .show_inside(ui, &mut v);
            });
        self.dock = dock;
        self.apply_pending();
        self.fix_selection();
        self.show_windows(ctx);
        self.apply_pending();
        self.continuous = ctx.wants_keyboard_input() || ctx.input(|i| i.pointer.is_decidedly_dragging());
        self.commit_history();

        // files dropped onto the window
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.open_files(dropped);
        }
        if let Some(id) = self.confirm_close {
            if !self.doc_dirty(id) {
                self.confirm_close = None;
                self.close_document(id);
            } else {
                let mut close = false;
                let mut cancel = false;
                egui::Window::new("Close document")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(format!("\"{}\" has unsaved changes.", self.doc_name_of(id)));
                        ui.horizontal(|ui| {
                            if ui.button("Discard changes and close").clicked() {
                                close = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if close {
                    self.confirm_close = None;
                    self.close_document(id);
                } else if cancel {
                    self.confirm_close = None;
                }
            }
        }
        if self.show_about {
            egui::Window::new("About").collapsible(false).resizable(false).show(ctx, |ui| {
                ui.heading("EtherCAT Configurator");
                ui.label("Creates EtherCAT network information (ENI) files from slave descriptions (ESI).");
                ui.label("ESI catalog · network topology · PDO assignment and mapping · CoE start-up commands · distributed clocks · init commands · process image · SII EEPROM generation · bus scan.");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                if ui.button("Close").clicked() {
                    self.show_about = false;
                }
            });
        }
    }
}
