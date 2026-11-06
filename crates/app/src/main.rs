mod browser;
mod editor;
mod guides;
mod storage;
mod worker_client;

use base64::{Engine, engine::general_purpose::STANDARD};
use dioxus::prelude::*;
use editor::Editor;
use numbering_core::{
    Alignment, Arrangement, Artwork, ArtworkKind, Order, PT_PER_MM, Project, Rotation,
};
use numbering_worker::{Operation, Output, Request, Response};
use std::rc::Rc;
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

#[derive(Clone, Default)]
struct Background {
    url: String,
    width: f64,
    height: f64,
    pages: usize,
}

type Bridge = Signal<Option<Rc<WorkerClient>>>;

fn main() {
    console_error_panic_hook::set_once();
    dioxus::launch(App);
}

fn send_job(
    mut bridge: Bridge,
    mut pending: Signal<Option<Job>>,
    mut serial: Signal<u64>,
    editor: Signal<Editor>,
    operation: Operation,
) {
    let id = serial() + 1;
    serial.set(id);
    let e = editor.read();
    pending.set(Some(Job {
        id,
        revision: e.revision,
        operation: operation.clone(),
    }));
    if let Some(worker) = bridge.write().as_ref() {
        if let Err(error) = worker.send(Request {
            id,
            project: e.project.clone(),
            operation,
        }) {
            web_sys::console::error_1(&error.into());
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

#[component]
fn App() -> Element {
    let mut editor = use_signal(Editor::default);
    let mut message = use_signal(|| String::from("Your files stay on this device."));
    let mut save_status = use_signal(|| String::from("Opening local storage…"));
    let mut restored = use_signal(|| false);
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

    use_effect(move || {
        let _generation = worker_generation();
        let worker = WorkerClient::new(
            move |response: Response| {
                let (mut pending, mut message, mut editor, mut background, mut proof) =
                    (pending, message, editor, background, proof);
                let Some(job) = pending.peek().clone() else {
                    return;
                };
                if response.id != job.id {
                    return;
                }
                if let Ok(Output::Progress { completed, total }) = &response.result {
                    message.set(format!("Preparing sheet {completed} of {total}…"));
                    return;
                }
                pending.set(None);
                if editor.peek().revision != job.revision
                    && !matches!(
                        job.operation,
                        Operation::Inspect { .. } | Operation::Background
                    )
                {
                    message.set("The document changed. Please run that operation again.".into());
                    return;
                }
                match response.result {
                    Err(error) => message.set(error),
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
                                message.set("Proof rendered from the actual output PDF. Print at actual size (100%).".into());
                            }
                            _ => {
                                background.write().url = url;
                                message.set("Artwork loaded. Ready to position numbers.".into());
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
                        match browser::download(&bytes, &format!("folio-sheets-{start}-{end}.pdf"), "application/pdf") {
                        Ok(()) => message.set(format!("Exported sheets {start}–{end}. Print at actual size (100%), without fit-to-page.")),
                        Err(error) => message.set(error),
                    }
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
        match storage::load().await {
            Ok(Some(bytes)) => match Project::from_bytes(&bytes) {
                Ok(project) => {
                    let has_artwork = project.artwork.is_some();
                    editor.write().replace(project);
                    export_end.set(editor.read().project.sheet_count().min(500));
                    if has_artwork {
                        send_job(
                            bridge,
                            pending,
                            serial,
                            editor,
                            Operation::Inspect {
                                adopt_page_size: false,
                            },
                        );
                    }
                    message.set("Restored your last local project.".into());
                }
                Err(error) => message.set(format!(
                    "Could not restore autosave: {error}. You can open a downloaded backup."
                )),
            },
            Ok(None) => {}
            Err(error) => message.set(error),
        }
        restored.set(true);
    });

    use_effect(move || {
        let revision = editor.read().revision;
        if !restored() {
            return;
        }
        save_status.set("Unsaved changes…".into());
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(900).await;
            if editor.peek().revision != revision {
                return;
            }
            let bytes = editor.peek().project.to_bytes();
            match bytes {
                Ok(bytes) => match storage::save(&bytes).await {
                    Ok(()) => {
                        if editor.peek().revision == revision {
                            save_status.set("Saved on this device".into());
                        }
                    }
                    Err(error) => save_status.set(error),
                },
                Err(_) => save_status.set("Fix validation errors to autosave".into()),
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

    // Terminating the native worker also stops pathological PDFs and frees its heap.
    use_effect(move || {
        if let Some(job) = pending() {
            spawn(async move {
                gloo_timers::future::TimeoutFuture::new(60_000).await;
                if pending.peek().as_ref().is_some_and(|p| p.id == job.id) {
                    pending.set(None);
                    bridge.set(None);
                    worker_generation += 1;
                    message.set("PDF operation timed out and was terminated. Try smaller artwork or fewer sheets.".into());
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
        let w = bg.width * scale;
        let h = bg.height * scale;
        ((page_width - w) / 2.0, (page_height - h) / 2.0, w, h)
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
    let free_placement = p.arrangement == Arrangement::Free;

    rsx! {
        div { class: "app-shell",
            header { class: "app-header",
                div { class: "brand", span { class: "brand-mark", "f" } div { strong { "folio" } span { "NUMBERING STUDIO" } } }
                div { class: "header-document",
                    input { class: "project-title", aria_label: "Project name", title: "{p.name}", maxlength: "200", value: "{p.name}", disabled: !restored(),
                        oninput: move |event| editor.write().edit(|p| p.name = event.value()) }
                    div { class: "header-status", span { class: "status-dot" } "Local-first · Built in Rust" }
                }
                div { class: "job-summary", strong { "{p.sequence.quantity}" } span { "items" } i {} strong { "{sheet_count}" } span { "sheets" } }
                div { class: "mobile-sidebar-controls",
                    button { class: "button outline", aria_expanded: mobile_panel() == Some("settings"), onclick: move |_| mobile_panel.set(if mobile_panel() == Some("settings") { None } else { Some("settings") }), "Settings" }
                    button { class: "button outline", aria_expanded: mobile_panel() == Some("fields"), onclick: move |_| mobile_panel.set(if mobile_panel() == Some("fields") { None } else { Some("fields") }), "Fields" }
                }
                div { class: "header-actions",
                    button { class: "button quiet", aria_label: "Open project", disabled: !restored() || busy,
                        onclick: move |_| browser::open_picker("project-file"), "Open" span { class: "action-detail", " project" } }
                    button { class: "button outline", aria_label: "Save project", disabled: !restored() || validation.is_some(), onclick: move |_| {
                        let result = editor.read().project.to_bytes().map_err(|e| e.to_string())
                            .and_then(|bytes| browser::download(&bytes, "project.folio.json", "application/json"));
                        message.set(result.err().unwrap_or_else(|| "Project backup downloaded, including its original artwork.".into()));
                    }, "Save" span { class: "action-detail", " project" } }
                }
            }
            main { class: "workspace", aria_busy: !restored(),
                if mobile_panel().is_some() {
                    button { class: "sidebar-backdrop", aria_label: "Close sidebar", onclick: move |_| mobile_panel.set(None) }
                }
                aside { class: if mobile_panel() == Some("settings") { "sidebar left-sidebar mobile-open" } else { "sidebar left-sidebar" }, aria_label: "Project settings",
                    SidebarSection { title: "Artwork & paper", number: "01",
                        button { class: "upload-button", disabled: busy || !restored() || !worker_available, onclick: move |_| browser::open_picker("artwork-file"),
                            span { class: "upload-icon", "+" } strong { if p.artwork.is_some() { "Replace artwork" } else { "Import artwork" } }
                            span { "PDF, PNG or JPEG · up to 20 MB" }
                        }
                        if let Some(art) = &p.artwork {
                            div { class: "artwork-info", span { "{art.name}" }
                                button { class: "text-button", disabled: busy, onclick: move |_| {
                                    let mut e = editor.write(); e.project.artwork = None; e.revision += 1;
                                    background.set(Background::default()); proof.set(None);
                                }, "Remove" }
                            }
                            if bg.pages > 1 {
                                NumberInput { label: "Artwork page", value: (art.page_index + 1) as f64, min: 1.0, max: bg.pages as f64,
                                    onchange: move |value| {
                                        editor.write().edit(|p| { if let Some(a) = &mut p.artwork { a.page_index = value as usize - 1; } });
                                        proof.set(None); send_job(bridge, pending, serial, editor, Operation::Inspect { adopt_page_size: false });
                                    }
                                }
                            }
                            p { class: "help", "Artwork fits proportionally on the full sheet. PDF import uses the selected page size." }
                        }
                        label { class: "control", span { "Paper preset" }
                            select { aria_label: "Paper preset", value: "custom", onchange: move |event| {
                                let size = match event.value().as_str() { "a4" => Some((210.0,297.0)), "letter" => Some((215.9,279.4)), "a3" => Some((297.0,420.0)), _ => None };
                                if let Some((w,h)) = size { editor.write().edit(|p| { p.page.width_mm = w; p.page.height_mm = h; clamp_fields(p); }); }
                            },
                                option { value: "custom", "Custom / imported" } option { value: "a4", "A4" }
                                option { value: "letter", "US Letter" } option { value: "a3", "A3" }
                            }
                        }
                        div { class: "two-columns",
                            NumberInput { label: "Width · mm", value: page_width, min: 10.0, max: 2000.0, step: 0.1,
                                onchange: move |value| editor.write().edit(|p| { p.page.width_mm = value; clamp_fields(p); }) }
                            NumberInput { label: "Height · mm", value: page_height, min: 10.0, max: 2000.0, step: 0.1,
                                onchange: move |value| editor.write().edit(|p| { p.page.height_mm = value; clamp_fields(p); }) }
                        }
                    }
                    SidebarSection { title: "Number sequence", number: "02",
                        div { class: "two-columns",
                            NumberInput { label: "Start at", value: p.sequence.start as f64, min: 0.0, max: 9_007_199_254_740_991.0,
                                onchange: move |value| editor.write().edit(|p| p.sequence.start = value as u64) }
                            NumberInput { label: "Quantity", value: p.sequence.quantity as f64, min: 1.0, max: 100_000.0,
                                onchange: move |value| { editor.write().edit(|p| p.sequence.quantity = value as u64); export_end.set(editor.read().project.sheet_count().min(500)); } }
                            NumberInput { label: "Increment", value: p.sequence.increment as f64, min: 1.0, max: 1_000_000.0,
                                onchange: move |value| editor.write().edit(|p| p.sequence.increment = value as u64) }
                            NumberInput { label: "Minimum digits", value: p.sequence.padding as f64, min: 0.0, max: 20.0,
                                onchange: move |value| editor.write().edit(|p| p.sequence.padding = value as usize) }
                            label { class: "control", span { "Prefix" } input { aria_label: "Prefix", maxlength: "100", value: "{p.sequence.prefix}",
                                oninput: move |event| editor.write().edit(|p| p.sequence.prefix = event.value()) } }
                            label { class: "control", span { "Suffix" } input { aria_label: "Suffix", maxlength: "100", value: "{p.sequence.suffix}",
                                oninput: move |event| editor.write().edit(|p| p.sequence.suffix = event.value()) } }
                        }
                        label { class: "control", span { "Numbering order" }
                            select { aria_label: "Numbering order", value: if p.sequence.order == Order::Sequential { "sequential" } else { "cut-stack" },
                                onchange: move |event| editor.write().edit(|p| p.sequence.order = if event.value() == "cut-stack" { Order::CutAndStack } else { Order::Sequential }),
                                option { value: "sequential", "Across each sheet" } option { value: "cut-stack", "Cut & stack" }
                            }
                        }
                        p { class: "help", if free_placement { "One sequence number per sheet; all fields on that sheet share it." } else if p.sequence.order == Order::CutAndStack { "Stack sheets in order, cut, then gather cells left-to-right, top-to-bottom. Unused tail cells stay blank." } else { "Numbers flow left-to-right, top-to-bottom on each sheet." } }
                        div { class: "sequence-sample", "{p.format_number(p.sequence.start)}" }
                    }
                    SidebarSection { title: "Sheet layout", number: "03",
                        label { class: "control", span { "Arrangement" }
                            select { aria_label: "Arrangement", value: if free_placement { "free" } else { "grid" },
                                onchange: move |event| {
                                    drag.set(None);
                                    editor.write().set_arrangement(if event.value() == "free" { Arrangement::Free } else { Arrangement::Grid });
                                },
                                option { value: "grid", "Grid" }
                                option { value: "free", "Free placement" }
                            }
                        }
                        if free_placement {
                            p { class: "help", "Position fields anywhere on the page. Each field has an X and Y guide; drag another field near a guide to align its anchor." }
                            label { class: "checkbox-control", input { r#type: "checkbox", checked: show_guides(),
                                onchange: move |event| show_guides.set(event.checked()) } "Show anchor guides" }
                            label { class: "checkbox-control", input { r#type: "checkbox", checked: snap_guides(), disabled: !show_guides(),
                                onchange: move |event| snap_guides.set(event.checked()) } "Snap to anchor guides" }
                            p { class: "help guide-help", "Guides are editing aids only; they never print. Snapping aligns positions, without linking future movement." }
                        } else {
                        div { class: "two-columns",
                            NumberInput { label: "Columns", value: p.grid.columns as f64, min: 1.0, max: 20.0,
                                onchange: move |value| editor.write().edit(|p| { p.grid.columns = value as u32; clamp_fields(p); }) }
                            NumberInput { label: "Rows", value: p.grid.rows as f64, min: 1.0, max: 20.0,
                                onchange: move |value| editor.write().edit(|p| { p.grid.rows = value as u32; clamp_fields(p); }) }
                            NumberInput { label: "Margin · mm", value: p.grid.margin_mm, min: 0.0, max: 200.0, step: 0.5,
                                onchange: move |value| editor.write().edit(|p| { p.grid.margin_mm = value; clamp_fields(p); }) }
                            NumberInput { label: "Gap · mm", value: p.grid.gap_mm, min: 0.0, max: 200.0, step: 0.5,
                                onchange: move |value| editor.write().edit(|p| { p.grid.gap_mm = value; clamp_fields(p); }) }
                        }
                        p { class: "help", "One item per cell. Every number field within a cell prints the same number." }
                        }
                    }
                }
                section { class: "canvas-panel",
                    div { class: "canvas-toolbar",
                        div { class: "segmented",
                            button { class: if current_proof.is_none() { "active" } else { "" }, onclick: move |_| proof.set(None), "Layout" }
                            button { class: if current_proof.is_some() { "active" } else { "" }, disabled: busy || validation.is_some() || !restored() || !worker_available, onclick: move |_| {
                                send_job(bridge, pending, serial, editor, Operation::Proof { sheet: active_sheet });
                                message.set("Rendering an output-PDF proof…".into());
                            }, "PDF proof" }
                        }
                        div { class: "toolbar-controls",
                            button { class: "icon-button", aria_label: "Undo", title: "Undo", disabled: !e.can_undo(), onclick: move |_| editor.write().undo(), "↶" }
                            button { class: "icon-button", aria_label: "Redo", title: "Redo", disabled: !e.can_redo(), onclick: move |_| editor.write().redo(), "↷" }
                            select { aria_label: "Zoom", value: "{zoom()}", onchange: move |event| { if let Ok(value) = event.value().parse() { zoom.set(value); } },
                                option { value: "1", "Fit to view" } option { value: "0.75", "75% of fit" } option { value: "0.5", "50% of fit" }
                            }
                        }
                    }
                    div { class: "canvas-scroll", style: "--page-ratio: {page_width / page_height}; --preview-scale: {zoom()};",
                        div { class: "paper-wrapper",
                            div { class: "paper-size-label", "{page_width:.1} × {page_height:.1} mm" }
                            svg { id: "page-editor", class: "paper", role: "img", "aria-label": "Numbering layout, drag a number to move it", tabindex: "0",
                                view_box: "0 0 {page_width} {page_height}", width: "100%", style: "aspect-ratio: {page_width} / {page_height}; touch-action: none;",
                                onpointermove: move |event| {
                                    if let Some(start) = drag() {
                                        let Some(rect) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("page-editor")).map(|e| e.get_bounding_client_rect()) else { return; };
                                        let pos = event.client_coordinates();
                                        let x = start.origin_x + (pos.x - start.client_x) * page_width / rect.width();
                                        let y = start.origin_y + (pos.y - start.client_y) * page_height / rect.height();
                                        let mut e = editor.write();
                                        let tolerance = (e.project.arrangement == Arrangement::Free && show_guides() && snap_guides())
                                            .then_some((6.0 * page_width / rect.width(), 6.0 * page_height / rect.height()));
                                        let targets: Vec<_> = e.project.fields.iter().filter(|f| !start.origins.iter().any(|(id,_,_)| *id == f.id)).cloned().collect();
                                        let (x,y) = guides::snap_position(&targets, start.id, (x,y), tolerance, snap());
                                        e.translate_from(&start.origins, x - start.origin_x, y - start.origin_y);
                                    }
                                },
                                onpointerup: move |_| drag.set(None), onpointercancel: move |_| drag.set(None), onlostpointercapture: move |_| drag.set(None),
                                onkeydown: move |event| {
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
                                            text { x: "0", y: "0", fill: "{number.field.color}", font_family: "Noto Sans", font_size: "{number.field.font_size_pt / PT_PER_MM}",
                                                text_anchor: match number.field.alignment { Alignment::Left => "start", Alignment::Center => "middle", Alignment::Right => "end" },
                                                "{number.text}" }
                                            if e.selected_ids.contains(&number.field.id) { circle { cx: "0", cy: "0", r: "0.9", fill: "#0c8a6b", stroke: "white", stroke_width: "0.3" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "canvas-footer",
                        div { class: "sheet-navigation",
                            button { class: "icon-button", aria_label: "Previous sheet", disabled: active_sheet == 0, onclick: move |_| sheet.set(active_sheet.saturating_sub(1)), "‹" }
                            span { "Sheet {active_sheet + 1} of {sheet_count}" }
                            button { class: "icon-button", aria_label: "Next sheet", disabled: active_sheet + 1 >= sheet_count, onclick: move |_| sheet.set((active_sheet + 1).min(sheet_count - 1)), "›" }
                        }
                        span { class: "help", if current_proof.is_some() { "Output-PDF proof" } else if free_placement { "Drag near a guide to align · Arrow keys to nudge" } else { "Drag to position · Arrow keys to nudge" } }
                    }
                }
                aside { class: if mobile_panel() == Some("fields") { "sidebar right-sidebar mobile-open" } else { "sidebar right-sidebar" }, aria_label: "Number fields and properties",
                    SidebarSection { title: "Number fields", badge: p.fields.len(),
                        p { class: "help", "Shift / Ctrl / ⌘ click to select multiple fields. Drag any selected field to move the group." }
                        div { class: "selection-actions",
                            span { class: "badge", "{selection_count} selected" }
                            button { class: "text-button", onclick: move |_| editor.write().select_all(), "Select all" }
                        }
                        div { class: "field-list",
                            for item in &p.fields {
                                div { class: "field-row",
                                input { r#type: "checkbox", aria_label: "Select Number {item.id}", checked: e.selected_ids.contains(&item.id),
                                    disabled: selection_count == 1 && e.selected_ids.contains(&item.id),
                                    onchange: { let id = item.id; move |_| editor.write().select(id, true) } }
                                button { class: if e.selected_ids.contains(&item.id) { "field-item active" } else { "field-item" }, aria_pressed: e.selected_ids.contains(&item.id),
                                    onclick: { let id = item.id; move |event: MouseEvent| editor.write().select(id, event.modifiers().shift() || event.modifiers().ctrl() || event.modifiers().meta()) },
                                    span { class: "field-symbol", "#" } span { "Number {item.id}" } small { "{item.font_size_pt:.0} pt" }
                                }
                                }
                            }
                        }
                        div { class: "two-columns",
                            button { class: "button outline", disabled: p.fields.len() >= 256, onclick: move |_| editor.write().add_field(), "Add" }
                            button { class: "button quiet", disabled: p.fields.len() <= 1, onclick: move |_| editor.write().delete_field(), "Delete" }
                        }
                    }
                    SidebarSection { title: "Field properties",
                        if selection_count > 1 { p { class: "help selection-summary", "Editing {selection_count} fields. X/Y move the group relative to Number {selected}; other properties apply to all selected fields." } }
                        p { class: "help", if free_placement { "Baseline anchor, measured from the page’s top-left corner." } else { "Baseline position, relative to each cell." } }
                        div { class: "two-columns",
                            NumberInput { label: "X · mm", value: field.x_mm, min: 0.0, max: cell_width.max(0.0), step: 0.1,
                                onchange: move |value| { let x = editor.read().field().map_or(0.0, |f| f.x_mm); editor.write().move_selection(value - x, 0.0); } }
                            NumberInput { label: "Y · mm", value: field.y_mm, min: 0.0, max: cell_height.max(0.0), step: 0.1,
                                onchange: move |value| { let y = editor.read().field().map_or(0.0, |f| f.y_mm); editor.write().move_selection(0.0, value - y); } }
                        }
                        label { class: "control", span { "Typeface" } select { disabled: true, option { "Noto Sans Regular" } } }
                        div { class: "two-columns",
                            NumberInput { label: "Size · pt", value: field.font_size_pt, mixed: mixed_size, min: 1.0, max: 300.0, step: 0.5,
                                onchange: move |value| editor.write().edit_field(|f| f.font_size_pt = value) }
                            label { class: "control", span { if mixed_color { "Colour (mixed)" } else { "Colour" } } input { r#type: "color", aria_label: "Field colour", value: "{field.color}",
                                oninput: move |event| editor.write().edit_field(|f| f.color = event.value()) } }
                        }
                        label { class: "control", span { "Rotation" } select { aria_label: "Rotation", value: if mixed_rotation { "mixed".to_string() } else { field.rotation.degrees().to_string() },
                            onchange: move |event| editor.write().edit_field(|f| f.rotation = match event.value().as_str() { "90" => Rotation::Deg90, "180" => Rotation::Deg180, "270" => Rotation::Deg270, _ => Rotation::Deg0 }),
                            if mixed_rotation { option { value: "mixed", disabled: true, "Mixed" } }
                            option { value: "0", "0°" } option { value: "90", "90°" } option { value: "180", "180°" } option { value: "270", "270°" }
                        } }
                        label { class: "control", span { "Alignment" } select { aria_label: "Alignment", value: if mixed_alignment { "mixed" } else { match field.alignment { Alignment::Left => "left", Alignment::Center => "center", Alignment::Right => "right" } },
                            onchange: move |event| editor.write().edit_field(|f| f.alignment = match event.value().as_str() { "center" => Alignment::Center, "right" => Alignment::Right, _ => Alignment::Left }),
                            if mixed_alignment { option { value: "mixed", disabled: true, "Mixed" } }
                            option { value: "left", "Left" } option { value: "center", "Centre" } option { value: "right", "Right" }
                        } }
                        label { class: "checkbox-control", input { r#type: "checkbox", checked: snap(), onchange: move |event| snap.set(event.checked()) } "Snap to 0.5 mm" }
                    }
                    SidebarSection { title: "Export your job", class: "export-panel",
                        p { class: "eyebrow", "READY FOR THE PRESS" }
                        p { class: "help", "Vector PDF · Embedded fonts · No uploads" }
                        div { class: "two-columns",
                            NumberInput { label: "First sheet", value: export_start() as f64, min: 1.0, max: sheet_count as f64,
                                onchange: move |value| export_start.set(value as usize) }
                            NumberInput { label: "Last sheet", value: export_end() as f64, min: 1.0, max: sheet_count as f64,
                                onchange: move |value| export_end.set(value as usize) }
                        }
                        button { class: "button primary full-width", disabled: busy || !restored() || validation.is_some() || !worker_available, onclick: move |_| {
                            let start = export_start(); let end = export_end();
                            if start == 0 || end < start || end > editor.read().project.sheet_count() || end - start + 1 > 500 {
                                message.set("Choose a valid range of at most 500 sheets. Export larger jobs in chunks.".into()); return;
                            }
                            message.set(format!("Generating sheets {start}–{end} in the PDF worker…"));
                            send_job(bridge, pending, serial, editor, Operation::Export { start_sheet: start - 1, end_sheet: end });
                        }, if busy { "Processing…" } else { "Download PDF ↓" } }
                        p { class: "help", "Up to 500 sheets per export. Print at actual size (100%). Single-sided output." }
                    }
                }
            }
            if let Some(error) = &validation { div { class: "validation", role: "alert", "{error}" } }
            footer { class: "status-bar",
                span { role: "status", aria_live: "polite", "{message}" }
                if busy {
                    button { class: "button outline", onclick: move |_| {
                        pending.set(None); bridge.set(None); worker_generation += 1;
                        message.set("PDF operation cancelled. The worker has been restarted.".into());
                    }, "Cancel operation" }
                } else if bridge.read().is_none() {
                    button { class: "button outline", onclick: move |_| worker_generation += 1, "Restart PDF worker" }
                }
                span { class: "save-status", "{save_status}" }
            }
            input { id: "artwork-file", class: "hidden", r#type: "file", accept: ".pdf,.png,.jpg,.jpeg", onchange: move |_| {
                spawn(async move {
                    let Some(file) = browser::selected_file("artwork-file") else { return; };
                    match browser::read_file(&file, numbering_core::MAX_ARTWORK_BYTES).await {
                        Ok(bytes) => {
                            let kind = if bytes.starts_with(b"%PDF-") { ArtworkKind::Pdf }
                                else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { ArtworkKind::Png }
                                else if bytes.starts_with(b"\xff\xd8\xff") { ArtworkKind::Jpeg }
                                else { message.set("Choose a PDF, PNG, or JPEG file.".into()); return; };
                            let mut p = editor.read().project.clone();
                            p.artwork = Some(Artwork { name: file.name(), kind, bytes, page_index: 0 });
                            editor.write().replace(p); background.set(Background::default()); proof.set(None);
                            send_job(bridge, pending, serial, editor, Operation::Inspect { adopt_page_size: true });
                            message.set("Opening artwork in the Rust PDF worker…".into());
                        }
                        Err(error) => message.set(error),
                    }
                });
            } }
            input { id: "project-file", class: "hidden", r#type: "file", accept: ".json,.folio", onchange: move |_| {
                spawn(async move {
                    let Some(file) = browser::selected_file("project-file") else { return; };
                    let result = browser::read_file(&file, numbering_core::MAX_PROJECT_BYTES).await
                        .and_then(|bytes| Project::from_bytes(&bytes).map_err(|e| e.to_string()));
                    match result {
                        Ok(p) => {
                            let has_artwork = p.artwork.is_some();
                            editor.write().replace(p); background.set(Background::default()); proof.set(None); sheet.set(0);
                            export_start.set(1); export_end.set(editor.read().project.sheet_count().min(500));
                            if has_artwork { send_job(bridge, pending, serial, editor, Operation::Inspect { adopt_page_size: false }); }
                            message.set("Project opened.".into());
                        }
                        Err(error) => message.set(error),
                    }
                });
            } }
        }
    }
}

#[component]
fn SidebarSection(
    title: &'static str,
    #[props(default = "")] number: &'static str,
    #[props(default = 0)] badge: usize,
    #[props(default = "")] class: &'static str,
    children: Element,
) -> Element {
    // Native disclosure keeps its own open/closed state across normal UI edits,
    // and provides keyboard and accessibility behaviour without JS handlers.
    rsx! {
        details { class: "panel {class}", open: true,
            summary { class: "panel-summary",
                h2 {
                    if !number.is_empty() { span { class: "section-number", "{number}" } }
                    "{title}"
                }
                if badge > 0 { span { class: "badge", "{badge}" } }
            }
            {children}
        }
    }
}

#[component]
fn NumberInput(
    label: &'static str,
    value: f64,
    min: f64,
    max: f64,
    #[props(default = 1.0)] step: f64,
    #[props(default = false)] mixed: bool,
    onchange: EventHandler<f64>,
) -> Element {
    rsx! {
        label { class: "control", span { "{label}" }
            input { r#type: "number", aria_label: "{label}", value: if mixed { String::new() } else { value.to_string() }, placeholder: "Mixed", min: "{min}", max: "{max}", step: "{step}",
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
