#[cfg(target_arch = "wasm32")]
fn main() {
    use numbering_worker::{Request, WorkerEvent};
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
                    let response = numbering_worker::process(request, |progress| {
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
fn main() {
    eprintln!("This entrypoint runs in a browser Web Worker. Use cargo xtask serve.");
}
