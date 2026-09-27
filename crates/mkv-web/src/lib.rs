mod backend;

use backend::WebBackend;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();

    if let Err(e) = mkv_ui::launch(WebBackend) {
        web_sys::console::error_1(&format!("failed to launch UI: {e}").into());
    }
}
