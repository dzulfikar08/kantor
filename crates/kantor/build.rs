//! Windows: embed Kantor's icon in kantor.exe (Explorer, the taskbar and
//! GPUI's window icon read resource 1). Nothing to do elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = "../../packaging/windows/kantor.rc";
    println!("cargo:rerun-if-changed={rc}");
    println!("cargo:rerun-if-changed=../../packaging/windows/kantor.ico");
    embed_resource::compile(rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
