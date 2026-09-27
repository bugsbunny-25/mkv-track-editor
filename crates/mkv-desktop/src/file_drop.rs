//! OS drag and drop: forwards winit's per-file hover/drop events to the
//! shared UI, which accepts MKV files and explains why anything else is
//! skipped. Dropped paths come from the OS, like native dialog paths, so
//! they're trusted local paths.

use mkv_ui::App;
use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
use slint::ComponentHandle;

pub fn install(app: &App) {
    let handle = app.handle();
    app.window().window().on_winit_window_event(move |_, event| match event {
        WindowEvent::HoveredFile(path) => {
            handle.drag_entered(&path.to_string_lossy());
            EventResult::Propagate
        }
        WindowEvent::HoveredFileCancelled => {
            handle.drag_left();
            EventResult::Propagate
        }
        WindowEvent::DroppedFile(path) => {
            handle.file_dropped(path.to_string_lossy().into_owned());
            EventResult::Propagate
        }
        _ => EventResult::Propagate,
    });
}
