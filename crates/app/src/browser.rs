use wasm_bindgen::JsCast;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, HtmlInputElement, Url};

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
