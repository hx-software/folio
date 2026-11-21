mod browser;
mod editor;
mod guides;
mod i18n;
mod storage;
mod worker_client;

use base64::{Engine, engine::general_purpose::STANDARD};
use dioxus::prelude::*;
use editor::Editor;
use numbering_core::{
    Alignment, Arrangement, Artwork, ArtworkKind, CustomFont, Order, PT_PER_MM, Project, Rotation,
};
use numbering_worker::{Operation, Output, Request, Response};
use std::{collections::BTreeSet, rc::Rc, time::Duration};
use worker_client::WorkerClient;

#[derive(Clone)]
struct Job {
    id: u64,
    revision: u64,
    operation: Operation,
}

#[derive(Clone)]
struct Drag {
    id: u32,
    client_x: f64,
    client_y: f64,
    origin_x: f64,
    origin_y: f64,
    origins: Vec<(u32, f64, f64)>,
}

/// A rubber-band selection, tracked in client pixels for hit-testing and in page mm
/// for drawing. With a modifier key it extends the selection that existed beforehand.
#[derive(Clone)]
struct Marquee {
    start_client: (f64, f64),
    start_mm: (f64, f64),
    current_mm: (f64, f64),
    base: BTreeSet<u32>,
    additive: bool,
}

#[derive(Clone, Default)]
struct Background {
    url: String,
    width: f64,
    height: f64,
    pages: usize,
}

/// A document replacement that waits until unsaved edits are resolved.
#[derive(Clone)]
enum PendingReplace {
    New,
    Open(String),
    Load {
        project: Box<Project>,
        id: Option<String>,
        notice: String,
    },
}

/// The signals that describe the open document, grouped so helpers can take one
/// argument instead of a dozen.
#[derive(Clone, Copy)]
struct DocumentCtx {
    editor: Signal<Editor>,
    current_id: Signal<Option<String>>,
    library: Signal<Vec<storage::ProjectSummary>>,
    library_open: Signal<bool>,
    background: Signal<Background>,
    proof: Signal<Option<(u64, usize, String)>>,
    sheet: Signal<usize>,
    export_start: Signal<usize>,
    export_end: Signal<usize>,
    bridge: Bridge,
    pending: Signal<Option<Job>>,
    serial: Signal<u64>,
    message: Signal<String>,
    save_status: Signal<String>,
    saved_revision: Signal<Option<u64>>,
    save_lock: Signal<Rc<futures_util::lock::Mutex<()>>>,
}

type Bridge = Signal<Option<Rc<WorkerClient>>>;

