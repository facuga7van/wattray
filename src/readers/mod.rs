mod bt;
mod centurion;
mod compx;
mod corsair;
mod generic;
mod ghub;
mod hidpp;
mod hyperx;
mod razer;
mod sinowealth;
mod steelseries;

use std::sync::OnceLock;
use std::thread::Thread;

use crate::reading::Reading;

pub static RESCAN: OnceLock<Thread> = OnceLock::new();

pub fn rescan() {
    if let Some(t) = RESCAN.get() {
        t.unpark();
    }
}

/// Polls every protocol. Blocking (HID timeouts): call from the worker thread.
pub fn read_all() -> Vec<Reading> {
    let mut out = Vec::new();
    steelseries::read(&mut out);
    corsair::read(&mut out);
    hyperx::read(&mut out);
    hidpp::read(&mut out);
    razer::read(&mut out);
    centurion::read(&mut out);
    compx::read(&mut out);
    sinowealth::read(&mut out);
    bt::read(&mut out);
    generic::read(&mut out);
    ghub::read(&mut out); // last: skips what HID++ already read
    out
}
