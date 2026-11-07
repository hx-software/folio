//! Worker lifecycle. The web build starts a module Web Worker; the desktop build
//! starts its own executable with `--pdf-worker` and talks length-prefixed bincode
//! over stdin/stdout. Either way, dropping the client terminates the worker at once.

#[cfg(target_arch = "wasm32")]
mod platform {
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
            .ok_or_else(|| {
                "Cannot locate the application bundle to start the PDF worker.".to_string()
            })
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
}

#[cfg(not(target_arch = "wasm32"))]
mod platform {
    use futures_channel::mpsc;
    use futures_util::StreamExt;
    use numbering_worker::{Request, Response, WorkerEvent, stdio};
    use std::{
        process::{Child, ChildStdin, Command, Stdio},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    enum Message {
        Event(WorkerEvent),
        Error(String),
    }

    pub struct WorkerClient {
        child: Child,
        stdin: Mutex<ChildStdin>,
        stopped: Arc<AtomicBool>,
    }

    impl WorkerClient {
        pub fn new(
            response: impl Fn(Response) + 'static,
            error: impl Fn(String) + 'static,
        ) -> Result<Self, String> {
            let executable = std::env::current_exe()
                .map_err(|e| format!("Cannot locate the application: {e}"))?;
            let mut child = Command::new(executable)
                .arg("--pdf-worker")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| format!("Cannot start PDF worker: {e}"))?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| "Cannot write to the PDF worker.".to_string())?;
            let mut stdout = child
                .stdout
                .take()
                .ok_or_else(|| "Cannot read from the PDF worker.".to_string())?;

            let stopped = Arc::new(AtomicBool::new(false));
            let (sender, mut receiver) = mpsc::unbounded();
            {
                let sender = sender.clone();
                let stopped = stopped.clone();
                std::thread::Builder::new()
                    .name("folio-pdf-worker".into())
                    .spawn(move || {
                        while !stopped.load(Ordering::Relaxed) {
                            match stdio::read_frame(&mut stdout) {
                                Ok(Some(bytes)) => {
                                    match bincode::deserialize::<WorkerEvent>(&bytes) {
                                        Ok(event) => {
                                            if sender.unbounded_send(Message::Event(event)).is_err()
                                            {
                                                break;
                                            }
                                        }
                                        Err(error) => {
                                            let _ = sender.unbounded_send(Message::Error(format!(
                                                "Invalid PDF worker response: {error}"
                                            )));
                                            break;
                                        }
                                    }
                                }
                                Ok(None) => {
                                    if !stopped.load(Ordering::Relaxed) {
                                        let _ = sender.unbounded_send(Message::Error(
                                            "The PDF worker stopped unexpectedly. Cancel or retry to restart it; try smaller artwork if this repeats.".into()
                                        ));
                                    }
                                    break;
                                }
                                Err(error) => {
                                    if !stopped.load(Ordering::Relaxed) {
                                        let _ = sender.unbounded_send(Message::Error(format!(
                                            "The PDF worker stopped unexpectedly: {error}"
                                        )));
                                    }
                                    break;
                                }
                            }
                        }
                    })
                    .map_err(|e| format!("Cannot monitor the PDF worker: {e}"))?;
            }

            // Deliver worker events on the UI thread.
            dioxus::prelude::spawn(async move {
                while let Some(message) = receiver.next().await {
                    match message {
                        Message::Event(WorkerEvent::Ready) => {}
                        Message::Event(WorkerEvent::Response(event)) => response(event),
                        Message::Error(failure) => error(failure),
                    }
                }
            });

            Ok(Self {
                child,
                stdin: Mutex::new(stdin),
                stopped,
            })
        }

        pub fn send(&self, request: Request) -> Result<(), String> {
            let bytes = bincode::serialize(&request).map_err(|e| e.to_string())?;
            let mut stdin = self
                .stdin
                .lock()
                .map_err(|_| "The PDF worker is unavailable.".to_string())?;
            stdio::write_frame(&mut *stdin, &bytes).map_err(|e| format!("Cannot send PDF job: {e}"))
        }
    }

    impl Drop for WorkerClient {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Relaxed);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub use platform::WorkerClient;
