//! Binary worker protocol, shared by the Dioxus client and Rust worker entrypoint.
use numbering_core::Project;
use numbering_pdf::{ArtworkInfo, FontInfo, Preview};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub project: Project,
    pub operation: Operation,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Operation {
    Inspect {
        adopt_page_size: bool,
    },
    Background,
    Proof {
        sheet: usize,
    },
    Export {
        start_sheet: usize,
        end_sheet: usize,
    },
    /// Validates the font in `project.font` before it is adopted.
    InspectFont,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: Result<Output, String>,
}

#[derive(Serialize, Deserialize)]
pub enum WorkerEvent {
    Ready,
    Response(Response),
}

#[derive(Serialize, Deserialize)]
pub enum Output {
    Inspected(ArtworkInfo),
    FontChecked(FontInfo),
    Preview(Preview),
    Progress { completed: usize, total: usize },
    Exported(Vec<u8>),
}

pub fn process(request: Request, mut progress: impl FnMut(Response)) -> Response {
    let p = &request.project;
    let result = match request.operation {
        Operation::Inspect { .. } => numbering_pdf::inspect_artwork(p).map(Output::Inspected),
        Operation::InspectFont => p
            .font
            .as_ref()
            .ok_or_else(|| numbering_core::Error("No font was provided.".into()))
            .and_then(|font| numbering_pdf::inspect_font(&font.bytes))
            .map(Output::FontChecked),
        Operation::Background => numbering_pdf::background(p, 1100).map(Output::Preview),
        Operation::Proof { sheet } => numbering_pdf::export(p, sheet..sheet + 1)
            .and_then(|bytes| numbering_pdf::render_pdf(&bytes, 0, 1100))
            .map(Output::Preview),
        Operation::Export {
            start_sheet,
            end_sheet,
        } => numbering_pdf::export_with_progress(p, start_sheet..end_sheet, |completed, total| {
            progress(Response {
                id: request.id,
                result: Ok(Output::Progress { completed, total }),
            });
        })
        .map(Output::Exported),
    }
    .map_err(|e| e.to_string());
    Response {
        id: request.id,
        result,
    }
}

/// The bundled typeface, re-exported so the UI can register it for its preview.
pub use numbering_pdf::FONT_BYTES;

/// Runs the PDF job loop inside a dedicated Web Worker.
///
/// One bundle serves both the UI and the worker: the app's `main` calls this when it
/// finds itself without a `window`, so no second binary or loader script is needed.
#[cfg(target_arch = "wasm32")]
pub fn run_worker() {
    use wasm_bindgen::{JsCast, closure::Closure};
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    fn send(scope: &DedicatedWorkerGlobalScope, event: WorkerEvent) {
        if let Ok(bytes) = bincode::serialize(&event) {
            let bytes = js_sys::Uint8Array::from(bytes.as_slice());
            let transfer = js_sys::Array::new();
            transfer.push(&bytes.buffer());
            let _ = scope.post_message_with_transfer(&bytes, &transfer);
        }
    }
    let handler = {
        let scope = scope.clone();
        Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
            match bincode::deserialize::<Request>(&bytes) {
                Ok(request) => {
                    let response = process(request, |progress| {
                        send(&scope, WorkerEvent::Response(progress))
                    });
                    send(&scope, WorkerEvent::Response(response));
                }
                Err(error) => {
                    web_sys::console::error_1(&format!("Invalid PDF request: {error}").into())
                }
            }
        })
    };
    scope.set_onmessage(Some(handler.as_ref().unchecked_ref()));
    handler.forget();
    send(&scope, WorkerEvent::Ready);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run_worker() {
    eprintln!("The PDF worker only runs inside a browser Web Worker.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_protocol_round_trips_and_returns_export_progress() {
        let request = Request {
            id: 42,
            project: Project::default(),
            operation: Operation::Export {
                start_sheet: 0,
                end_sheet: 2,
            },
        };
        let request = bincode::deserialize(&bincode::serialize(&request).unwrap()).unwrap();
        let mut updates = Vec::new();
        let response = process(request, |progress| updates.push(progress));
        assert_eq!(response.id, 42);
        assert_eq!(updates.len(), 2);
        assert!(
            matches!(&response.result, Ok(Output::Exported(bytes)) if bytes.starts_with(b"%PDF-"))
        );
        let bytes = bincode::serialize(&WorkerEvent::Response(response)).unwrap();
        assert!(matches!(
            bincode::deserialize::<WorkerEvent>(&bytes).unwrap(),
            WorkerEvent::Response(Response { id: 42, .. })
        ));
    }

    #[test]
    fn binary_protocol_carries_blobs_and_validates_fonts() {
        let mut project = Project::default();
        project.artwork = Some(numbering_core::Artwork {
            name: "art.png".into(),
            kind: numbering_core::ArtworkKind::Png,
            bytes: (0..=255).collect(),
            page_index: 0,
            scale_percent: 100.0,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
        });
        project.font = Some(numbering_core::CustomFont::new(
            "noto.ttf",
            numbering_pdf::FONT_BYTES.to_vec(),
        ));
        let request = Request {
            id: 3,
            project: project.clone(),
            operation: Operation::InspectFont,
        };
        let bytes = bincode::serialize(&request).unwrap();
        assert!(bytes.len() < numbering_pdf::FONT_BYTES.len() + 1024);
        let decoded: Request = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.project, project);
        let response = process(decoded, |_| panic!("font checks do not report progress"));
        assert!(matches!(
            response.result,
            Ok(Output::FontChecked(FontInfo { family: Some(ref name) })) if name == "Noto Sans"
        ));
        let mut missing = Project::default();
        missing.font = Some(numbering_core::CustomFont::new("junk.ttf", vec![1, 2, 3]));
        let response = process(
            Request {
                id: 4,
                project: missing,
                operation: Operation::InspectFont,
            },
            |_| {},
        );
        assert!(response.result.is_err());
        let response = process(
            Request {
                id: 5,
                project: Project::default(),
                operation: Operation::InspectFont,
            },
            |_| {},
        );
        assert!(response.result.is_err());
    }

    #[test]
    fn invalid_jobs_return_errors_without_progress() {
        let request = Request {
            id: 7,
            project: Project::default(),
            operation: Operation::Export {
                start_sheet: 0,
                end_sheet: 999,
            },
        };
        let response = process(request, |_| {
            panic!("Invalid job should not report progress")
        });
        assert_eq!(response.id, 7);
        assert!(response.result.is_err());
    }
}