fn main() {
    // Web build: the same bundle also runs as the PDF worker (a Web Worker has no `window`).
    #[cfg(target_arch = "wasm32")]
    if web_sys::window().is_none() {
        numbering_worker::run_worker();
        return;
    }

    // Desktop build: the app starts this same executable as a child PDF worker.
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::args().any(|argument| argument == "--pdf-worker") {
        if let Err(error) = numbering_worker::stdio::run_worker_stdio() {
            eprintln!("PDF worker failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    i18n::select_language(i18n::LanguagePreference::System);

    #[cfg(not(target_arch = "wasm32"))]
    {
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
        let window = WindowBuilder::new()
            .with_title(fl!("folio-precise-numbering"))
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_min_inner_size(LogicalSize::new(960.0, 640.0));
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let window = window.with_window_icon(Some(desktop_icon()));

        dioxus::LaunchBuilder::desktop()
            .with_cfg(Config::new().with_window(window))
            .launch(App);
    }

    #[cfg(target_arch = "wasm32")]
    dioxus::launch(App);
}

/// A smooth continuous-corner ("squircle") outline in a 100 × 100 box.
const SQUIRCLE: &str = "M50 0C85 0 100 15 100 50S85 100 50 100 0 85 0 50 15 0 50 0Z";

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn desktop_icon() -> dioxus::desktop::tao::window::Icon {
    let icon = image::load_from_memory_with_format(
        include_bytes!("../icons/32x32.png"),
        image::ImageFormat::Png,
    )
    .expect("Folio's embedded window icon must be a valid PNG")
    .into_rgba8();
    let (width, height) = icon.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(icon.into_raw(), width, height)
        .expect("Folio's window icon must have valid RGBA dimensions")
}

fn send_job(
    bridge: Bridge,
    pending: Signal<Option<Job>>,
    serial: Signal<u64>,
    editor: Signal<Editor>,
    operation: Operation,
) {
    let (revision, project) = {
        let e = editor.read();
        (e.revision, e.project.clone())
    };
    send_request(bridge, pending, serial, revision, project, operation);
}

fn send_request(
    mut bridge: Bridge,
    mut pending: Signal<Option<Job>>,
    mut serial: Signal<u64>,
    revision: u64,
    project: Project,
    operation: Operation,
) {
    let id = serial() + 1;
    serial.set(id);
    pending.set(Some(Job {
        id,
        revision,
        operation: operation.clone(),
    }));
    if let Some(worker) = bridge.write().as_ref() {
        if let Err(error) = worker.send(Request {
            id,
            project,
            operation,
        }) {
            browser::log_error(&error);
            pending.set(None);
        }
    } else {
        pending.set(None);
    }
}

fn clamp_fields(project: &mut Project) {
    let (w, h) = project.cell_size();
    for field in &mut project.fields {
        field.x_mm = field.x_mm.clamp(0.0, w.max(0.0));
        field.y_mm = field.y_mm.clamp(0.0, h.max(0.0));
    }
}

/// Replaces the current document and resets every view that refers to the old one.
/// `id` is the library entry the document came from, if it has one.
fn adopt_project(project: Project, id: Option<String>, mut doc: DocumentCtx) {
    let has_artwork = project.artwork.is_some();
    let sheet_count = project.sheet_count();
    doc.editor.write().replace(project);
    doc.current_id.set(id);
    doc.background.set(Background::default());
    doc.proof.set(None);
    doc.sheet.set(0);
    doc.export_start.set(1);
    doc.export_end.set(sheet_count.min(500));
    if has_artwork {
        send_job(
            doc.bridge,
            doc.pending,
            doc.serial,
            doc.editor,
            Operation::Inspect {
                adopt_page_size: false,
            },
        );
    }
}

async fn restore_project() -> Result<Option<(Project, Option<String>)>, String> {
    let id = storage::load_current_id().await?;
    let (bytes, id) = match id.as_deref().and_then(|id| id.strip_prefix("library:")) {
        Some(id) => (storage::load_project(id).await?, Some(id.to_string())),
        // Migrate the old recovery snapshot, including edits that had never
        // been written to its associated library entry.
        None => (storage::load().await?, id),
    };
    bytes
        .map(|bytes| {
            Project::from_bytes(&bytes)
                .map(|project| (project, id))
                .map_err(|error| error.to_string())
        })
        .transpose()
}

async fn autosave(mut doc: DocumentCtx) -> Result<(), String> {
    let (revision, bytes, name) = {
        let editor = doc.editor.peek();
        if *doc.saved_revision.peek() == Some(editor.revision) {
            return Ok(());
        }
        (
            editor.revision,
            editor
                .project
                .to_bytes()
                .map_err(|error| error.to_string())?,
            editor.project.name.clone(),
        )
    };
    let existing_id = doc.current_id.peek().clone();
    let id = existing_id.unwrap_or_else(|| {
        let id = storage::new_project_id();
        doc.current_id.set(Some(id.clone()));
        id
    });
    let summary = storage::save_project(&id, &name, &bytes).await?;
    {
        let mut entries = doc.library.write();
        entries.retain(|entry| entry.id != id);
        entries.insert(0, summary);
    }
    storage::save_current_id(Some(&format!("library:{id}"))).await?;
    doc.saved_revision.set(Some(revision));
    if doc.editor.peek().revision == revision {
        doc.save_status.set(fl!("autosaved-to-library"));
    }
    Ok(())
}

fn blank_project(library: &[storage::ProjectSummary]) -> Project {
    let name_prefix = format!("{} ", fl!("untitled-numbering"));
    let next_number = library
        .iter()
        .filter_map(|entry| {
            ["Untitled numbering ", "Numeración sin título "]
                .iter()
                .find_map(|prefix| entry.name.strip_prefix(prefix)?.parse::<u64>().ok())
        })
        .max()
        .map_or(1, |number| number.checked_add(1).unwrap_or(1));
    let number = (next_number..)
        .find(|number| {
            let name = format!("{name_prefix}{number}");
            !library.iter().any(|entry| entry.name == name)
        })
        .expect("a finite library cannot use every project number");
    Project {
        name: format!("{name_prefix}{number}"),
        arrangement: Arrangement::Free,
        ..Project::default()
    }
}

fn apply_replace(action: PendingReplace, mut doc: DocumentCtx) {
    doc.library_open.set(false);
    match action {
        PendingReplace::New => {
            adopt_project(blank_project(&doc.library.peek()), None, doc);
            doc.message.set(fl!("new-project-created"));
        }
        PendingReplace::Open(_) => unreachable!("Library entry must be loaded before replacement"),
        PendingReplace::Load {
            project,
            id,
            notice,
        } => {
            adopt_project(*project, id, doc);
            doc.message.set(notice);
        }
    }
}

fn request_replace(
    action: PendingReplace,
    doc: DocumentCtx,
    mut pending_replace: Signal<Option<PendingReplace>>,
) {
    spawn(async move {
        let mut doc = doc;
        let lock = doc.save_lock.peek().clone();
        let _guard = lock.lock().await;
        let action = match action {
            PendingReplace::Open(id) => {
                if doc.current_id.peek().as_deref() == Some(id.as_str()) {
                    doc.library_open.set(false);
                    return;
                }
                let project = match storage::load_project(&id).await {
                    Ok(Some(bytes)) => {
                        Project::from_bytes(&bytes).map_err(|error| error.to_string())
                    }
                    Ok(None) => Err(fl!("that-project-is-no-longer-in-the")),
                    Err(error) => Err(error),
                };
                match project {
                    Ok(project) => PendingReplace::Load {
                        project: Box::new(project),
                        id: Some(id),
                        notice: fl!("project-opened-from-your-library"),
                    },
                    Err(error) => {
                        doc.message.set(error);
                        return;
                    }
                }
            }
            action => action,
        };
        while *doc.saved_revision.peek() != Some(doc.editor.peek().revision) {
            if let Err(error) = autosave(doc).await {
                doc.save_status.set(fl!("autosave-failed", error = error));
                pending_replace.set(Some(action));
                return;
            }
        }
        apply_replace(action, doc);
        if let Err(error) = autosave(doc).await {
            doc.save_status.set(fl!("autosave-failed", error = error));
        }
    });
}

fn refresh_library(mut doc: DocumentCtx) {
    spawn(async move {
        let lock = doc.save_lock.peek().clone();
        let _guard = lock.lock().await;
        match storage::list_projects().await {
            Ok(entries) => doc.library.set(entries),
            Err(error) => doc.message.set(error),
        }
    });
}

fn export_filename(name: &str) -> String {
    let mut stem = String::new();
    let mut last_dash = false;
    for character in name.trim().chars() {
        if stem.len() >= 80 {
            break;
        }
        if character.is_alphanumeric() {
            stem.push(character);
            last_dash = false;
        } else if !last_dash {
            stem.push('-');
            last_dash = true;
        }
    }
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "project" } else { stem };
    format!("{stem}.folio.json")
}

fn export_project(bytes: Vec<u8>, name: &str, mut message: Signal<String>) {
    let filename = export_filename(name);
    spawn(async move {
        match browser::download(&bytes, &filename, "application/json").await {
            Ok(true) => message.set(fl!("exported-a-project-file-keep-it-as")),
            Ok(false) => message.set(fl!("export-canceled")),
            Err(error) => message.set(error),
        }
    });
}

fn relative_time(updated_at: u64) -> String {
    let seconds = storage::now_millis().saturating_sub(updated_at) / 1000;
    match seconds {
        0..=59 => fl!("just-now"),
        60..=3_599 => fl!("min-ago", count = (seconds / 60)),
        3_600..=86_399 => fl!("h-ago", count = (seconds / 3_600)),
        _ => fl!("d-ago", count = (seconds / 86_400)),
    }
}

const NEW_ICON: &str = "M12 5v14M5 12h14";
const IMPORT_ICON: &str = "M12 16V4m-4 4 4-4 4 4M5 15v5h14v-5";
const EXPORT_ICON: &str = "M12 4v12m-4-4 4 4 4-4M5 16v4h14v-4";
const SETTINGS_ICON: &str = "M4 7h9m4 0h3M4 17h3m4 0h9M13 4v6M7 14v6";
const DELETE_ICON: &str = "M4 7h16m-14 0 1 13h10l1-13M9 7V4h6v3m-5 3v7m4-7v7";

fn action_icon(path_data: &str) -> Element {
    rsx! {
        svg { view_box: "0 0 24 24", width: "20", height: "20", fill: "none",
            stroke: "currentColor", stroke_width: "1.7", stroke_linecap: "round",
            stroke_linejoin: "round", "aria-hidden": "true",
            path { d: path_data }
        }
    }
}

#[component]
fn App() -> Element {
    let mut language = use_context_provider(|| Signal::new(i18n::LanguagePreference::System));
    let _current_language = language();
    let mut language_loaded = use_signal(|| false);
    let mut language_saving = use_signal(|| false);
    let mut settings_open = use_signal(|| false);
    let mut settings_error = use_signal(String::new);
    use_effect(move || {
        let _ = language();
        use i18n_embed::LanguageLoader;
        let language_tag = i18n::LANGUAGE_LOADER.current_language();
        #[cfg(not(target_arch = "wasm32"))]
        dioxus::desktop::window().set_title(&fl!("folio-precise-numbering"));
        spawn(async move {
            let _ = document::eval(&format!(
                "document.documentElement.lang = '{language_tag}';"
            ))
            .join::<()>()
            .await;
        });
    });
    let mut editor = use_signal(Editor::default);
    let mut message = use_signal(|| fl!("your-files-stay-on-this-device"));
    let mut save_status = use_signal(|| fl!("opening-local-storage"));
    let mut restored = use_signal(|| false);
    let mut library_open = use_signal(|| true);
    let mut library = use_signal(Vec::<storage::ProjectSummary>::new);
    let mut library_search = use_signal(String::new);
    let mut library_order = use_signal(|| String::from("recent"));
    let filtered_projects = use_memo(move || {
        let query = library_search.read().trim().to_lowercase();
        let mut projects = library
            .read()
            .iter()
            .filter(|project| project.name.to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        let order = library_order.read();
        projects.sort_by(|a, b| {
            let comparison = match order.as_str() {
                "oldest" => a.updated_at.cmp(&b.updated_at),
                "name-ascending" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                "name-descending" => b.name.to_lowercase().cmp(&a.name.to_lowercase()),
                _ => b.updated_at.cmp(&a.updated_at),
            };
            comparison.then_with(|| a.id.cmp(&b.id))
        });
        projects
    });
    let current_id = use_signal(|| None::<String>);
    let save_lock = use_signal(|| Rc::new(futures_util::lock::Mutex::new(())));
    let saved_revision = use_signal(|| None::<u64>);
    let mut deleting_project = use_signal(|| None::<storage::ProjectSummary>);
    let mut pending_replace = use_signal(|| None::<PendingReplace>);
    let mut background = use_signal(Background::default);
    let mut proof = use_signal(|| None::<(u64, usize, String)>);
    let mut pending = use_signal(|| None::<Job>);
    let serial = use_signal(|| 0_u64);
    let mut bridge: Bridge = use_signal(|| None);
    let mut worker_generation = use_signal(|| 0_u64);
    let mut sheet = use_signal(|| 0_usize);
    let mut zoom = use_signal(|| 1.0_f64);
    let mut drag = use_signal(|| None::<Drag>);
    let mut snap = use_signal(|| true);
    let mut show_guides = use_signal(|| true);
    let mut snap_guides = use_signal(|| true);
    let mut export_start = use_signal(|| 1_usize);
    let mut export_end = use_signal(|| 17_usize);
    let mut mobile_panel = use_signal(|| None::<&'static str>);
    let mut marquee = use_signal(|| None::<Marquee>);
    let mut art_move = use_signal(|| false);
    let dragging_pane = use_signal(|| None::<&'static str>);
    let drop_target = use_signal(|| None::<&'static str>);
    let pane_order = use_signal(|| -> PaneOrder {
        (
            vec!["fields", "sequence", "layout"],
            vec!["artwork", "props", "export"],
        )
    });
    // Pointer start (client px) and the artwork offset (mm) when a drag began.
    let mut art_drag = use_signal(|| None::<(f64, f64, f64, f64)>);
    let mut candidate_font = use_signal(|| None::<CustomFont>);
    let mut just_imported = use_signal(|| false);
    let artwork_open = use_signal(|| true);
    let sequence_open = use_signal(|| true);
    let layout_open = use_signal(|| true);
    let fields_open = use_signal(|| true);
    let props_open = use_signal(|| true);
    let export_open = use_signal(|| true);

    let doc = DocumentCtx {
        editor,
        current_id,
        library,
        library_open,
        background,
        proof,
        sheet,
        export_start,
        export_end,
        bridge,
        pending,
        serial,
        message,
        save_status,
        saved_revision,
        save_lock,
    };

    use_effect(move || {
        let _generation = worker_generation();
        let worker = WorkerClient::new(
            move |response: Response| {
                let (
                    mut pending,
                    mut message,
                    mut editor,
                    mut background,
                    mut proof,
                    mut candidate_font,
                    mut just_imported,
                    mut artwork_open,
                ) = (
                    pending,
                    message,
                    editor,
                    background,
                    proof,
                    candidate_font,
                    just_imported,
                    artwork_open,
                );
                let Some(job) = pending.peek().clone() else {
                    return;
                };
                browser::log_debug(&format!(
                    "[dbg] response id={} job.id={} job.rev={} editor.rev={} op={}",
                    response.id,
                    job.id,
                    job.revision,
                    editor.peek().revision,
                    match &job.operation {
                        Operation::Inspect { adopt_page_size } =>
                            format!("inspect adopt={adopt_page_size}"),
                        Operation::Background => "background".to_string(),
                        _ => "other".to_string(),
                    }
                ));
                if response.id != job.id {
                    return;
                }
                if let Ok(Output::Progress { completed, total }) = &response.result {
                    message.set(fl!(
                        "preparing-sheet-of",
                        completed = completed,
                        total = total
                    ));
                    return;
                }
                pending.set(None);
                if editor.peek().revision != job.revision
                    && !matches!(
                        job.operation,
                        Operation::Inspect { .. } | Operation::Background | Operation::InspectFont
                    )
                {
                    message.set(fl!("the-document-changed-please-run-that-operation"));
                    return;
                }
                match response.result {
                    Err(error) => {
                        just_imported.set(false);
                        if matches!(job.operation, Operation::InspectFont) {
                            candidate_font.set(None);
                        }
                        message.set(error);
                    }
                    Ok(Output::FontChecked(info)) => {
                        let candidate = candidate_font.peek().clone();
                        if let Some(font) = candidate {
                            let label = info.family.unwrap_or_else(|| font.name.clone());
                            // The worker has accepted the file; now let the browser parse it
                            // for the live preview before the project adopts it.
                            spawn(async move {
                                match browser::ensure_font(&font.family, &font.bytes).await {
                                    Ok(()) => {
                                        editor.write().set_font(Some(font));
                                        message.set(fl!(
                                            "font-imported-it-is-embedded-in-exported",
                                            label = label
                                        ));
                                    }
                                    Err(error) => message.set(error),
                                }
                                candidate_font.set(None);
                            });
                        }
                    }
                    Ok(Output::Inspected(info)) => {
                        {
                            let mut e = editor.write();
                            let is_pdf = e
                                .project
                                .artwork
                                .as_ref()
                                .is_some_and(|a| a.kind == ArtworkKind::Pdf);
                            if is_pdf
                                && e.revision == job.revision
                                && matches!(
                                    job.operation,
                                    Operation::Inspect {
                                        adopt_page_size: true
                                    }
                                )
                            {
                                e.project.page.width_mm = info.width_mm;
                                e.project.page.height_mm = info.height_mm;
                                clamp_fields(&mut e.project);
                                e.revision += 1;
                            }
                        }
                        background.set(Background {
                            width: info.width_mm,
                            height: info.height_mm,
                            pages: info.page_count,
                            url: String::new(),
                        });
                        send_job(bridge, pending, serial, editor, Operation::Background);
                    }
                    Ok(Output::Preview(preview)) => {
                        let url = format!("data:image/png;base64,{}", STANDARD.encode(preview.png));
                        match job.operation {
                            Operation::Proof { sheet: index } => {
                                proof.set(Some((job.revision, index, url)));
                                message.set(fl!("proof-rendered-from-the-actual-output-pdf"));
                            }
                            _ => {
                                background.write().url = url;
                                if *just_imported.peek() {
                                    artwork_open.set(false);
                                    just_imported.set(false);
                                }
                                message.set(fl!("artwork-loaded-ready-to-position-numbers"));
                            }
                        }
                    }
                    Ok(Output::Exported(bytes)) => {
                        let (start, end) = match job.operation {
                            Operation::Export {
                                start_sheet,
                                end_sheet,
                            } => (start_sheet + 1, end_sheet),
                            _ => (1, 1),
                        };
                        let filename = format!("folio-sheets-{start}-{end}.pdf");
                        spawn(async move {
                            match browser::download(&bytes, &filename, "application/pdf").await {
                                Ok(true) => message.set(fl!(
                                    "exported-sheets-print-at-actual-size-100",
                                    start = start,
                                    end = end
                                )),
                                Ok(false) => message.set(fl!("export-canceled")),
                                Err(error) => message.set(error),
                            }
                        });
                    }
                    Ok(Output::Progress { .. }) => unreachable!(),
                }
            },
            move |error| {
                let (mut pending, mut message, mut bridge) = (pending, message, bridge);
                pending.set(None);
                bridge.set(None);
                message.set(error);
            },
        );
        match worker {
            Ok(worker) => bridge.set(Some(Rc::new(worker))),
            Err(error) => message.set(error),
        }
    });

    use_future(move || async move {
        // The bundled typeface is part of the binary, so no font file is served.
        if let Err(error) = browser::ensure_font("Noto Sans", numbering_worker::FONT_BYTES).await {
            browser::log_warn(&error);
        }
    });

    // A desktop WebView cannot be measured synchronously from Rust. Keep a cache of the
    // canvas and number rectangles warm so pointer handlers can work without awaiting.
    #[cfg(not(target_arch = "wasm32"))]
    use_future(move || async move {
        loop {
            browser::refresh_metrics().await;
            futures_timer::Delay::new(browser::METRICS_INTERVAL).await;
        }
    });

    use_future(move || async move {
        match storage::load_language().await {
            Ok(value) => {
                let preference =
                    i18n::LanguagePreference::from_value(value.as_deref().unwrap_or("system"));
                i18n::select_language(preference);
                language.set(preference);
            }
            Err(error) => settings_error.set(error),
        }
        language_loaded.set(true);
        match storage::list_projects().await {
            Ok(entries) => library.set(entries),
            Err(error) => message.set(error),
        }
        match restore_project().await {
            Ok(Some((project, id))) => {
                adopt_project(project, id, doc);
                message.set(fl!("restored-your-last-local-project"));
            }
            Ok(None) => adopt_project(blank_project(&library.peek()), None, doc),
            Err(error) => {
                adopt_project(blank_project(&library.peek()), None, doc);
                message.set(fl!("could-not-restore-your-project", error = error));
            }
        }
        restored.set(true);
    });

    use_effect(move || {
        let revision = editor.read().revision;
        if !restored() {
            return;
        }
        if *saved_revision.peek() == Some(revision) {
            return;
        }
        save_status.set(fl!("saving"));
        let is_new = current_id.peek().is_none();
        spawn(async move {
            if !is_new {
                futures_timer::Delay::new(Duration::from_millis(900)).await;
            }
            let lock = save_lock.peek().clone();
            let _guard = lock.lock().await;
            if editor.peek().revision == revision
                && let Err(error) = autosave(doc).await
            {
                save_status.set(fl!("autosave-failed", error = error));
            }
        });
    });

    use_effect(move || {
        let count = editor.read().project.sheet_count().max(1);
        if *export_start.peek() > count {
            export_start.set(count);
        }
        if *export_end.peek() > count {
            export_end.set(count);
        }
    });

    // Re-register an embedded font after a reload or when a project is opened.
    use_effect(move || {
        let family = editor
            .read()
            .project
            .font
            .as_ref()
            .map(|font| font.family.clone());
        if let Some(family) = family
            && !browser::font_loaded(&family)
        {
            let bytes = editor
                .peek()
                .project
                .font
                .as_ref()
                .map(|font| font.bytes.clone())
                .unwrap_or_default();
            spawn(async move {
                if let Err(error) = browser::ensure_font(&family, &bytes).await {
                    message.set(fl!("the-preview-is-using-the-default-font", error = error));
                }
            });
        }
    });

    // Terminating the native worker also stops pathological PDFs and frees its heap.
    use_effect(move || {
        if let Some(job) = pending() {
            spawn(async move {
                futures_timer::Delay::new(Duration::from_millis(60_000)).await;
                if pending.peek().as_ref().is_some_and(|p| p.id == job.id) {
                    pending.set(None);
                    bridge.set(None);
                    worker_generation += 1;
                    message.set(fl!("pdf-operation-timed-out-and-was-terminated"));
                }
            });
        }
    });

    let e = editor.read();
    let p = &e.project;
    let field = e
        .field()
        .cloned()
        .unwrap_or_else(|| Project::default().fields.remove(0));
    let validation = p.validate().err().map(|e| e.to_string());
    let sheet_count = p.sheet_count();
    let last_sheet = sheet_count.max(1);
    let first_number = p
        .field_number(p.sequence.start, 0)
        .unwrap_or(p.sequence.start);
    let active_sheet = sheet().min(sheet_count.saturating_sub(1));
    let current_proof = proof
        .read()
        .as_ref()
        .filter(|(revision, index, _)| *revision == e.revision && *index == active_sheet)
        .map(|(_, _, url)| url.clone());
    let busy = pending.read().is_some();
    let worker_available = bridge.read().is_some();
    let page_width = p.page.width_mm;
    let page_height = p.page.height_mm;
    let (cell_width, cell_height) = p.cell_size();
    let layout = p.layout_sheet(active_sheet).unwrap_or_default();
    let bg = background.read().clone();
    let fit = if bg.width > 0.0 && bg.height > 0.0 {
        let scale = (page_width / bg.width).min(page_height / bg.height);
        let (zoom, dx, dy) = p.artwork.as_ref().map_or((1.0, 0.0, 0.0), |a| {
            (a.scale_percent / 100.0, a.offset_x_mm, a.offset_y_mm)
        });
        let w = bg.width * scale * zoom;
        let h = bg.height * scale * zoom;
        (
            (page_width - w) / 2.0 + dx,
            (page_height - h) / 2.0 + dy,
            w,
            h,
        )
    } else {
        (0.0, 0.0, page_width, page_height)
    };
    let selected = e.selected;
    let selection_count = e.selected_ids.len();
    let mixed_size = p
        .fields
        .iter()
        .any(|f| e.selected_ids.contains(&f.id) && f.font_size_pt != field.font_size_pt);
    let mixed_rotation = p
        .fields
        .iter()
        .any(|f| e.selected_ids.contains(&f.id) && f.rotation != field.rotation);
    let mixed_alignment = p
        .fields
        .iter()
        .any(|f| e.selected_ids.contains(&f.id) && f.alignment != field.alignment);
    let mixed_color = p
        .fields
        .iter()
        .any(|f| e.selected_ids.contains(&f.id) && f.color != field.color);
    let mixed_start = p
        .fields
        .iter()
        .filter(|f| e.selected_ids.contains(&f.id))
        .any(|f| f.start != field.start);
    let start_value = if mixed_start {
        String::new()
    } else {
        field.start.map(|s| s.to_string()).unwrap_or_default()
    };
    let start_placeholder = if mixed_start {
        fl!("mixed")
    } else {
        fl!("project", number = p.sequence.start)
    };
    let free_placement = p.arrangement == Arrangement::Free;
    let proof_showing = current_proof.is_some();
    let font_css = match &p.font {
        Some(font) => format!("'{}', 'Noto Sans'", font.family),
        None => "'Noto Sans'".to_string(),
    };
    let font_label = p
        .font
        .as_ref()
        .map_or(fl!("noto-sans-regular-bundled"), |f| f.name.clone());
    let prefix_gap = p.sequence.prefix_spacing_mm;
    let suffix_gap = p.sequence.suffix_spacing_mm;

    let pane_fields = rsx! {
                    SidebarSection { title: fl!("number-fields"), badge: p.fields.len(), open: fields_open,
                        p { class: "help", {fl!("shift-ctrl-click-to-select-multiple-fields")} }
                        div { class: "selection-actions",
                            span { class: "badge", {fl!("selected", selection_count = selection_count)} }
                            button { class: "text-button", onclick: move |_| editor.write().select_all(), {fl!("select-all")} }
                        }
                        div { class: "field-list",
                            for item in &p.fields {
                                div { class: "field-row",
                                input { r#type: "checkbox", aria_label: fl!("select-number", id = item.id), checked: e.selected_ids.contains(&item.id),
                                    disabled: selection_count == 1 && e.selected_ids.contains(&item.id),
                                    onchange: { let id = item.id; move |_| editor.write().select(id, true) } }
                                button { class: if e.selected_ids.contains(&item.id) { "field-item active" } else { "field-item" }, aria_pressed: e.selected_ids.contains(&item.id),
                                    onclick: { let id = item.id; move |event: MouseEvent| editor.write().select(id, event.modifiers().shift() || event.modifiers().ctrl() || event.modifiers().meta()) },
                                    span { class: "field-symbol", "#" } span { {fl!("number", id = item.id)} } small { "{item.font_size_pt:.0} pt" }
                                }
                                }
                            }
                        }
                        div { class: "two-columns",
                            button { class: "button outline", disabled: p.fields.len() >= 256, onclick: move |_| editor.write().add_field(), {fl!("add")} }
                            button { class: "button quiet", disabled: p.fields.len() <= 1, onclick: move |_| editor.write().delete_field(), {fl!("delete")} }
                        }
                    }
    };
    let pane_sequence = rsx! {
                    SidebarSection { title: fl!("number-sequence"), number: "01", open: sequence_open,
                        div { class: "two-columns",
                            NumberInput { label: fl!("start-at"), value: p.sequence.start as f64, min: 0.0, max: 9_007_199_254_740_991.0,
                                onchange: move |value| editor.write().edit(|p| p.sequence.start = value as u64) }
                            NumberInput { label: fl!("quantity"), value: p.sequence.quantity as f64, min: 1.0, max: 100_000.0,
                                onchange: move |value| { editor.write().edit(|p| p.sequence.quantity = value as u64); export_end.set(editor.read().project.sheet_count().min(500)); } }
                            NumberInput { label: fl!("increment"), value: p.sequence.increment as f64, min: 1.0, max: 1_000_000.0,
                                onchange: move |value| editor.write().edit(|p| p.sequence.increment = value as u64) }
                            NumberInput { label: fl!("minimum-digits"), value: p.sequence.padding as f64, min: 0.0, max: 20.0,
                                onchange: move |value| editor.write().edit(|p| p.sequence.padding = value as usize) }
                            label { class: "control", span { {fl!("prefix")} } input { aria_label: fl!("prefix"), maxlength: "100", value: "{p.sequence.prefix}",
                                oninput: move |event| editor.write().edit(|p| p.sequence.prefix = event.value()) } }
                            label { class: "control", span { {fl!("suffix")} } input { aria_label: fl!("suffix"), maxlength: "100", value: "{p.sequence.suffix}",
                                oninput: move |event| editor.write().edit(|p| p.sequence.suffix = event.value()) } }
                            NumberInput { label: fl!("prefix-spacing-mm"), value: p.sequence.prefix_spacing_mm, min: 0.0, max: 50.0, step: 0.1,
                                onchange: move |value| editor.write().edit(|p| p.sequence.prefix_spacing_mm = value) }
                            NumberInput { label: fl!("suffix-spacing-mm"), value: p.sequence.suffix_spacing_mm, min: 0.0, max: 50.0, step: 0.1,
                                onchange: move |value| editor.write().edit(|p| p.sequence.suffix_spacing_mm = value) }
                        }
                        label { class: "control", span { {fl!("numbering-order")} }
                            select { aria_label: fl!("numbering-order"), value: if p.sequence.order == Order::Sequential { "sequential" } else { "cut-stack" },
                                onchange: move |event| editor.write().edit(|p| p.sequence.order = if event.value() == "cut-stack" { Order::CutAndStack } else { Order::Sequential }),
                                option { value: "sequential", {fl!("across-each-sheet")} } option { value: "cut-stack", {fl!("cut-stack")} }
                            }
                        }
                        label { class: "checkbox-control", input { r#type: "checkbox", checked: p.sequence.reverse,
                            onchange: move |event| { let on = event.checked(); editor.write().edit(|p| p.sequence.reverse = on); } } {fl!("reverse-numbering")} }
                        p { class: "help", if p.sequence.reverse { {fl!("counting-down-the-same-numbers-printed-from")} } else { {fl!("turn-on-to-print-the-same-numbers")} } }
                        p { class: "help", if free_placement { {fl!("one-sequence-number-per-sheet-all-fields")} } else if p.sequence.order == Order::CutAndStack { {fl!("stack-sheets-in-order-cut-then-gather")} } else { {fl!("numbers-flow-left-to-right-top-to")} } }
                        div { class: "sequence-sample", title: fl!("first-number-printed"), "{p.format_number(first_number)}" }
                    }
    };
    let pane_layout = rsx! {
                    SidebarSection { title: fl!("sheet-layout"), number: "02", open: layout_open,
                        label { class: "control", span { {fl!("arrangement")} }
                            select { aria_label: fl!("arrangement"), value: if free_placement { "free" } else { "grid" },
                                onchange: move |event| {
                                    drag.set(None);
                                    editor.write().set_arrangement(if event.value() == "free" { Arrangement::Free } else { Arrangement::Grid });
                                },
                                option { value: "grid", {fl!("grid")} }
                                option { value: "free", {fl!("free-placement")} }
                            }
                        }
                        if free_placement {
                            p { class: "help", {fl!("position-fields-anywhere-on-the-page-each")} }
                            label { class: "checkbox-control", input { r#type: "checkbox", checked: show_guides(),
                                onchange: move |event| show_guides.set(event.checked()) } {fl!("show-anchor-guides")} }
                            label { class: "checkbox-control", input { r#type: "checkbox", checked: snap_guides(), disabled: !show_guides(),
                                onchange: move |event| snap_guides.set(event.checked()) } {fl!("snap-to-anchor-guides")} }
                            p { class: "help guide-help", {fl!("guides-are-editing-aids-only-they-never")} }
                        } else {
                        div { class: "two-columns",
                            NumberInput { label: fl!("columns"), value: p.grid.columns as f64, min: 1.0, max: 20.0,
                                onchange: move |value| editor.write().edit(|p| { p.grid.columns = value as u32; clamp_fields(p); }) }
                            NumberInput { label: fl!("rows"), value: p.grid.rows as f64, min: 1.0, max: 20.0,
                                onchange: move |value| editor.write().edit(|p| { p.grid.rows = value as u32; clamp_fields(p); }) }
                            NumberInput { label: fl!("margin-mm"), value: p.grid.margin_mm, min: 0.0, max: 200.0, step: 0.5,
                                onchange: move |value| editor.write().edit(|p| { p.grid.margin_mm = value; clamp_fields(p); }) }
                            NumberInput { label: fl!("gap-mm"), value: p.grid.gap_mm, min: 0.0, max: 200.0, step: 0.5,
                                onchange: move |value| editor.write().edit(|p| { p.grid.gap_mm = value; clamp_fields(p); }) }
                        }
                        p { class: "help", {fl!("one-item-per-cell-every-number-field")} }
                        }
                    }
    };
    let pane_artwork = rsx! {
                    SidebarSection { title: fl!("artwork-paper"), number: "03", open: artwork_open,
                        button { class: "upload-button", disabled: busy || !restored() || !worker_available, onclick: move |_| {
                            // Open the picker inside the click handler so it still counts as a user gesture.
                            let picking = browser::pick_file(browser::FileKind::Artwork);
                            spawn(async move {
                                let Some(file) = (match picking.await {
                                    Ok(file) => file,
                                    Err(error) => { message.set(error); return; }
                                }) else { return; };
                                let name = file.name;
                                let bytes = file.bytes;
                                let kind = if bytes.starts_with(b"%PDF-") { ArtworkKind::Pdf }
                                    else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { ArtworkKind::Png }
                                    else if bytes.starts_with(b"\xff\xd8\xff") { ArtworkKind::Jpeg }
                                    else { message.set(fl!("choose-a-pdf-png-or-jpeg-file")); return; };
                                let mut p = editor.read().project.clone();
                                p.artwork = Some(Artwork { name, kind, bytes, page_index: 0, scale_percent: 100.0, offset_x_mm: 0.0, offset_y_mm: 0.0 });
                                editor.write().replace(p); background.set(Background::default()); proof.set(None);
                                just_imported.set(true);
                                send_job(bridge, pending, serial, editor, Operation::Inspect { adopt_page_size: true });
                                message.set(fl!("opening-artwork-in-the-rust-pdf-worker"));
                            });
                        },
                            span { class: "upload-icon", "+" } strong { if p.artwork.is_some() { {fl!("replace-artwork")} } else { {fl!("import-artwork")} } }
                            span { {fl!("pdf-png-or-jpeg-up-to-20")} }
                        }
                        if let Some(art) = &p.artwork {
                            div { class: "artwork-info", span { "{art.name}" }
                                button { class: "text-button", disabled: busy, onclick: move |_| {
                                    let mut e = editor.write(); e.project.artwork = None; e.revision += 1;
                                    background.set(Background::default()); proof.set(None);
                                }, {fl!("remove")} }
                            }
                            if bg.pages > 1 {
                                NumberInput { label: fl!("artwork-page"), value: (art.page_index + 1) as f64, min: 1.0, max: bg.pages as f64,
                                    onchange: move |value| {
                                        editor.write().edit(|p| { if let Some(a) = &mut p.artwork { a.page_index = value as usize - 1; } });
                                        proof.set(None); send_job(bridge, pending, serial, editor, Operation::Inspect { adopt_page_size: false });
                                    }
                                }
                            }
                            NumberInput { label: fl!("artwork-scale"), value: art.scale_percent, min: numbering_core::MIN_ARTWORK_SCALE, max: numbering_core::MAX_ARTWORK_SCALE, step: 1.0,
                                onchange: move |value| {
                                    editor.write().edit(|p| { if let Some(a) = &mut p.artwork { a.scale_percent = value; } });
                                    proof.set(None);
                                } }
                            div { class: "two-columns",
                                NumberInput { label: fl!("artwork-x-mm"), value: art.offset_x_mm, min: -numbering_core::MAX_ARTWORK_OFFSET_MM, max: numbering_core::MAX_ARTWORK_OFFSET_MM, step: 0.1,
                                    onchange: move |value| { editor.write().edit(|p| { if let Some(a) = &mut p.artwork { a.offset_x_mm = value; } }); } }
                                NumberInput { label: fl!("artwork-y-mm"), value: art.offset_y_mm, min: -numbering_core::MAX_ARTWORK_OFFSET_MM, max: numbering_core::MAX_ARTWORK_OFFSET_MM, step: 0.1,
                                    onchange: move |value| { editor.write().edit(|p| { if let Some(a) = &mut p.artwork { a.offset_y_mm = value; } }); } }
                            }
                            div { class: "two-columns",
                                button { class: if art_move() { "button outline active" } else { "button outline" }, aria_pressed: art_move(), onclick: move |_| art_move.toggle(),
                                    if art_move() { {fl!("done-moving")} } else { {fl!("move-artwork")} } }
                                button { class: "button quiet", disabled: art.is_centered(), onclick: move |_| { editor.write().edit(|p| { if let Some(a) = &mut p.artwork { a.offset_x_mm = 0.0; a.offset_y_mm = 0.0; } }); }, {fl!("center")} }
                            }
                            p { class: "help", if art_move() { {fl!("drag-the-artwork-or-focus-the-sheet")} } else { {fl!("artwork-fits-proportionally-on-the-sheet-scale")} } }
                        }
                        label { class: "control", span { {fl!("paper-preset")} }
                            select { aria_label: fl!("paper-preset"), value: "custom", onchange: move |event| {
                                let size = match event.value().as_str() { "a4" => Some((210.0,297.0)), "letter" => Some((215.9,279.4)), "a3" => Some((297.0,420.0)), _ => None };
                                if let Some((w,h)) = size { editor.write().edit(|p| { p.page.width_mm = w; p.page.height_mm = h; clamp_fields(p); }); }
                            },
                                option { value: "custom", {fl!("custom-imported")} } option { value: "a4", "A4" }
                                option { value: "letter", {fl!("us-letter")} } option { value: "a3", "A3" }
                            }
                        }
                        div { class: "two-columns",
                            NumberInput { label: fl!("width-mm"), value: page_width, min: 10.0, max: 2000.0, step: 0.1,
                                onchange: move |value| editor.write().edit(|p| { p.page.width_mm = value; clamp_fields(p); }) }
                            NumberInput { label: fl!("height-mm"), value: page_height, min: 10.0, max: 2000.0, step: 0.1,
                                onchange: move |value| editor.write().edit(|p| { p.page.height_mm = value; clamp_fields(p); }) }
                        }
                    }
    };
    let pane_props = rsx! {
                    SidebarSection { title: fl!("field-properties"), open: props_open,
                        if selection_count > 1 { p { class: "help selection-summary", {fl!("editing-fields-x-y-move-the-group", selection_count = selection_count, selected = selected)} } }
                        p { class: "help", if free_placement { {fl!("baseline-anchor-measured-from-the-page-s")} } else { {fl!("baseline-position-relative-to-each-cell")} } }
                        div { class: "two-columns",
                            NumberInput { label: "X · mm", value: field.x_mm, min: 0.0, max: cell_width.max(0.0), step: 0.1,
                                onchange: move |value| { let x = editor.read().field().map_or(0.0, |f| f.x_mm); editor.write().move_selection(value - x, 0.0); } }
                            NumberInput { label: "Y · mm", value: field.y_mm, min: 0.0, max: cell_height.max(0.0), step: 0.1,
                                onchange: move |value| { let y = editor.read().field().map_or(0.0, |f| f.y_mm); editor.write().move_selection(0.0, value - y); } }
                        }
                        label { class: "control", span { {fl!("start-number")} }
                            input { r#type: "number", aria_label: fl!("start-number"), min: "0", step: "1", value: "{start_value}", placeholder: "{start_placeholder}",
                                onchange: move |event| {
                                    let text = event.value();
                                    let text = text.trim();
                                    if text.is_empty() {
                                        editor.write().edit_field(|f| f.start = None);
                                    } else if text.chars().all(|c| c.is_ascii_digit()) {
                                        // Digits that overflow u64 are clamped, like any too-large value.
                                        let number = text.parse::<u64>().unwrap_or(u64::MAX).min(numbering_core::MAX_START);
                                        editor.write().edit_field(|f| f.start = Some(number));
                                    }
                                } }
                        }
                        p { class: "help", {fl!("blank-follows-the-project-s-start-at")} }
                        div { class: "control font-control",
                            span { {fl!("typeface-all-fields")} }
                            div { class: "font-name", title: "{font_label}", "{font_label}" }
                            div { class: "two-columns",
                                button { class: "button outline", disabled: busy || !restored() || !worker_available,
                                    onclick: move |_| {
                                        // Open the picker inside the click handler so it still counts as a user gesture.
                                        let picking = browser::pick_file(browser::FileKind::Font);
                                        spawn(async move {
                                            let Some(file) = (match picking.await {
                                                Ok(file) => file,
                                                Err(error) => { message.set(error); return; }
                                            }) else { return; };
                                            let font = CustomFont::new(file.name, file.bytes);
                                            message.set(fl!("checking-the-font"));
                                            let candidate = Project { font: Some(font.clone()), ..Default::default() };
                                            candidate_font.set(Some(font));
                                            let revision = editor.peek().revision;
                                            send_request(bridge, pending, serial, revision, candidate, Operation::InspectFont);
                                        });
                                    }, {fl!("import-font")} }
                                button { class: "button quiet", disabled: p.font.is_none(),
                                    onclick: move |_| { editor.write().set_font(None); message.set(fl!("using-the-bundled-noto-sans-font")); }, {fl!("use-default")} }
                            }
                        }
                        div { class: "two-columns",
                            NumberInput { label: fl!("size-pt"), value: field.font_size_pt, mixed: mixed_size, min: 1.0, max: 300.0, step: 0.5,
                                onchange: move |value| editor.write().edit_field(|f| f.font_size_pt = value) }
                            label { class: "control", span { if mixed_color { {fl!("colour-mixed")} } else { {fl!("colour")} } } input { r#type: "color", aria_label: fl!("field-colour"), value: "{field.color}",
                                oninput: move |event| editor.write().edit_field(|f| f.color = event.value()) } }
                        }
                        label { class: "control", span { {fl!("rotation")} } select { aria_label: fl!("rotation"), value: if mixed_rotation { "mixed".to_string() } else { field.rotation.degrees().to_string() },
                            onchange: move |event| editor.write().edit_field(|f| f.rotation = match event.value().as_str() { "90" => Rotation::Deg90, "180" => Rotation::Deg180, "270" => Rotation::Deg270, _ => Rotation::Deg0 }),
                            if mixed_rotation { option { value: "mixed", disabled: true, {fl!("mixed")} } }
                            option { value: "0", "0°" } option { value: "90", "90°" } option { value: "180", "180°" } option { value: "270", "270°" }
                        } }
                        label { class: "control", span { {fl!("alignment")} } select { aria_label: fl!("alignment"), value: if mixed_alignment { "mixed" } else { match field.alignment { Alignment::Left => "left", Alignment::Center => "center", Alignment::Right => "right" } },
                            onchange: move |event| editor.write().edit_field(|f| f.alignment = match event.value().as_str() { "center" => Alignment::Center, "right" => Alignment::Right, _ => Alignment::Left }),
                            if mixed_alignment { option { value: "mixed", disabled: true, {fl!("mixed")} } }
                            option { value: "left", {fl!("left")} } option { value: "center", {fl!("centre")} } option { value: "right", {fl!("right")} }
                        } }
                        label { class: "checkbox-control", input { r#type: "checkbox", checked: snap(), onchange: move |event| snap.set(event.checked()) } {fl!("snap-to-0-5-mm")} }
                    }
    };
    let pane_export = rsx! {
                    SidebarSection { title: fl!("export-your-job"), class: "export-panel", open: export_open,
                        p { class: "eyebrow", {fl!("ready-for-the-press")} }
                        p { class: "help", {fl!("vector-pdf-embedded-fonts-no-uploads")} }
                        div { class: "two-columns",
                            NumberInput { label: fl!("first-sheet"), value: export_start() as f64, min: 1.0, max: sheet_count as f64,
                                onchange: move |value| export_start.set(value as usize) }
                            NumberInput { label: fl!("last-sheet"), value: export_end() as f64, min: 1.0, max: sheet_count as f64,
                                onchange: move |value| export_end.set(value as usize) }
                        }
                        button { class: "button primary full-width", disabled: busy || !restored() || validation.is_some() || !worker_available, onclick: move |_| {
                            let start = export_start(); let end = export_end();
                            if start == 0 || end < start || end > editor.read().project.sheet_count() || end - start + 1 > 500 {
                                message.set(fl!("choose-a-valid-range-of-at-most")); return;
                            }
                            message.set(fl!("generating-sheets-in-the-pdf-worker", start = start, end = end));
                            send_job(bridge, pending, serial, editor, Operation::Export { start_sheet: start - 1, end_sheet: end });
                        }, if busy { {fl!("processing")} } else { {fl!("download-pdf")} } }
                        p { class: "help", {fl!("up-to-500-sheets-per-export-print")} }
                    }
    };
    let pane = |id: &str| -> Element {
        match id {
            "artwork" => pane_artwork.clone(),
            "sequence" => pane_sequence.clone(),
            "layout" => pane_layout.clone(),
            "fields" => pane_fields.clone(),
            "props" => pane_props.clone(),
            _ => pane_export.clone(),
        }
    };
    rsx! {
        document::Title { {fl!("folio-precise-numbering")} }
        document::Meta { name: "description", content: fl!("private-local-first-ticket-and-form-numbering") }
        document::Meta { name: "theme-color", content: "#153f39" }
        document::Link { rel: "icon", r#type: "image/svg+xml", href: asset!("/assets/folio.svg") }
        document::Stylesheet { href: asset!("/assets/styles.css") }
        div { class: "app-shell",
            if !library_open() {
            header { class: "app-header", inert: settings_open().then_some(""),
                button { class: "header-icon-button", aria_label: fl!("project-library"), title: fl!("project-library"), disabled: !restored(),
                    onclick: move |_| {
                        library_open.set(true);
                        deleting_project.set(None);
                        refresh_library(doc);
                    },
                    svg { view_box: "0 0 24 24", width: "20", height: "20", fill: "none", stroke: "currentColor", stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                        path { d: "M4 4.5h3.8v15H4z" }
                        path { d: "M9.2 4.5h3.8v15H9.2z" }
                        path { d: "M14.6 5.4l3.2-.9 3 14.6-3.2.9z" }
                        path { d: "M3.5 21h17" }
                    }
                }
                div { class: "brand",
                    svg { class: "brand-mark", view_box: "0 0 100 100", role: "img", "aria-label": "Folio",
                        path { d: SQUIRCLE, fill: "#175c4c" }
                        text { x: "49", y: "62", text_anchor: "middle", font_family: "Georgia, 'Times New Roman', serif", font_style: "italic", font_size: "62", fill: "white", "f" }
                    }
                    div { strong { "folio" } span { {fl!("numbering-studio")} } }
                }
                div { class: "header-document",
                    input { class: "project-title", aria_label: fl!("project-name"), title: "{p.name}", maxlength: "200", value: "{p.name}", disabled: !restored(),
                        oninput: move |event| editor.write().edit(|p| p.name = event.value()) }
                }
                div { class: "job-summary", strong { "{p.sequence.quantity}" } span { {fl!("item-count-label", count = p.sequence.quantity)} } i {} strong { "{sheet_count}" } span { {fl!("sheet-count-label", count = sheet_count)} } }
                div { class: "mobile-sidebar-controls",
                    button { class: "button outline", aria_expanded: mobile_panel() == Some("settings"), onclick: move |_| mobile_panel.set(if mobile_panel() == Some("settings") { None } else { Some("settings") }), {fl!("settings")} }
                    button { class: "button outline", aria_expanded: mobile_panel() == Some("fields"), onclick: move |_| mobile_panel.set(if mobile_panel() == Some("fields") { None } else { Some("fields") }), {fl!("fields")} }
                }
                div { class: "header-actions",
                    button { class: "header-icon-button", title: fl!("app-settings"), aria_label: fl!("app-settings"),
                        onclick: move |_| settings_open.set(true), {action_icon(SETTINGS_ICON)} }
                    button { class: "button quiet", aria_label: fl!("export-project-to-a-file"), disabled: !restored() || validation.is_some(),
                        onclick: move |_| {
                            let (bytes, name) = {
                                let editor = editor.read();
                                match editor.project.to_bytes() {
                                    Ok(bytes) => (bytes, editor.project.name.clone()),
                                    Err(error) => { message.set(error.to_string()); return; }
                                }
                            };
                            export_project(bytes, &name, message);
                        }, {fl!("export")} span { class: "action-detail", {fl!("export-file-detail")} } }
                    button { class: "button outline", aria_label: fl!("new-project"), disabled: !restored(),
                        onclick: move |_| request_replace(PendingReplace::New, doc, pending_replace),
                        {fl!("new-project-short")} span { class: "action-detail", {fl!("new-project-detail")} } }
                }
            }
            main { class: "workspace", aria_busy: !restored(), inert: settings_open().then_some(""),
                if mobile_panel().is_some() {
                    button { class: "sidebar-backdrop", aria_label: fl!("close-sidebar"), onclick: move |_| mobile_panel.set(None) }
                }
                aside { class: if mobile_panel() == Some("settings") { "sidebar left-sidebar mobile-open" } else { "sidebar left-sidebar" }, aria_label: fl!("project-settings"),
                    for id in pane_order().0.clone() {
                        PaneSlot { key: "{id}", id, dragging: dragging_pane, target: drop_target, order: pane_order, {pane(id)} }
                    }
                }
                section { class: "canvas-panel",
                    div { class: "canvas-toolbar",
                        div { class: "segmented",
                            button { class: if current_proof.is_none() { "active" } else { "" }, onclick: move |_| proof.set(None), {fl!("layout")} }
                            button { class: if current_proof.is_some() { "active" } else { "" }, disabled: busy || validation.is_some() || !restored() || !worker_available, onclick: move |_| {
                                send_job(bridge, pending, serial, editor, Operation::Proof { sheet: active_sheet });
                                message.set(fl!("rendering-an-output-pdf-proof"));
                            }, {fl!("pdf-proof")} }
                        }
                        div { class: "toolbar-controls",
                            button { class: "icon-button", aria_label: fl!("undo"), title: fl!("undo"), disabled: !e.can_undo(), onclick: move |_| editor.write().undo(), "↶" }
                            button { class: "icon-button", aria_label: fl!("redo"), title: fl!("redo"), disabled: !e.can_redo(), onclick: move |_| editor.write().redo(), "↷" }
                            select { aria_label: fl!("zoom"), value: "{zoom()}", onchange: move |event| { if let Ok(value) = event.value().parse() { zoom.set(value); } },
                                option { value: "1", {fl!("fit-to-view")} } option { value: "0.75", {fl!("fit-scale-75")} } option { value: "0.5", {fl!("fit-scale-50")} }
                            }
                        }
                    }
                    div { class: "canvas-scroll", style: "--page-ratio: {page_width / page_height}; --preview-scale: {zoom()};",
                        div { class: "paper-wrapper",
                            div { class: "paper-size-label", "{page_width:.1} × {page_height:.1} mm" }
                            svg { id: "page-editor", class: if art_move() { "paper moving-artwork" } else { "paper" }, role: "img", "aria-label": fl!("numbering-layout-drag-a-number-to-move"), tabindex: "0",
                                view_box: "0 0 {page_width} {page_height}", width: "100%", style: "aspect-ratio: {page_width} / {page_height}; touch-action: none;",
                                onpointerdown: move |event| {
                                    if proof_showing || event.trigger_button() != Some(dioxus::html::input_data::MouseButton::Primary) { return; }
                                    let Some(rect) = browser::editor_rect() else { return; };
                                    let pos = event.client_coordinates();
                                    if art_move() {
                                        let origin = editor.read().project.artwork.as_ref().map(|a| (a.offset_x_mm, a.offset_y_mm));
                                        if let Some((ox, oy)) = origin {
                                            browser::capture_editor_pointer(event.pointer_id());
                                            editor.write().checkpoint();
                                            art_drag.set(Some((pos.x, pos.y, ox, oy)));
                                        }
                                        return;
                                    }
                                    let additive = event.modifiers().shift() || event.modifiers().ctrl() || event.modifiers().meta();
                                    browser::capture_editor_pointer(event.pointer_id());
                                    let base = {
                                        let mut e = editor.write();
                                        if additive { e.selected_ids.clone() } else {
                                            let primary = e.selected;
                                            e.select(primary, false);
                                            BTreeSet::new()
                                        }
                                    };
                                    let at = ((pos.x - rect.x) * page_width / rect.width, (pos.y - rect.y) * page_height / rect.height);
                                    marquee.set(Some(Marquee { start_client: (pos.x, pos.y), start_mm: at, current_mm: at, base, additive }));
                                },
                                onpointermove: move |event| {
                                    if let Some((cx, cy, ox, oy)) = art_drag() {
                                        let Some(rect) = browser::editor_rect() else { return; };
                                        let pos = event.client_coordinates();
                                        let mut x = ox + (pos.x - cx) * page_width / rect.width;
                                        let mut y = oy + (pos.y - cy) * page_height / rect.height;
                                        if snap() {
                                            // Pull onto the centred position when close, per axis (about 8 screen px).
                                            if x.abs() * rect.width / page_width < 8.0 { x = 0.0; }
                                            if y.abs() * rect.height / page_height < 8.0 { y = 0.0; }
                                        }
                                        editor.write().set_artwork_offset(x, y);
                                        return;
                                    }
                                    if let Some(active) = marquee() {
                                        let Some(rect) = browser::editor_rect() else { return; };
                                        let pos = event.client_coordinates();
                                        let mut ids: Vec<u32> = if active.additive { active.base.iter().copied().collect() } else { Vec::new() };
                                        ids.extend(browser::fields_in_client_rect(active.start_client.0, active.start_client.1, pos.x, pos.y));
                                        editor.write().set_selection(ids);
                                        marquee.set(Some(Marquee {
                                            current_mm: ((pos.x - rect.x) * page_width / rect.width, (pos.y - rect.y) * page_height / rect.height),
                                            ..active
                                        }));
                                        return;
                                    }
                                    if let Some(start) = drag() {
                                        let Some(rect) = browser::editor_rect() else { return; };
                                        let pos = event.client_coordinates();
                                        let x = start.origin_x + (pos.x - start.client_x) * page_width / rect.width;
                                        let y = start.origin_y + (pos.y - start.client_y) * page_height / rect.height;
                                        let mut e = editor.write();
                                        let tolerance = (e.project.arrangement == Arrangement::Free && show_guides() && snap_guides())
                                            .then_some((6.0 * page_width / rect.width, 6.0 * page_height / rect.height));
                                        let targets: Vec<_> = e.project.fields.iter().filter(|f| !start.origins.iter().any(|(id,_,_)| *id == f.id)).cloned().collect();
                                        let (x,y) = guides::snap_position(&targets, start.id, (x,y), tolerance, snap());
                                        e.translate_from(&start.origins, x - start.origin_x, y - start.origin_y);
                                    }
                                },
                                onpointerup: move |_| { drag.set(None); marquee.set(None); art_drag.set(None); },
                                onpointercancel: move |_| { drag.set(None); marquee.set(None); art_drag.set(None); },
                                onlostpointercapture: move |_| { drag.set(None); marquee.set(None); art_drag.set(None); },
                                onkeydown: move |event| {
                                    if art_move() {
                                        let amount = if event.modifiers().shift() { 1.0 } else { 0.1 };
                                        let delta = match event.key() { Key::ArrowLeft => Some((-amount,0.0)), Key::ArrowRight => Some((amount,0.0)), Key::ArrowUp => Some((0.0,-amount)), Key::ArrowDown => Some((0.0,amount)), _ => None };
                                        let current = editor.read().project.artwork.as_ref().map(|a| (a.offset_x_mm, a.offset_y_mm));
                                        if let Some((x, y)) = current {
                                            if let Some((dx, dy)) = delta {
                                                event.prevent_default();
                                                let mut e = editor.write(); e.checkpoint();
                                                e.set_artwork_offset(((x + dx) * 10.0).round() / 10.0, ((y + dy) * 10.0).round() / 10.0);
                                            } else if event.key() == Key::Home {
                                                event.prevent_default();
                                                let mut e = editor.write(); e.checkpoint(); e.set_artwork_offset(0.0, 0.0);
                                            }
                                        }
                                        if matches!(event.key(), Key::Escape) { art_move.set(false); }
                                        return;
                                    }
                                    if (event.modifiers().ctrl() || event.modifiers().meta()) && matches!(event.key(), Key::Character(ref key) if key.eq_ignore_ascii_case("a")) {
                                        event.prevent_default(); editor.write().select_all(); return;
                                    }
                                    if event.key() == Key::Escape { let primary = editor.read().selected; editor.write().select(primary, false); return; }
                                    let amount = if event.modifiers().shift() { 1.0 } else { 0.1 };
                                    let delta = match event.key() { Key::ArrowLeft => Some((-amount,0.0)), Key::ArrowRight => Some((amount,0.0)), Key::ArrowUp => Some((0.0,-amount)), Key::ArrowDown => Some((0.0,amount)), _ => None };
                                    if let Some((dx,dy)) = delta { event.prevent_default(); editor.write().move_selection(dx,dy); }
                                    if event.key() == Key::Delete || event.key() == Key::Backspace { event.prevent_default(); editor.write().delete_field(); }
                                },
                                rect { width: "{page_width}", height: "{page_height}", fill: "white" }
                                if let Some(url) = current_proof.clone() {
                                    image { href: "{url}", x: "0", y: "0", width: "{page_width}", height: "{page_height}" }
                                } else {
                                    if !bg.url.is_empty() { image { href: "{bg.url}", x: "{fit.0}", y: "{fit.1}", width: "{fit.2}", height: "{fit.3}" } }
                                    if validation.is_none() && !free_placement {
                                        for slot in 0..p.slots() {
                                            rect { class: "cell-guide", x: "{p.grid.margin_mm + (slot % p.grid.columns as usize) as f64 * (cell_width + p.grid.gap_mm)}",
                                                y: "{p.grid.margin_mm + (slot / p.grid.columns as usize) as f64 * (cell_height + p.grid.gap_mm)}",
                                                width: "{cell_width}", height: "{cell_height}", fill: "none", stroke: "#cdd9d5", stroke_width: "0.25", stroke_dasharray: "1.5 1.5" }
                                        }
                                    }
                                    if validation.is_none() && free_placement && show_guides() {
                                        g { class: "anchor-guides", "aria-hidden": "true",
                                            for item in &p.fields {
                                                line { class: if e.selected_ids.contains(&item.id) { "anchor-guide selected" } else if (item.x_mm - field.x_mm).abs() < 0.000_001 { "anchor-guide aligned" } else { "anchor-guide" },
                                                    "data-field-id": "{item.id}", "data-axis": "x",
                                                    x1: "{item.x_mm}", x2: "{item.x_mm}", y1: "0", y2: "{page_height}" }
                                                line { class: if e.selected_ids.contains(&item.id) { "anchor-guide selected" } else if (item.y_mm - field.y_mm).abs() < 0.000_001 { "anchor-guide aligned" } else { "anchor-guide" },
                                                    "data-field-id": "{item.id}", "data-axis": "y",
                                                    x1: "0", x2: "{page_width}", y1: "{item.y_mm}", y2: "{item.y_mm}" }
                                            }
                                        }
                                    }
                                    for number in layout {
                                        g { class: if e.selected_ids.contains(&number.field.id) { "number-field selected" } else { "number-field" }, "data-field-id": "{number.field.id}",
                                            transform: "translate({number.x_mm} {number.y_mm}) rotate({number.field.rotation.degrees()})",
                                            onpointerdown: {
                                                let f = number.field.clone();
                                                move |event: PointerEvent| {
                                                    event.prevent_default(); event.stop_propagation();
                                                    browser::capture_editor_pointer(event.pointer_id());
                                                    let pos = event.client_coordinates();
                                                    let mut e = editor.write();
                                                    if event.modifiers().shift() || event.modifiers().ctrl() || event.modifiers().meta() {
                                                        e.select(f.id, true); return;
                                                    }
                                                    if !e.selected_ids.contains(&f.id) { e.select(f.id, false); }
                                                    e.selected = f.id; e.checkpoint();
                                                    drag.set(Some(Drag { id: f.id, client_x: pos.x, client_y: pos.y, origin_x: f.x_mm, origin_y: f.y_mm, origins: e.selection_origins() }));
                                                }
                                            },
                                            text { x: "0", y: "0", fill: "{number.field.color}", font_family: "{font_css}", font_size: "{number.field.font_size_pt / PT_PER_MM}",
                                                text_anchor: match number.field.alignment { Alignment::Left => "start", Alignment::Center => "middle", Alignment::Right => "end" },
                                                if !number.prefix.is_empty() { tspan { "{number.prefix}" } }
                                                tspan { dx: if number.prefix.is_empty() { 0.0 } else { prefix_gap }, "{number.digits}" }
                                                if !number.suffix.is_empty() { tspan { dx: suffix_gap, "{number.suffix}" } }
                                            }
                                            if e.selected_ids.contains(&number.field.id) { circle { cx: "0", cy: "0", r: "0.9", fill: "#0c8a6b", stroke: "white", stroke_width: "0.3" } }
                                        }
                                    }
                                    if let Some(active) = marquee() {
                                        rect { class: "marquee", x: "{active.start_mm.0.min(active.current_mm.0)}", y: "{active.start_mm.1.min(active.current_mm.1)}",
                                            width: "{(active.current_mm.0 - active.start_mm.0).abs()}", height: "{(active.current_mm.1 - active.start_mm.1).abs()}" }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "canvas-footer",
                        div { class: "sheet-navigation",
                            button { class: "icon-button", aria_label: fl!("previous-sheet"), disabled: active_sheet == 0, onclick: move |_| sheet.set(active_sheet.saturating_sub(1)), "‹" }
                            span { class: "sheet-jump", {fl!("sheet")}
                                input { id: "sheet-number", r#type: "number", aria_label: fl!("sheet-number"), min: "1", max: "{last_sheet}", step: "1", value: "{active_sheet + 1}",
                                    onchange: move |event| {
                                        let text = event.value();
                                        let text = text.trim();
                                        // Digits beyond usize still mean "the last sheet"; anything else is undone.
                                        let shown = if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
                                            let target = text.parse::<usize>().unwrap_or(usize::MAX).clamp(1, last_sheet);
                                            sheet.set(target - 1);
                                            target
                                        } else {
                                            active_sheet + 1
                                        };
                                        browser::set_input_value("sheet-number", &shown.to_string());
                                    } }
                                {fl!("of", sheet_count = sheet_count)}
                            }
                            button { class: "icon-button", aria_label: fl!("next-sheet"), disabled: active_sheet + 1 >= sheet_count, onclick: move |_| sheet.set((active_sheet + 1).min(last_sheet - 1)), "›" }
                        }
                        span { class: "help", if current_proof.is_some() { {fl!("output-pdf-proof")} } else if free_placement { {fl!("drag-near-a-guide-to-align-drag")} } else { {fl!("drag-to-position-drag-empty-space-to")} } }
                    }
                }
                aside { class: if mobile_panel() == Some("fields") { "sidebar right-sidebar mobile-open" } else { "sidebar right-sidebar" }, aria_label: fl!("number-fields-and-properties"),
                    for id in pane_order().1.clone() {
                        PaneSlot { key: "{id}", id, dragging: dragging_pane, target: drop_target, order: pane_order, {pane(id)} }
                    }
                }
            }
            if let Some(error) = &validation { div { class: "validation", role: "alert", {i18n::message(error)} } }
            }
            if library_open() {
                main { class: "library-view", aria_labelledby: "library-title", aria_busy: !restored(),
                    inert: (deleting_project().is_some() || pending_replace().is_some() || settings_open()).then_some(""),
                    div { class: "library-content",
                        header { class: "library-header",
                            div {
                                h1 { id: "library-title", {fl!("project-library")} }
                                p { {fl!("saved-inside-folio-on-this-device")} }
                            }
                            div { class: "library-header-actions",
                                button { class: "header-icon-button", title: fl!("app-settings"), aria_label: fl!("app-settings"),
                                    onclick: move |_| settings_open.set(true), {action_icon(SETTINGS_ICON)} }
                            button { class: "button outline", disabled: !restored(), onclick: move |_| library_open.set(false), {fl!("back-to-editor")} }
                            }
                        }
                        div { class: "library-actions",
                            button { class: "header-icon-button", title: fl!("new-project"), aria_label: fl!("new-project"), disabled: !restored(),
                                onclick: move |_| request_replace(PendingReplace::New, doc, pending_replace),
                                {action_icon(NEW_ICON)} }
                            button { class: "header-icon-button", title: fl!("export-current-project"), aria_label: fl!("export-current-project"), disabled: !restored() || validation.is_some(),
                                onclick: move |_| {
                                    let (bytes, name) = {
                                        let editor = editor.read();
                                        match editor.project.to_bytes() {
                                            Ok(bytes) => (bytes, editor.project.name.clone()),
                                            Err(error) => { message.set(error.to_string()); return; }
                                        }
                                    };
                                    export_project(bytes, &name, message);
                                }, {action_icon(EXPORT_ICON)} }
                            button { class: "header-icon-button", title: fl!("import-project"), aria_label: fl!("import-project"), disabled: !restored(),
                                onclick: move |_| {
                                    // Open the picker inside the click handler so it still counts as a user gesture.
                                    let picking = browser::pick_file(browser::FileKind::Project);
                                    spawn(async move {
                                        let Some(file) = (match picking.await {
                                            Ok(file) => file,
                                            Err(error) => { message.set(error); return; }
                                        }) else { return; };
                                        match Project::from_bytes(&file.bytes) {
                                            Ok(project) => request_replace(
                                                PendingReplace::Load {
                                                    project: Box::new(project),
                                                    id: None,
                                                    notice: fl!("project-imported"),
                                                },
                                                doc,
                                                pending_replace,
                                            ),
                                            Err(error) => message.set(error.to_string()),
                                        }
                                    });
                                }, {action_icon(IMPORT_ICON)} }
                            label { class: "control library-order",
                                select { aria_label: fl!("library-order"), value: "{library_order}",
                                    disabled: !restored(),
                                    onchange: move |event| library_order.set(event.value()),
                                    option { value: "recent", {fl!("library-order-recent")} }
                                    option { value: "oldest", {fl!("library-order-oldest")} }
                                    option { value: "name-ascending", {fl!("library-order-name-ascending")} }
                                    option { value: "name-descending", {fl!("library-order-name-descending")} }
                                }
                            }
                            label { class: "control library-search",
                                input { r#type: "search", aria_label: fl!("search-projects"),
                                    placeholder: fl!("search-projects-placeholder"), value: "{library_search}",
                                    disabled: !restored(),
                                    oninput: move |event| library_search.set(event.value()),
                                    onkeydown: move |event: KeyboardEvent| {
                                        if event.key() == Key::Escape { library_search.set(String::new()); }
                                    }
                                }
                            }
                        }
                        div { class: "library-list", aria_live: "polite",
                            if !restored() {
                                p { class: "library-empty", {fl!("loading-projects")} }
                            } else if library().is_empty() {
                                p { class: "library-empty", {fl!("no-projects-yet-create-a-new-project")} }
                            } else if filtered_projects().is_empty() {
                                p { class: "library-empty", {fl!("search-projects-empty")} }
                            } else {
                                for entry in filtered_projects() {
                                    div { key: "{entry.id}", class: if current_id().as_deref() == Some(entry.id.as_str()) { "library-row active" } else { "library-row" },
                                        button { class: "library-open",
                                            onclick: {
                                                let id = entry.id.clone();
                                                move |_| request_replace(
                                                    PendingReplace::Open(id.clone()), doc, pending_replace,
                                                )
                                            },
                                            strong { "{entry.name}" }
                                            span { {fl!("edited", time = relative_time(entry.updated_at))} }
                                        }
                                        div { class: "library-row-actions",
                                            if current_id().as_deref() == Some(entry.id.as_str()) {
                                                span { class: "badge", {fl!("current")} }
                                            }
                                            button { class: "header-icon-button", title: fl!("export-2", name = entry.name.clone()), aria_label: fl!("export-to-a-file", name = entry.name.clone()),
                                                onclick: {
                                                    let id = entry.id.clone();
                                                    let name = entry.name.clone();
                                                    move |_| {
                                                        let id = id.clone();
                                                        let name = name.clone();
                                                        spawn(async move {
                                                            match storage::load_project(&id).await {
                                                                Ok(Some(bytes)) => export_project(bytes, &name, message),
                                                                Ok(None) => message.set(fl!("that-project-is-no-longer-in-the")),
                                                                Err(error) => message.set(error),
                                                            }
                                                        });
                                                    }
                                                }, {action_icon(EXPORT_ICON)} }
                                            button { class: "header-icon-button", title: fl!("delete-2", name = entry.name.clone()), aria_label: fl!("delete-2", name = entry.name.clone()),
                                                onclick: {
                                                    let entry = entry.clone();
                                                    move |_| deleting_project.set(Some(entry.clone()))
                                                }, {action_icon(DELETE_ICON)} }
                                        }
                                    }
                                }
                            }
                        }
                        p { class: "library-footer", {fl!("projects-are-kept-in-this-app-s")} }
                    }
                }
            }
            footer { class: "status-bar",
                inert: (deleting_project().is_some() || settings_open()).then_some(""),
                span { role: "status", aria_live: "polite", {i18n::message(&message())} }
                if busy {
                    button { class: "button outline", onclick: move |_| {
                        pending.set(None); bridge.set(None); worker_generation += 1;
                        message.set(fl!("pdf-operation-cancelled-the-worker-has-been"));
                    }, {fl!("cancel-operation")} }
                } else if bridge.read().is_none() {
                    button { class: "button outline", onclick: move |_| worker_generation += 1, {fl!("restart-pdf-worker")} }
                }
                span { class: "save-status", {i18n::message(&save_status())} }
            }
            if settings_open() {
                div { class: "library-backdrop settings-backdrop", onclick: move |_| settings_open.set(false),
                    section { class: "settings-pane", role: "dialog", aria_modal: "true", aria_labelledby: "settings-title",
                        onclick: move |event: MouseEvent| event.stop_propagation(),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Escape { settings_open.set(false); }
                        },
                        header { class: "library-header",
                            h2 { id: "settings-title", {fl!("app-settings")} }
                            button { class: "icon-button", autofocus: true, aria_label: fl!("close-settings"),
                                onclick: move |_| settings_open.set(false), "×" }
                        }
                        div { class: "settings-body",
                            label { class: "control",
                                span { {fl!("interface-language")} }
                                select { aria_label: fl!("interface-language"), value: language().value(),
                                    disabled: !language_loaded() || language_saving(),
                                    onchange: move |event| {
                                        let preference = i18n::LanguagePreference::from_value(&event.value());
                                        language_saving.set(true);
                                        settings_error.set(String::new());
                                        spawn(async move {
                                            match storage::save_language(preference.value()).await {
                                                Ok(()) => {
                                                    i18n::select_language(preference);
                                                    language.set(preference);
                                                    message.set(fl!("language-updated"));
                                                    save_status.set(if *saved_revision.peek() == Some(editor.peek().revision) {
                                                        fl!("autosaved-to-library")
                                                    } else { fl!("saving") });
                                                }
                                                Err(error) => settings_error.set(error),
                                            }
                                            language_saving.set(false);
                                        });
                                    },
                                    option { value: "system", {fl!("language-system")} }
                                    option { value: "en", "English" }
                                    option { value: "es-MX", "Español (México)" }
                                }
                            }
                            p { class: "help", {fl!("language-preference-help")} }
                            if !settings_error().is_empty() {
                                p { class: "settings-error", role: "alert", {fl!("language-preference-error", error = settings_error())} }
                            }
                        }
                    }
                }
            }
            if let Some(entry) = deleting_project() {
                div { class: "library-backdrop", onclick: move |_| deleting_project.set(None),
                    section { class: "library-dialog", role: "alertdialog", aria_modal: "true",
                        aria_labelledby: "delete-title", aria_describedby: "delete-description",
                        onclick: move |event: MouseEvent| event.stop_propagation(),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Escape {
                                deleting_project.set(None);
                            }
                        },
                        header { class: "library-header",
                            h2 { id: "delete-title", {fl!("delete-project")} }
                        }
                        div { class: "unsaved-body",
                            p { id: "delete-description", {fl!("will-be-permanently-removed-from-your-library", name = entry.name.clone())} }
                        }
                        div { class: "unsaved-actions",
                            button { class: "button quiet", autofocus: true,
                                onclick: move |_| deleting_project.set(None), {fl!("cancel")} }
                            button { class: "button outline danger",
                                onclick: move |_| {
                                    let id = entry.id.clone();
                                    deleting_project.set(None);
                                    spawn(async move {
                                        let lock = save_lock.peek().clone();
                                        let _guard = lock.lock().await;
                                        match storage::delete_project(&id).await {
                                            Ok(()) => {
                                                if current_id.peek().as_deref() == Some(id.as_str()) {
                                                    adopt_project(blank_project(&doc.library.peek()), None, doc);
                                                    if let Err(error) = autosave(doc).await {
                                                        save_status.set(fl!("autosave-failed", error = error));
                                                    }
                                                }
                                                library.write().retain(|project| project.id != id);
                                                message.set(fl!("project-deleted-from-your-library"));
                                            }
                                            Err(error) => message.set(error),
                                        }
                                    });
                                }, {fl!("delete-project-2")} }
                        }
                    }
                }
            }
            if pending_replace().is_some() {
                div { class: "library-backdrop", onclick: move |_| pending_replace.set(None),
                    section { class: "library-dialog unsaved-dialog", role: "dialog", "aria-modal": "true", "aria-labelledby": "unsaved-title",
                        onclick: move |event: MouseEvent| event.stop_propagation(),
                        header { class: "library-header",
                            div {
                                h2 { id: "unsaved-title", {fl!("autosave-could-not-finish")} }
                                p { {fl!("this-project-has-changes-that-are-not")} }
                            }
                            button { class: "icon-button", aria_label: fl!("cancel"), onclick: move |_| pending_replace.set(None), "×" }
                        }
                        div { class: "unsaved-body",
                            p { {fl!("retry-autosave-discard-the-unsaved-changes-or")} }
                        }
                        div { class: "unsaved-actions",
                            button { class: "button primary", disabled: validation.is_some(),
                                onclick: move |_| {
                                    if let Some(action) = pending_replace.take() {
                                        request_replace(action, doc, pending_replace);
                                    }
                                }, {fl!("retry-autosave")} }
                            button { class: "button quiet",
                                onclick: move |_| {
                                    if let Some(action) = pending_replace.take() {
                                        spawn(async move {
                                            let lock = save_lock.peek().clone();
                                            let _guard = lock.lock().await;
                                            apply_replace(action, doc);
                                            if let Err(error) = autosave(doc).await {
                                                save_status.set(fl!("autosave-failed", error = error));
                                            }
                                        });
                                    }
                                }, {fl!("discard-changes")} }
                            button { class: "button quiet", onclick: move |_| pending_replace.set(None), {fl!("cancel")} }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SidebarSection(
    title: String,
    mut open: Signal<bool>,
    #[props(default = "")] number: &'static str,
    #[props(default = 0)] badge: usize,
    #[props(default = "")] class: &'static str,
    children: Element,
) -> Element {
    let _language = use_context::<Signal<i18n::LanguagePreference>>()();
    // The open state lives in Rust, so a section can be collapsed programmatically
    // (after an artwork import) as well as by clicking or Enter/Space on its heading.
    rsx! {
        details { class: "panel {class}", open: open(),
            summary { class: "panel-summary", draggable: "true", title: fl!("click-to-expand-or-collapse-drag-to"),
                onclick: move |event| {
                    event.prevent_default();
                    open.toggle();
                },
                svg { class: "pane-grip", view_box: "0 0 10 16", width: "10", height: "16", "aria-hidden": "true",
                    for (cx, cy) in [(2.5, 3.0), (7.5, 3.0), (2.5, 8.0), (7.5, 8.0), (2.5, 13.0), (7.5, 13.0)] {
                        circle { cx: "{cx}", cy: "{cy}", r: "1.3", fill: "currentColor" }
                    }
                }
                h2 {
                    if !number.is_empty() { span { class: "section-number", "{number}" } }
                    "{title}"
                }
                if badge > 0 { span { class: "badge", "{badge}" } }
            }
            div { class: "panel-body", {children} }
        }
    }
}

#[component]
fn NumberInput(
    label: String,
    value: f64,
    min: f64,
    max: f64,
    #[props(default = 1.0)] step: f64,
    #[props(default = false)] mixed: bool,
    onchange: EventHandler<f64>,
) -> Element {
    let _language = use_context::<Signal<i18n::LanguagePreference>>()();
    rsx! {
        label { class: "control", span { "{label}" }
            input { r#type: "number", aria_label: "{label}", value: if mixed { String::new() } else { value.to_string() }, placeholder: fl!("mixed"), min: "{min}", max: "{max}", step: "{step}",
                onchange: move |event| {
                    if let Ok(value) = event.value().parse::<f64>()
                        && value.is_finite() {
                        onchange.call(value.clamp(min, max.max(min)));
                    }
                }
            }
        }
    }
}

/// The sidebar panes, left sidebar first, each in display order.
type PaneOrder = (Vec<&'static str>, Vec<&'static str>);

fn pane_title(id: &str) -> String {
    match id {
        "artwork" => fl!("artwork-paper"),
        "sequence" => fl!("number-sequence"),
        "layout" => fl!("sheet-layout"),
        "fields" => fl!("number-fields"),
        "props" => fl!("field-properties"),
        _ => fl!("export-your-job"),
    }
}

fn pane_cell(order: &mut PaneOrder, (side, index): (usize, usize)) -> &mut &'static str {
    if side == 0 {
        &mut order.0[index]
    } else {
        &mut order.1[index]
    }
}

/// Exchanges the positions of two panes, even across sidebars.
fn swap_panes(order: &mut PaneOrder, a: &'static str, b: &'static str) {
    let find = |order: &PaneOrder, id: &str| {
        order
            .0
            .iter()
            .position(|p| *p == id)
            .map(|i| (0, i))
            .or_else(|| order.1.iter().position(|p| *p == id).map(|i| (1, i)))
    };
    let (Some(from), Some(to)) = (find(order, a), find(order, b)) else {
        return;
    };
    let (first, second) = (*pane_cell(order, from), *pane_cell(order, to));
    *pane_cell(order, from) = second;
    *pane_cell(order, to) = first;
}

/// A sidebar pane. Its header is the drag handle (dragstart bubbles up from the
/// `<summary>`); dropping on another pane swaps them, and the pane under the pointer is
/// highlighted so the landing spot is clear.
#[component]
fn PaneSlot(
    id: &'static str,
    mut dragging: Signal<Option<&'static str>>,
    mut target: Signal<Option<&'static str>>,
    mut order: Signal<PaneOrder>,
    children: Element,
) -> Element {
    let _language = use_context::<Signal<i18n::LanguagePreference>>()();
    let active = dragging();
    let class = match active {
        Some(from) if from == id => "pane-slot is-dragging",
        Some(_) if target() == Some(id) => "pane-slot drop-target",
        Some(_) => "pane-slot drop-ready",
        None => "pane-slot",
    };
    let swap_label = active
        .map(|from| fl!("swap-with", pane = pane_title(from)))
        .unwrap_or_default();
    rsx! {
        div { class: "{class}", "data-pane": "{id}", "data-swap": "{swap_label}",
            ondragstart: move |event| {
                let _ = event.data_transfer().set_data("text/plain", id);
                dragging.set(Some(id));
            },
            ondragend: move |_| { dragging.set(None); target.set(None); },
            ondragover: move |event| {
                if dragging().is_some_and(|from| from != id) {
                    event.prevent_default();
                    if target() != Some(id) { target.set(Some(id)); }
                }
            },
            ondrop: move |event| {
                event.prevent_default();
                if let Some(from) = dragging()
                    && from != id
                {
                    swap_panes(&mut order.write(), from, id);
                }
                dragging.set(None);
                target.set(None);
            },
            {children}
        }
    }
}
