//! Finds mice among the input devices this user can read.

use evdev::{Device, KeyCode, RelativeAxisCode};

/// Names starting with this are clickshift's own virtual devices.
pub const OWN_DEVICE_PREFIX: &str = "clickshift";

pub fn is_mouse(device: &Device) -> bool {
    let has_button = device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
    let has_motion = device.supported_relative_axes().is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    });
    has_button && has_motion
}

pub fn device_name(device: &Device) -> &str {
    device.name().unwrap_or("").trim()
}

/// Sorted, without duplicates or clickshift's own devices.
pub fn readable_mouse_names() -> Vec<String> {
    let mut names: Vec<String> = evdev::enumerate()
        .map(|(_, device)| device)
        .filter(is_mouse)
        .map(|device| device_name(&device).to_owned())
        .filter(|name| !name.starts_with(OWN_DEVICE_PREFIX))
        .collect();
    names.sort();
    names.dedup();
    names
}
