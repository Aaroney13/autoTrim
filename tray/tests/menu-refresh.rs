// AppKit menus must be tested on the process's main thread, not a libtest worker.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/main.rs"]
mod tray_app;

fn main() {
    #[cfg(target_os = "macos")]
    tray_app::menu_refresh_tests::run();
    #[cfg(not(target_os = "macos"))]
    println!("native menu refresh regression skipped: requires macOS AppKit");
}
