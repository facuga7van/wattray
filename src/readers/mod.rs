mod bt;
mod corsair;
mod generic;
mod hidpp;
mod hyperx;
mod razer;
mod steelseries;

use crate::reading::Reading;

/// Polls every protocol. Blocking (HID timeouts): call from the worker thread.
pub fn read_all() -> Vec<Reading> {
    let mut out = Vec::new();
    steelseries::read(&mut out);
    corsair::read(&mut out);
    hyperx::read(&mut out);
    hidpp::read(&mut out);
    razer::read(&mut out);
    bt::read(&mut out);
    generic::read(&mut out);
    out
}
