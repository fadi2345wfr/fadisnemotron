//! On Windows the program gets its icon and version information (shown by
//! Explorer, the task manager and Windows' warning about unknown programs).
//! Elsewhere there is nothing to do.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut resources = winresource::WindowsResource::new();
        resources.set_icon("assets/icon.ico");
        resources.set("ProductName", "Fadis Nemotron");
        resources.set("FileDescription", "Fadis Nemotron");
        resources.set("OriginalFilename", "FadisNemotron.exe");
        // The program works without its icon; do not fail the build for it.
        if let Err(e) = resources.compile() {
            println!("cargo:warning=Symbol und Versionsangaben wurden nicht eingebunden: {e}");
        }
    }
}
