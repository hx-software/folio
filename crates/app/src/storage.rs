//! Local storage. The web build uses IndexedDB; the desktop build writes atomic files
//! under the platform's local application-data directory.
//!
//! Two kinds of state are kept:
//! - a working autosave, restored on launch, plus the id of the library project it came
//!   from (if any);
//! - the project library: named project snapshots that only change when the user saves.

use serde::{Deserialize, Serialize};

/// A saved project in the library, without its (potentially large) document bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    /// Unix time in milliseconds.
    pub updated_at: u64,
}

fn sort_summaries(list: &mut [ProjectSummary]) {
    list.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.name.cmp(&b.name))
    });
}

#[cfg(target_arch = "wasm32")]
pub fn now_millis() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Generates an identifier that is unique per process and stable across restarts.
pub fn new_project_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:013x}-{:04x}", now_millis(), counter & 0xffff)
}

#[cfg(target_arch = "wasm32")]
mod platform {
    use super::{ProjectSummary, now_millis, sort_summaries};
    use futures_channel::oneshot;
    use gloo_events::EventListener;
    use std::{cell::RefCell, rc::Rc};
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{IdbDatabase, IdbRequest, IdbTransactionMode};

    const AUTOSAVE: &str = "autosave";
    const AUTOSAVE_ID: &str = "autosave-id";
    const LIBRARY_INDEX: &str = "library-index";

    fn message(value: JsValue) -> String {
        format!("Browser storage error: {value:?}")
    }

