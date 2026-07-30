//! Build script: embed the application icon into the Windows executable's
//! resource table so Explorer shows `vgmstudio.exe` with the `vgmstudio.ico` icon (the
//! window/taskbar icon is a separate runtime concern, `vgmstudio.rs::load_icon`).
//! A failed compile is downgraded to a `cargo::warning` so it can never block a
//! build; the only consequence is a generic Explorer icon.

fn main() {
    #[cfg(windows)]
    {
        // Relative to this crate's manifest dir; the repo's shared icon, the
        // same file `vgmstudio.rs` decodes for the window icon.
        println!("cargo::rerun-if-changed=../../src/vgmstudio.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../src/vgmstudio.ico");
        if let Err(error) = resource.compile() {
            println!("cargo::warning=could not embed the vgmstudio.exe icon: {error}");
        }
    }
}
