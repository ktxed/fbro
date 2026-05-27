use gdk::keys::constants::{Down, Multi_key, Up};
use gtk::glib::Propagation;
use gtk::prelude::WidgetExt;
use tauri::Emitter;

pub fn init_input_history(window: &gtk::ApplicationWindow, app_handle: tauri::AppHandle) {
    let compose_held = std::rc::Rc::new(std::cell::RefCell::new(false));

    {
        let compose_held = compose_held.clone();
        let app_handle = app_handle.clone();
        window.connect_key_press_event(move |_widget: &gtk::ApplicationWindow, key: &gdk::EventKey| {
            let keyval = key.keyval();
            if keyval == Multi_key {
                *compose_held.borrow_mut() = true;
                return Propagation::Stop;
            }
            if *compose_held.borrow() && keyval == Down {
                let _ = app_handle.emit("alt-arrow-down", ());
                return Propagation::Stop;
            }
            if *compose_held.borrow() && keyval == Up {
                let _ = app_handle.emit("alt-arrow-up", ());
                return Propagation::Stop;
            }
            Propagation::Proceed
        });
    }

    {
        let compose_held = compose_held.clone();
        window.connect_key_release_event(move |_widget: &gtk::ApplicationWindow, key: &gdk::EventKey| {
            if key.keyval() == Multi_key {
                *compose_held.borrow_mut() = false;
                return Propagation::Stop;
            }
            Propagation::Proceed
        });
    }
}
