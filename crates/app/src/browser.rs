use std::{cell::RefCell, collections::HashSet};
use wasm_bindgen::JsCast;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, HtmlInputElement, Url};

thread_local! {
    static LOADED_FONTS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub fn font_loaded(family: &str) -> bool {
    LOADED_FONTS.with(|fonts| fonts.borrow().contains(family))
}

/// Registers font bytes for the editor preview. Resolves only when the browser has
/// accepted and parsed the file, so a damaged font is rejected before it is adopted.
pub async fn ensure_font(family: &str, bytes: &[u8]) -> Result<(), String> {
    if font_loaded(family) {
        return Ok(());
    }
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("No browser document.")?;
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
    document
        .fonts()
        .add(&face)
        .map_err(|_| "The browser could not register this font.".to_string())?;
    LOADED_FONTS.with(|fonts| fonts.borrow_mut().insert(family.to_string()));
    Ok(())
}

/// Field ids whose number text intersects a rectangle given in client coordinates,
/// as a desktop marquee does. Uses rendered bounds, so rotation and fonts are honoured.
pub fn fields_in_client_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<u32> {
    let (left, right) = (x0.min(x1), x0.max(x1));
    let (top, bottom) = (y0.min(y1), y0.max(y1));
    let mut ids = Vec::new();
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
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

pub fn download(bytes: &[u8], filename: &str, mime: &str) -> Result<(), String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("No browser document.")?;
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
        gloo_timers::future::TimeoutFuture::new(30_000).await;
        let _ = Url::revoke_object_url(&url);
    });
    Ok(())
}

pub fn open_picker(id: &str) {
    if let Some(input) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
    {
        input.set_value("");
        input.click();
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
    if let Some(element) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("page-editor"))
    {
        let _ = element.set_pointer_capture(pointer_id);
        if let Some(svg) = element.dyn_ref::<web_sys::SvgElement>() {
            let _ = svg.focus();
        }
    }
}

pub fn selected_file(id: &str) -> Option<web_sys::File> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into::<HtmlInputElement>()
        .ok()?
        .files()?
        .get(0)
}

pub async fn read_file(file: &web_sys::File, limit: usize) -> Result<Vec<u8>, String> {
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
