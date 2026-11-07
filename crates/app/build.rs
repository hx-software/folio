fn main() {
    #[cfg(all(windows, target_env = "msvc"))]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

#[cfg(all(windows, target_env = "msvc"))]
fn embed_windows_icon() {
    use std::{env, path::PathBuf};

    println!("cargo:rerun-if-changed=icons/folio.rc");
    println!("cargo:rerun-if-changed=icons/Folio.ico");

    let target = env::var("TARGET").expect("Cargo must set TARGET");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR")).join("folio.res");
    let compiler = cc::windows_registry::find_tool(&target, "rc.exe")
        .expect("The Windows SDK resource compiler (rc.exe) is required");
    let status = compiler
        .to_command()
        .args(["/nologo", "/fo"])
        .arg(&output)
        .arg("icons/folio.rc")
        .status()
        .expect("Could not run the Windows resource compiler");
    assert!(status.success(), "Could not compile the Folio icon");
    println!("cargo:rustc-link-arg-bin=folio={}", output.display());
}
