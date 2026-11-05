//! D-Bus endpoint the GNOME Shell extension calls whenever window focus changes.

use std::sync::{Arc, RwLock};

use anyhow::Result;
use zbus::blocking::{Connection, connection};

use crate::config::Focus;

pub type SharedFocus = Arc<RwLock<Focus>>;

pub const DEFAULT_BUS_NAME: &str = "io.github.clickshift.Daemon";
const OBJECT_PATH: &str = "/io/github/clickshift/Daemon";

struct FocusService {
    focus: SharedFocus,
    verbose: bool,
}

#[zbus::interface(name = "io.github.clickshift.Focus1")]
impl FocusService {
    fn set_focus(&self, app_id: String, wm_class: String) {
        if self.verbose {
            println!("focus: app_id=\"{app_id}\" wm_class=\"{wm_class}\"");
        }
        let mut current = self.focus.write().unwrap_or_else(|e| e.into_inner());
        *current = Focus { app_id, wm_class };
    }
}

/// The returned connection must stay alive for the endpoint to keep working.
pub fn serve(focus: SharedFocus, verbose: bool, bus_name: &str) -> Result<Connection> {
    let connection = connection::Builder::session()?
        .name(bus_name)?
        .serve_at(OBJECT_PATH, FocusService { focus, verbose })?
        .build()?;
    Ok(connection)
}

pub fn current(focus: &SharedFocus) -> Focus {
    focus.read().unwrap_or_else(|e| e.into_inner()).clone()
}
