//! SIGTERM and SIGHUP (`kill`, Ctrl-C in scripts/two-devices.sh, closing its terminal) close the
//! windows the way the user would, so the UI saves what was typed first, instead of ending the app
//! at once. Closing the last window then quits, as with Quit in the menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};

static RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    // Only an atomic store: all a signal handler may safely do.
    RECEIVED.store(true, Ordering::SeqCst);
}

pub fn close_windows_on_signal(app: AppHandle) {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: `on_signal` is async-signal-safe, and replacing the default action of these two
    // signals affects nothing else in the process.
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
    }
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(100));
        if RECEIVED.swap(false, Ordering::SeqCst) {
            for window in app.webview_windows().into_values() {
                // Asks, like the close button: the UI saves, then closes (or says why not).
                let _ = window.close();
            }
        }
    });
}
