//! Tauri build script for the diagnostic shell: validates `tauri.conf.json`,
//! generates the Windows resource and the permission metadata. The frontend that is
//! embedded here is the generated `apps/gui/dist` build; this script never runs a
//! package manager, never starts a window and never touches a network.
fn main() {
    tauri_build::build()
}
