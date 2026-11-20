use std::env;
use std::path::PathBuf;
use std::process::Command;

// compiles win32 icon resouce into the exe
fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/logo.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let res_obj = out_dir.join("app_rc.o");
    if Command::new("windres").args(["app.rc", "-O", "coff", "-o", res_obj.to_str().unwrap()]).status().map(|s| s.success()).unwrap_or(false) {
        println!("cargo:rustc-link-arg={}", res_obj.display());
    }
}
