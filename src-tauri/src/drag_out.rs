use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use tauri::Emitter;

#[derive(Clone, Debug, serde::Serialize)]
struct DragOutResult {
    success: bool,
}

struct PendingDrag {
    paths: Vec<String>,
}

struct DragOutState {
    pending: RefCell<Option<PendingDrag>>,
    active_paths: RefCell<Vec<String>>,
}

pub struct DragOutHandle {
    state: Rc<DragOutState>,
}

impl DragOutHandle {
    pub fn request_drag_out(&self, paths: Vec<String>) {
        *self.state.pending.borrow_mut() = Some(PendingDrag { paths });
    }
}

pub fn init_drag_out(
    window: &gtk::ApplicationWindow,
    app_handle: tauri::AppHandle,
) -> DragOutHandle {
    let state = Rc::new(DragOutState {
        pending: RefCell::new(None),
        active_paths: RefCell::new(Vec::new()),
    });

    let state_motion = state.clone();
    let app_handle_motion = app_handle.clone();
    window.connect_motion_notify_event(move |widget, event| {
        let pending = state_motion.pending.borrow_mut().take();
        if let Some(pending) = pending {
            if widget.is_realized() {
                let (cx, cy) = event.coords().unwrap_or((0.0, 0.0));
                let button = event.button().unwrap_or(1);

                *state_motion.active_paths.borrow_mut() = pending.paths.clone();

                let target_entries = [gtk::TargetEntry::new(
                    "text/uri-list",
                    gtk::TargetFlags::OTHER_APP,
                    0,
                )];
                widget.drag_source_set(
                    gdk::ModifierType::BUTTON1_MASK,
                    &target_entries,
                    gdk::DragAction::COPY,
                );
                widget.drag_source_add_uri_targets();

                let ctx = widget.drag_begin_with_coordinates(
                    &gtk::TargetList::new(&target_entries),
                    gdk::DragAction::COPY,
                    button as i32,
                    Some(event),
                    cx as i32,
                    cy as i32,
                );

                if ctx.is_none() {
                    eprintln!("[fbro] drag_begin_with_coordinates returned None — drag-out failed");
                    *state_motion.active_paths.borrow_mut() = Vec::new();
                    widget.drag_source_unset();
                    let _ = app_handle_motion
                        .emit("drag-out-complete", DragOutResult { success: false });
                }
            }
        }

        gtk::glib::Propagation::Proceed
    });

    let state_data = state.clone();
    window.connect_drag_data_get(move |_widget, _ctx, data, _info, _time| {
        let paths = state_data.active_paths.borrow();
        if paths.is_empty() {
            return;
        }

        let uri_strings: Vec<String> = paths
            .iter()
            .map(|path| gio::File::for_path(path).uri().to_string())
            .collect();
        let uri_refs: Vec<&str> = uri_strings.iter().map(|s| s.as_str()).collect();
        data.set_uris(&uri_refs);
    });

    let state_end = state.clone();
    let app_handle_end = app_handle.clone();
    window.connect_drag_end(move |widget, ctx| {
        let succeeded = ctx.drag_drop_succeeded();
        *state_end.active_paths.borrow_mut() = Vec::new();
        widget.drag_source_unset();

        let _ = app_handle_end.emit("drag-out-complete", DragOutResult { success: succeeded });
    });

    let state_failed = state.clone();
    let app_handle_failed = app_handle.clone();
    window.connect_drag_failed(move |_widget, _ctx, _result| {
        *state_failed.active_paths.borrow_mut() = Vec::new();
        let _ = app_handle_failed.emit("drag-out-complete", DragOutResult { success: false });
        gtk::glib::Propagation::Stop
    });

    DragOutHandle { state }
}

thread_local! {
    static DRAG_OUT_HANDLE: RefCell<Option<DragOutHandle>> = RefCell::new(None);
}

pub fn set_drag_out_handle(handle: DragOutHandle) {
    DRAG_OUT_HANDLE.with(|h| {
        *h.borrow_mut() = Some(handle);
    });
}

pub fn request_drag_out(paths: Vec<String>) -> Result<(), String> {
    DRAG_OUT_HANDLE.with(|h| match h.borrow().as_ref() {
        Some(handle) => {
            handle.request_drag_out(paths);
            Ok(())
        }
        None => Err("Drag-out not initialized".to_string()),
    })
}

pub fn start_drag_out(paths: Vec<String>) -> Result<(), String> {
    let paths_clone = paths.clone();
    gtk::glib::MainContext::default().invoke(move || {
        if let Err(e) = request_drag_out(paths_clone) {
            eprintln!("[fbro] drag-out request failed: {}", e);
        }
    });
    Ok(())
}