    fn library_key(id: &str) -> String {
        format!("library:{id}")
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

    async fn get_key(db: &IdbDatabase, key: &str) -> Result<Option<Vec<u8>>, String> {
        let tx = db.transaction_with_str("projects").map_err(message)?;
        let request = tx
            .object_store("projects")
            .map_err(message)?
            .get(&JsValue::from_str(key))
            .map_err(message)?;
        let value = wait(&request).await?;
        if value.is_undefined() {
            Ok(None)
        } else {
            Ok(Some(js_sys::Uint8Array::new(&value).to_vec()))
        }
    }

    async fn put_key(db: &IdbDatabase, key: &str, bytes: &[u8]) -> Result<(), String> {
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
                        "Storage write failed (storage may be full). Export a project backup."
                            .into(),
                    ));
                }
            })
        };
        tx.object_store("projects")
            .map_err(message)?
            .put_with_key(&js_sys::Uint8Array::from(bytes), &JsValue::from_str(key))
            .map_err(message)?;
        let result = receiver
            .await
            .map_err(|_| "Storage request was interrupted.".to_string())?;
        drop((complete, abort));
        result
    }

    async fn delete_key(db: &IdbDatabase, key: &str) -> Result<(), String> {
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
                    let _ = sender.send(Err("Storage delete failed.".into()));
                }
            })
        };
        tx.object_store("projects")
            .map_err(message)?
            .delete(&JsValue::from_str(key))
            .map_err(message)?;
        let result = receiver
            .await
            .map_err(|_| "Storage request was interrupted.".to_string())?;
        drop((complete, abort));
        result
    }

    async fn read_library(db: &IdbDatabase) -> Result<Vec<ProjectSummary>, String> {
        let Some(bytes) = get_key(db, LIBRARY_INDEX).await? else {
            return Ok(Vec::new());
        };
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("Cannot read the project library: {error}"))
    }

    async fn write_library(db: &IdbDatabase, list: &[ProjectSummary]) -> Result<(), String> {
        let bytes = serde_json::to_vec(list)
            .map_err(|error| format!("Cannot write the project library: {error}"))?;
        put_key(db, LIBRARY_INDEX, &bytes).await
    }

    pub async fn load() -> Result<Option<Vec<u8>>, String> {
        let db = open().await?;
        let result = get_key(&db, AUTOSAVE).await;
        db.close();
        result
    }

    pub async fn load_current_id() -> Result<Option<String>, String> {
        let db = open().await?;
        let result = get_key(&db, AUTOSAVE_ID).await;
        db.close();
        match result? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "The saved project identifier is invalid.".to_string()),
            None => Ok(None),
        }
    }

    pub async fn save_current_id(id: Option<&str>) -> Result<(), String> {
        let db = open().await?;
        let result = match id {
            Some(id) => put_key(&db, AUTOSAVE_ID, id.as_bytes()).await,
            None => delete_key(&db, AUTOSAVE_ID).await,
        };
        db.close();
        result
    }

    pub async fn list_projects() -> Result<Vec<ProjectSummary>, String> {
        let db = open().await?;
        let result = read_library(&db).await;
        db.close();
        let mut list = result?;
        sort_summaries(&mut list);
        Ok(list)
    }

    pub async fn save_project(
        id: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<ProjectSummary, String> {
        if id.is_empty() {
            return Err("Invalid project identifier.".into());
        }
        let db = open().await?;
        let result = async {
            put_key(&db, &library_key(id), bytes).await?;
            let mut list = read_library(&db).await?;
            list.retain(|project| project.id != id);
            let summary = ProjectSummary {
                id: id.to_string(),
                name: name.to_string(),
                updated_at: now_millis(),
            };
            list.push(summary.clone());
            write_library(&db, &list).await?;
            Ok(summary)
        }
        .await;
        db.close();
        result
    }

    pub async fn load_project(id: &str) -> Result<Option<Vec<u8>>, String> {
        if id.is_empty() {
            return Err("Invalid project identifier.".into());
        }
        let db = open().await?;
        let result = get_key(&db, &library_key(id)).await;
        db.close();
        result
    }

    pub async fn delete_project(id: &str) -> Result<(), String> {
        if id.is_empty() {
            return Err("Invalid project identifier.".into());
        }
        let db = open().await?;
        let result = async {
            delete_key(&db, &library_key(id)).await?;
            let mut list = read_library(&db).await?;
            list.retain(|project| project.id != id);
            write_library(&db, &list).await
        }
        .await;
        db.close();
        result
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod platform {
    use super::{ProjectSummary, now_millis, sort_summaries};
    use futures_channel::oneshot;
    use std::{
        io::ErrorKind,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    fn app_dir() -> Result<PathBuf, String> {
        // An explicit directory keeps tests (and portable copies) away from the
        // user's real application data.
        if let Some(directory) = std::env::var_os("FOLIO_DATA_DIR") {
            return Ok(PathBuf::from(directory));
        }
        dirs::data_local_dir()
            .ok_or_else(|| "No local application-data directory is available.".to_string())
            .map(|directory| directory.join("Folio"))
    }

    fn autosave_path() -> Result<PathBuf, String> {
        Ok(app_dir()?.join("autosave.folio.json"))
    }

    fn current_id_path() -> Result<PathBuf, String> {
        Ok(app_dir()?.join("autosave.id"))
    }

    fn library_dir() -> Result<PathBuf, String> {
        Ok(app_dir()?.join("library"))
    }

    fn library_index_path() -> Result<PathBuf, String> {
        Ok(library_dir()?.join("library.json"))
    }

    fn library_path(id: &str) -> Result<PathBuf, String> {
        let valid = !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err("Invalid project identifier.".into());
        }
        Ok(library_dir()?.join(format!("{id}.folio.json")))
    }

    async fn run_blocking<T: Send + 'static>(
        task: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, String> {
        let (sender, receiver) = oneshot::channel();
        std::thread::spawn(move || {
            let _ = sender.send(task());
        });
        receiver
            .await
            .map_err(|_| "The storage operation was interrupted.".to_string())
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
        static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);
        let directory = path
            .parent()
            .ok_or_else(|| "Invalid storage path.".to_string())?;
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("Cannot create local storage: {error}"))?;
        let serial = TEMP_SERIAL.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_extension(format!("tmp-{}-{serial}", std::process::id()));
        std::fs::write(&temp, bytes).map_err(|error| format!("Cannot write file: {error}"))?;
        std::fs::rename(&temp, path).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            format!("Cannot finish saving: {error}")
        })
    }

    fn read_index(path: &Path) -> Result<Vec<ProjectSummary>, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("Cannot read the project library: {error}")),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(format!("Cannot read the project library: {error}")),
        }
    }

    fn write_index(path: &Path, list: &[ProjectSummary]) -> Result<(), String> {
        let bytes = serde_json::to_vec(list)
            .map_err(|error| format!("Cannot write the project library: {error}"))?;
        write_atomic(path, &bytes)
    }

    pub async fn load() -> Result<Option<Vec<u8>>, String> {
        let path = autosave_path()?;
        run_blocking(move || match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Cannot read local autosave: {error}")),
        })
        .await?
    }

    pub async fn load_current_id() -> Result<Option<String>, String> {
        let path = current_id_path()?;
        run_blocking(move || match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "The saved project identifier is invalid.".to_string()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Cannot read local project state: {error}")),
        })
        .await?
    }

    pub async fn save_current_id(id: Option<&str>) -> Result<(), String> {
        let path = current_id_path()?;
        let id = id.map(str::to_string);
        run_blocking(move || match id {
            Some(id) => write_atomic(&path, id.as_bytes()),
            None => match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("Cannot update local project state: {error}")),
            },
        })
        .await?
    }

    pub async fn list_projects() -> Result<Vec<ProjectSummary>, String> {
        let path = library_index_path()?;
        let mut list = run_blocking(move || read_index(&path)).await??;
        sort_summaries(&mut list);
        Ok(list)
    }

    pub async fn save_project(
        id: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<ProjectSummary, String> {
        let path = library_path(id)?;
        let index_path = library_index_path()?;
        let id = id.to_string();
        let name = name.to_string();
        let bytes = bytes.to_vec();
        run_blocking(move || {
            write_atomic(&path, &bytes)?;
            let mut list = read_index(&index_path)?;
            list.retain(|project| project.id != id);
            let summary = ProjectSummary {
                id: id.clone(),
                name: name.clone(),
                updated_at: now_millis(),
            };
            list.push(summary.clone());
            write_index(&index_path, &list)?;
            Ok(summary)
        })
        .await?
    }

    pub async fn load_project(id: &str) -> Result<Option<Vec<u8>>, String> {
        let path = library_path(id)?;
        run_blocking(move || match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Cannot read project: {error}")),
        })
        .await?
    }

    pub async fn delete_project(id: &str) -> Result<(), String> {
        let path = library_path(id)?;
        let index_path = library_index_path()?;
        let id = id.to_string();
        run_blocking(move || {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Cannot delete project: {error}")),
            }
            let mut list = read_index(&index_path)?;
            list.retain(|project| project.id != id);
            write_index(&index_path, &list)
        })
        .await?
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::future::Future;
        use std::path::PathBuf;
        use std::pin::pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        struct Unpark(std::thread::Thread);

        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }

        fn block_on<F: Future>(future: F) -> F::Output {
            let mut future = pin!(future);
            let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
            let mut context = Context::from_waker(&waker);
            loop {
                match future.as_mut().poll(&mut context) {
                    Poll::Ready(value) => return value,
                    Poll::Pending => std::thread::park(),
                }
            }
        }

        /// Points desktop storage at a temporary directory and removes it afterwards.
        struct TestDataDir(PathBuf);

        impl TestDataDir {
            fn new(label: &str) -> Self {
                let directory = std::env::temp_dir().join(format!(
                    "folio-storage-{label}-{}-{}",
                    std::process::id(),
                    now_millis()
                ));
                // SAFETY: this is the only test that reads or writes FOLIO_DATA_DIR.
                unsafe { std::env::set_var("FOLIO_DATA_DIR", &directory) };
                Self(directory)
            }
        }

        impl Drop for TestDataDir {
            fn drop(&mut self) {
                // SAFETY: see TestDataDir::new.
                unsafe { std::env::remove_var("FOLIO_DATA_DIR") };
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn library_round_trips_and_keeps_one_entry_per_id() {
            let _data = TestDataDir::new("library");

            for id in ["", "../escape", "a/b", "a b"] {
                assert!(library_path(id).is_err(), "{id:?} should be rejected");
            }
            assert!(library_path("project-1").is_ok());

            let first = block_on(save_project("project-1", "First project", b"one")).unwrap();
            assert_eq!(first.name, "First project");
            assert_eq!(
                block_on(load_project("project-1")).unwrap().as_deref(),
                Some(&b"one"[..])
            );

            block_on(save_current_id(Some("project-1"))).unwrap();
            assert_eq!(
                block_on(load_current_id()).unwrap().as_deref(),
                Some("project-1")
            );

            let list = block_on(list_projects()).unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].id, "project-1");
            assert_eq!(list[0].name, "First project");

            block_on(save_project("project-1", "Renamed", b"two")).unwrap();
            let list = block_on(list_projects()).unwrap();
            assert_eq!(list.len(), 1, "saving an existing id must update it");
            assert_eq!(list[0].name, "Renamed");
            assert_eq!(
                block_on(load_project("project-1")).unwrap().as_deref(),
                Some(&b"two"[..])
            );

            block_on(delete_project("project-1")).unwrap();
            assert!(block_on(list_projects()).unwrap().is_empty());
            assert_eq!(block_on(load_project("project-1")).unwrap(), None);

            block_on(save_current_id(None)).unwrap();
            assert_eq!(block_on(load_current_id()).unwrap(), None);
        }
    }
}

pub use platform::*;
