//! IndexedDB bindings authored in Rust; failed autosaves are surfaced to the UI.
use futures_channel::oneshot;
use gloo_events::EventListener;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{IdbDatabase, IdbRequest, IdbTransactionMode};

fn message(value: JsValue) -> String {
    format!("Browser storage error: {value:?}")
}

async fn wait(request: &IdbRequest) -> Result<JsValue, String> {
    let (tx, rx) = oneshot::channel();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let success = {
        let tx = tx.clone();
        let request = request.clone();
        EventListener::once(request.clone().as_ref(), "success", move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(request.result().map_err(message));
            }
        })
    };
    let failure = {
        let tx = tx.clone();
        EventListener::once(request.as_ref(), "error", move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Err("IndexedDB request failed.".into()));
            }
        })
    };
    let result = rx
        .await
        .map_err(|_| "Storage request was interrupted.".to_string())?;
    drop((success, failure));
    result
}

async fn open() -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .ok_or("No browser window.")?
        .indexed_db()
        .map_err(message)?
        .ok_or("IndexedDB is unavailable; use downloadable project backups.")?;
    let request = factory
        .open_with_u32("folio-numbering", 1)
        .map_err(message)?;
    let upgrade = {
        let request = request.clone();
        EventListener::new(request.clone().as_ref(), "upgradeneeded", move |_| {
            if let Ok(value) = request.result()
                && let Ok(db) = value.dyn_into::<IdbDatabase>()
                && !db.object_store_names().contains("projects")
            {
                let _ = db.create_object_store("projects");
            }
        })
    };
    let result = wait(request.as_ref())
        .await?
        .dyn_into::<IdbDatabase>()
        .map_err(message);
    drop(upgrade);
    result
}

pub async fn load() -> Result<Option<Vec<u8>>, String> {
    let db = open().await?;
    let tx = db.transaction_with_str("projects").map_err(message)?;
    let request = tx
        .object_store("projects")
        .map_err(message)?
        .get(&"autosave".into())
        .map_err(message)?;
    let result = wait(&request).await;
    db.close();
    let value = result?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(js_sys::Uint8Array::new(&value).to_vec()))
    }
}

pub async fn save(bytes: &[u8]) -> Result<(), String> {
    let db = open().await?;
    let tx = db
        .transaction_with_str_and_mode("projects", IdbTransactionMode::Readwrite)
        .map_err(message)?;
    let (sender, receiver) = oneshot::channel();
    let sender = Rc::new(RefCell::new(Some(sender)));
    let complete = {
        let sender = sender.clone();
        EventListener::once(tx.as_ref(), "complete", move |_| {
            if let Some(sender) = sender.borrow_mut().take() {
                let _ = sender.send(Ok(()));
            }
        })
    };
    let abort = {
        let sender = sender.clone();
        EventListener::once(tx.as_ref(), "abort", move |_| {
            if let Some(sender) = sender.borrow_mut().take() {
                let _ = sender.send(Err(
                    "Autosave failed (storage may be full). Download a project backup.".into(),
                ));
            }
        })
    };
    let bytes = js_sys::Uint8Array::from(bytes);
    tx.object_store("projects")
        .map_err(message)?
        .put_with_key(&bytes, &"autosave".into())
        .map_err(message)?;
    let result = receiver
        .await
        .map_err(|_| "Autosave was interrupted.".to_string())?;
    drop((complete, abort));
    db.close();
    result
}
