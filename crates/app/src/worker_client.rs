//! Rust-authored worker lifecycle, with immediate termination on cancel or timeout.
use gloo_events::EventListener;
use numbering_worker::{Request, Response, WorkerEvent};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

pub struct WorkerClient {
    worker: Worker,
    ready: Rc<Cell<bool>>,
    queued: Rc<RefCell<Option<Request>>>,
    _message: EventListener,
    _error: EventListener,
}

/// The URL of the app's own JavaScript module, as injected by the Dioxus CLI. Starting a
/// module worker from it runs this same bundle, whose `main` then acts as the worker.
fn bundle_url() -> Result<String, String> {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector("script[type=module][src]")
                .ok()
                .flatten()
        })
        .and_then(|script| script.dyn_into::<web_sys::HtmlScriptElement>().ok())
        .map(|script| script.src())
        .ok_or_else(|| "Cannot locate the application bundle to start the PDF worker.".to_string())
}

fn post(worker: &Worker, request: &Request) -> Result<(), String> {
    let bytes = bincode::serialize(request).map_err(|e| e.to_string())?;
    let bytes = js_sys::Uint8Array::from(bytes.as_slice());
    let transfer = js_sys::Array::new();
    transfer.push(&bytes.buffer());
    worker
        .post_message_with_transfer(&bytes, &transfer)
        .map_err(|e| format!("Cannot send PDF job: {e:?}"))
}

impl WorkerClient {
    pub fn new(
        response: impl Fn(Response) + 'static,
        error: impl Fn(String) + 'static,
    ) -> Result<Self, String> {
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&bundle_url()?, &options)
            .map_err(|e| format!("Cannot start PDF worker: {e:?}"))?;
        let ready = Rc::new(Cell::new(false));
        let queued = Rc::new(RefCell::new(None::<Request>));
        let error = Rc::new(error);
        let message = {
            let worker = worker.clone();
            let ready = ready.clone();
            let queued = queued.clone();
            let error = error.clone();
            EventListener::new(worker.clone().as_ref(), "message", move |event| {
                let Some(event) = event.dyn_ref::<MessageEvent>() else {
                    return;
                };
                let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
                match bincode::deserialize::<WorkerEvent>(&bytes) {
                    Ok(WorkerEvent::Ready) => {
                        ready.set(true);
                        if let Some(request) = queued.borrow_mut().take()
                            && let Err(e) = post(&worker, &request)
                        {
                            error(e);
                        }
                    }
                    Ok(WorkerEvent::Response(value)) => response(value),
                    Err(e) => error(format!("Invalid PDF worker response: {e}")),
                }
            })
        };
        let failure = EventListener::new(worker.as_ref(), "error", move |_| {
            error("The PDF worker stopped unexpectedly. Cancel or retry to restart it; try smaller artwork if this repeats.".into());
        });
        Ok(Self {
            worker,
            ready,
            queued,
            _message: message,
            _error: failure,
        })
    }

    pub fn send(&self, request: Request) -> Result<(), String> {
        if self.ready.get() {
            post(&self.worker, &request)
        } else {
            *self.queued.borrow_mut() = Some(request);
            Ok(())
        }
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        self.worker.terminate();
    }
}
