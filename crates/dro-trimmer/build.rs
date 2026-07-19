//! Build script: embed the application icon into the Windows executable's
//! resource table, so Explorer shows `drotrim.exe` with the `dt.ico` icon. The
//! window/taskbar icon is a separate, runtime concern (`drotrim.rs::load_icon`);
//! this is the on-disk file icon.
//!
//! Deliberately resilient: if the resource cannot be compiled (no resource
//! compiler on `PATH`, say) the failure is downgraded to a `cargo::warning` so it
//! can never block a build. The only consequence is a generic Explorer icon.

fn main() {
    #[cfg(windows)]
    {
        // Relative to this crate's manifest dir; `../../src/dt.ico` is the repo's
        // shared icon, the same file `drotrim.rs` decodes for the window icon.
        println!("cargo::rerun-if-changed=../../src/dt.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../src/dt.ico");
        if let Err(error) = resource.compile() {
            println!("cargo::warning=could not embed the drotrim.exe icon: {error}");
        }
    }
}
