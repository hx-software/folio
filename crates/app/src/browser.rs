//! Platform glue: native dialogs and WebView DOM access for the desktop build, and
//! the equivalent browser APIs for the web build. Callers stay platform-agnostic.
use std::{cell::RefCell, collections::HashSet, future::Future, pin::Pin};

thread_local! {
    static LOADED_FONTS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub fn font_loaded(family: &str) -> bool {
    LOADED_FONTS.with(|fonts| fonts.borrow().contains(family))
}

fn remember_font(family: &str) {
    LOADED_FONTS.with(|fonts| fonts.borrow_mut().insert(family.to_string()));
}

/// A rectangle in client (viewport) pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ClientRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug)]
pub enum FileKind {
    Artwork,
    Font,
    Project,
}

impl FileKind {
    pub fn max_bytes(self) -> usize {
        match self {
            Self::Artwork => numbering_core::MAX_ARTWORK_BYTES,
            Self::Font => numbering_core::MAX_FONT_BYTES,
            Self::Project => numbering_core::MAX_PROJECT_BYTES,
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn accept(self) -> &'static str {
        match self {
            Self::Artwork => ".pdf,.png,.jpg,.jpeg",
            Self::Font => ".ttf,.otf,.ttc",
            Self::Project => ".json,.folio",
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn title(self) -> &'static str {
        match self {
            Self::Artwork => "Choose artwork",
            Self::Font => "Choose a font",
            Self::Project => "Open a project",
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn filter_name(self) -> &'static str {
        match self {
            Self::Artwork => "Artwork",
            Self::Font => "Font",
            Self::Project => "Folio project",
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Artwork => &["pdf", "png", "jpg", "jpeg"],
            Self::Font => &["ttf", "otf", "ttc"],
            Self::Project => &["json", "folio"],
        }
    }
}

/// Bytes read by a platform file picker, already checked against the size limit.
pub struct PickedFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub type PickFuture = Pin<Box<dyn Future<Output = Result<Option<PickedFile>, String>>>>;

#[cfg(target_arch = "wasm32")]
mod platform {
    use super::{ClientRect, FileKind, PickFuture, PickedFile, font_loaded, remember_font};
    use futures_channel::oneshot;
    use gloo_events::EventListener;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;
    use wasm_bindgen::JsCast;
    use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, HtmlInputElement, Url};

    fn document() -> Result<web_sys::Document, String> {
        web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| "No browser document.".to_string())
    }

    fn editor_element() -> Option<web_sys::Element> {
        web_sys::window()?
            .document()?
            .get_element_by_id("page-editor")
    }

    /// Registers font bytes for the editor preview. Resolves only when the browser has
    /// accepted and parsed the file, so a damaged font is rejected before it is adopted.
    pub async fn ensure_font(family: &str, bytes: &[u8]) -> Result<(), String> {
        if font_loaded(family) {
            return Ok(());
        }
        let face = web_sys::FontFace::new_with_u8_array(family, bytes)
            .map_err(|_| "The browser could not read this font file.".to_string())?;
        let load = face
            .load()
            .map_err(|_| "The browser could not load this font file.".to_string())?;
        wasm_bindgen_futures::JsFuture::from(load)
            .await
            .map_err(|_| {
                "The browser rejected this font. It may be damaged or unsupported.".to_string()
            })?;
        document()?
            .fonts()
            .add(&face)
            .map_err(|_| "The browser could not register this font.".to_string())?;
        remember_font(family);
        Ok(())
    }

    pub fn editor_rect() -> Option<ClientRect> {
        let rect = editor_element()?.get_bounding_client_rect();
        Some(ClientRect {
            x: rect.x(),
            y: rect.y(),
            width: rect.width(),
            height: rect.height(),
        })
    }

    /// Field ids whose number text intersects a rectangle given in client coordinates,
    /// as a desktop marquee does. Uses rendered bounds, so rotation and fonts are honoured.
    pub fn fields_in_client_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<u32> {
        let (left, right) = (x0.min(x1), x0.max(x1));
        let (top, bottom) = (y0.min(y1), y0.max(y1));
        let mut ids = Vec::new();
        let Ok(document) = document() else {
            return ids;
        };
        let Ok(nodes) = document.query_selector_all("#page-editor .number-field text") else {
            return ids;
        };
        for index in 0..nodes.length() {
            let Some(element) = nodes
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            let rect = element.get_bounding_client_rect();
            if rect.right() >= left
                && rect.left() <= right
                && rect.bottom() >= top
                && rect.top() <= bottom
                && let Some(id) = element
                    .parent_element()
                    .and_then(|parent| parent.get_attribute("data-field-id"))
                    .and_then(|value| value.parse::<u32>().ok())
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        ids
    }

    /// Saves through the browser's download flow. Returns `true` when a download started.
    pub async fn download(bytes: &[u8], filename: &str, mime: &str) -> Result<bool, String> {
        let document = document()?;
        let parts = js_sys::Array::new();
        parts.push(&js_sys::Uint8Array::from(bytes));
        let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &{
            let options = BlobPropertyBag::new();
            options.set_type(mime);
            options
        })
        .map_err(|e| format!("Cannot create download: {e:?}"))?;
        let url = Url::create_object_url_with_blob(&blob)
            .map_err(|e| format!("Cannot create download URL: {e:?}"))?;
        let anchor = document
            .create_element("a")
            .map_err(|e| format!("{e:?}"))?
            .dyn_into::<HtmlAnchorElement>()
            .map_err(|_| "Invalid download element.")?;
        anchor.set_href(&url);
        anchor.set_download(filename);
        let body = document.body().ok_or("No page body.")?;
        body.append_child(&anchor).map_err(|e| format!("{e:?}"))?;
        anchor.click();
        anchor.remove();
        wasm_bindgen_futures::spawn_local(async move {
            futures_timer::Delay::new(Duration::from_secs(30)).await;
            let _ = Url::revoke_object_url(&url);
        });
        Ok(true)
    }

    fn selected_file(input: &HtmlInputElement) -> Option<web_sys::File> {
        input.files()?.get(0)
    }

    async fn read_file(file: &web_sys::File, limit: usize) -> Result<Vec<u8>, String> {
        if file.size() > limit as f64 {
            return Err(format!(
                "File exceeds the {} MB limit.",
                limit / 1024 / 1024
            ));
        }
        let buffer = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
            .await
            .map_err(|e| format!("Cannot read file: {e:?}"))?;
        Ok(js_sys::Uint8Array::new(&buffer).to_vec())
    }

    /// Adds a temporary `<input type=file>` and returns a future that resolves when the
    /// user chooses a file or cancels. The input is created and clicked synchronously so
    /// the browser still sees the button press as the initiating user gesture.
    pub fn pick_file(kind: FileKind) -> PickFuture {
        let setup = (|| {
            let document = document()?;
            let input = document
                .create_element("input")
                .map_err(|e| format!("Cannot create a file picker: {e:?}"))?
                .dyn_into::<HtmlInputElement>()
                .map_err(|_| "Cannot create a file picker.".to_string())?;
            input.set_type("file");
            input.set_accept(kind.accept());
            input.set_class_name("hidden");
            document
                .body()
                .ok_or("No page body.")?
                .append_child(&input)
                .map_err(|e| format!("Cannot show the file picker: {e:?}"))?;
            input.set_value("");

            let (sender, receiver) = oneshot::channel();
            let sender = Rc::new(RefCell::new(Some(sender)));
            let change = {
                let sender = sender.clone();
                let input = input.clone();
                let target = input.clone();
                EventListener::once(target.as_ref(), "change", move |_| {
                    let Some(file) = selected_file(&input) else {
                        if let Some(sender) = sender.borrow_mut().take() {
                            let _ = sender.send(Ok(None));
                        }
                        return;
                    };
                    let limit = kind.max_bytes();
                    let sender = sender.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = read_file(&file, limit).await.map(|bytes| {
                            Some(PickedFile {
                                name: file.name(),
                                bytes,
                            })
                        });
                        if let Some(sender) = sender.borrow_mut().take() {
                            let _ = sender.send(result);
                        }
                    });
                })
            };
            let cancel = {
                let sender = sender.clone();
                EventListener::once(input.as_ref(), "cancel", move |_| {
                    if let Some(sender) = sender.borrow_mut().take() {
                        let _ = sender.send(Ok(None));
                    }
                })
            };
            input.click();
            Ok((input, receiver, (change, cancel)))
        })();
        match setup {
            Ok((input, receiver, listeners)) => Box::pin(async move {
                let _listeners = listeners;
                let result = receiver
                    .await
                    .map_err(|_| "The file picker was interrupted.".to_string())?;
                input.remove();
                result
            }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    /// Sets an input's displayed value directly. A controlled input whose state did not
    /// change (for example after clamping) would otherwise keep showing what was typed.
    pub fn set_input_value(id: &str, value: &str) {
        if let Some(input) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(id))
            .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
        {
            input.set_value(value);
        }
    }

    pub fn capture_editor_pointer(pointer_id: i32) {
        if let Some(element) = editor_element() {
            let _ = element.set_pointer_capture(pointer_id);
            if let Some(svg) = element.dyn_ref::<web_sys::SvgElement>() {
                let _ = svg.focus();
            }
        }
    }

    pub fn log_debug(message: &str) {
        web_sys::console::log_1(&message.into());
    }

    pub fn log_warn(message: &str) {
        web_sys::console::warn_1(&message.into());
    }

    pub fn log_error(message: &str) {
        web_sys::console::error_1(&message.into());
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod platform {
    use super::{ClientRect, FileKind, PickFuture, PickedFile, font_loaded, remember_font};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use futures_channel::oneshot;
    use std::sync::{OnceLock, RwLock};
    use std::time::Duration;

    fn editor_cache() -> &'static RwLock<Option<ClientRect>> {
        static CACHE: OnceLock<RwLock<Option<ClientRect>>> = OnceLock::new();
        CACHE.get_or_init(|| RwLock::new(None))
    }

    fn field_cache() -> &'static RwLock<Vec<(u32, ClientRect)>> {
        static CACHE: OnceLock<RwLock<Vec<(u32, ClientRect)>>> = OnceLock::new();
        CACHE.get_or_init(|| RwLock::new(Vec::new()))
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
            .map_err(|_| "The operation was interrupted.".to_string())
    }

    /// Registers font bytes with the WebView's font set. The Rust worker has already
    /// validated the file; this only affects the live editor preview.
    pub async fn ensure_font(family: &str, bytes: &[u8]) -> Result<(), String> {
        if font_loaded(family) {
            return Ok(());
        }
        let family_json = serde_json::to_string(family)
            .map_err(|error| format!("Invalid font family: {error}"))?;
        let encoded = serde_json::to_string(&STANDARD.encode(bytes))
            .map_err(|error| format!("Cannot encode font: {error}"))?;
        let script = format!(
            r#"
            try {{
                const raw = atob({encoded});
                const data = new Uint8Array(raw.length);
                for (let i = 0; i < raw.length; i++) data[i] = raw.charCodeAt(i);
                const face = new FontFace({family_json}, data);
                await face.load();
                document.fonts.add(face);
                return null;
            }} catch (error) {{
                return String(error && error.message ? error.message : error);
            }}
            "#
        );
        let error = dioxus::document::eval(&script)
            .join::<Option<String>>()
            .await
            .map_err(|error| format!("The preview could not register this font: {error}"))?;
        if let Some(error) = error {
            return Err(format!(
                "The preview rejected this font. It may be damaged or unsupported. ({error})"
            ));
        }
        remember_font(family);
        Ok(())
    }

    /// Returns the last polled canvas rectangle. Pointer handlers run synchronously, so
    /// they cannot await a WebView round-trip; `refresh_metrics` keeps this cache warm.
    pub fn editor_rect() -> Option<ClientRect> {
        editor_cache().read().ok().and_then(|rect| *rect)
    }

    pub fn fields_in_client_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<u32> {
        let (left, right) = (x0.min(x1), x0.max(x1));
        let (top, bottom) = (y0.min(y1), y0.max(y1));
        let Ok(fields) = field_cache().read() else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for (id, rect) in fields.iter() {
            if rect.x + rect.width >= left
                && rect.x <= right
                && rect.y + rect.height >= top
                && rect.y <= bottom
                && !ids.contains(id)
            {
                ids.push(*id);
            }
        }
        ids
    }

    /// Refreshes the cached canvas and number-text rectangles from the WebView DOM.
    pub async fn refresh_metrics() {
        #[derive(serde::Deserialize)]
        struct FieldRect {
            id: u32,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
        }

        #[derive(serde::Deserialize)]
        struct Metrics {
            editor: Option<ClientRect>,
            fields: Vec<FieldRect>,
        }

        const SCRIPT: &str = r#"
            const editor = document.getElementById('page-editor');
            if (!editor) return { editor: null, fields: [] };
            const bounds = editor.getBoundingClientRect();
            const fields = [];
            for (const text of editor.querySelectorAll('.number-field text')) {
                const group = text.parentElement;
                const id = Number(group && group.dataset ? group.dataset.fieldId : NaN);
                if (!Number.isInteger(id)) continue;
                const rect = text.getBoundingClientRect();
                fields.push({ id, x: rect.x, y: rect.y, width: rect.width, height: rect.height });
            }
            return {
                editor: { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height },
                fields
            };
        "#;
        let Ok(metrics) = dioxus::document::eval(SCRIPT).join::<Metrics>().await else {
            // The DOM is not ready yet, or the window is being torn down.
            return;
        };
        if let Ok(mut cache) = editor_cache().write() {
            *cache = metrics.editor;
        }
        let fields = metrics
            .fields
            .into_iter()
            .map(|rect| {
                (
                    rect.id,
                    ClientRect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: rect.height,
                    },
                )
            })
            .collect();
        if let Ok(mut cache) = field_cache().write() {
            *cache = fields;
        }
    }

    /// Saves through a native dialog. Returns `false` when the user cancels.
    pub async fn download(bytes: &[u8], filename: &str, mime: &str) -> Result<bool, String> {
        let mut dialog = rfd::AsyncFileDialog::new().set_file_name(filename);
        if let Some(directory) = dirs::download_dir() {
            dialog = dialog.set_directory(directory);
        }
        dialog = match mime {
            "application/pdf" => dialog.add_filter("PDF", &["pdf"]),
            _ => dialog.add_filter("Folio project", &["json", "folio"]),
        };
        let Some(handle) = dialog.save_file().await else {
            return Ok(false);
        };
        let path = handle.path().to_path_buf();
        let bytes = bytes.to_vec();
        run_blocking(move || std::fs::write(&path, &bytes))
            .await?
            .map_err(|error| format!("Cannot save file: {error}"))?;
        Ok(true)
    }

    /// Opens a native file picker and reads the selection on a background thread.
    pub fn pick_file(kind: FileKind) -> PickFuture {
        Box::pin(async move {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(kind.title());
            if let Some(directory) = dirs::download_dir() {
                dialog = dialog.set_directory(directory);
            }
            dialog = dialog.add_filter(kind.filter_name(), kind.extensions());
            let Some(handle) = dialog.pick_file().await else {
                return Ok(None);
            };
            let path = handle.path().to_path_buf();
            let limit = kind.max_bytes();
            let size = {
                let path = path.clone();
                run_blocking(move || std::fs::metadata(&path))
                    .await?
                    .map_err(|error| format!("Cannot read file: {error}"))?
                    .len()
            };
            if size > limit as u64 {
                return Err(format!(
                    "File exceeds the {} MB limit.",
                    limit / 1024 / 1024
                ));
            }
            let bytes = run_blocking(move || std::fs::read(&path))
                .await?
                .map_err(|error| format!("Cannot read file: {error}"))?;
            if bytes.len() > limit {
                return Err(format!(
                    "File exceeds the {} MB limit.",
                    limit / 1024 / 1024
                ));
            }
            Ok(Some(PickedFile {
                name: handle.file_name(),
                bytes,
            }))
        })
    }

    /// Mirrors web `set_input_value`; the DOM is only reachable through an eval on desktop.
    pub fn set_input_value(id: &str, value: &str) {
        let (Ok(id), Ok(value)) = (serde_json::to_string(id), serde_json::to_string(value)) else {
            return;
        };
        let _ = dioxus::document::eval(&format!(
            "const input = document.getElementById({id}); if (input) input.value = {value};"
        ));
    }

    pub fn capture_editor_pointer(pointer_id: i32) {
        let _ = dioxus::document::eval(&format!(
            "const editor = document.getElementById('page-editor');
             if (editor) {{
                 try {{ editor.setPointerCapture({pointer_id}); }} catch (_) {{}}
                 if (editor.focus) editor.focus();
             }}"
        ));
    }

    pub fn log_debug(message: &str) {
        println!("{message}");
    }

    pub fn log_warn(message: &str) {
        eprintln!("warning: {message}");
    }

    pub fn log_error(message: &str) {
        eprintln!("error: {message}");
    }

    /// Keeps the metrics cache fresh. Called from a UI-side polling loop.
    pub const METRICS_INTERVAL: Duration = Duration::from_millis(100);
}

pub use platform::*;
