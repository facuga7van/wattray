mod bt;
mod hidpp;
mod razer;
mod steelseries;

use crate::reading::Reading;

/// Polls every protocol. Blocking (HID timeouts): call from the worker thread.
pub fn read_all() -> Vec<Reading> {
    let mut out = Vec::new();
    steelseries::read(&mut out);
    hidpp::read(&mut out);
    razer::read(&mut out);
    bt::read(&mut out);
    out
}
