//! Binary worker protocol, shared by the Dioxus client and Rust worker entrypoint.
use numbering_core::Project;
use numbering_pdf::{ArtworkInfo, Preview};
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
    Preview(Preview),
    Progress { completed: usize, total: usize },
    Exported(Vec<u8>),
}

pub fn process(request: Request, mut progress: impl FnMut(Response)) -> Response {
    let p = &request.project;
    let result = match request.operation {
        Operation::Inspect { .. } => numbering_pdf::inspect_artwork(p).map(Output::Inspected),
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
