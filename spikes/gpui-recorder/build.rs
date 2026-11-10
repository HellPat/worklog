use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=native/TimePicker.swift");
    println!("cargo:rerun-if-changed=native/ActivityCapture.swift");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        panic!("Worklog currently requires macOS");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default()).join("worklog-time-picker");
    let arch = if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let status = Command::new("xcrun")
        .args(["swiftc", "-O", "-target"])
        .arg(format!("{arch}-apple-macos14.0"))
        .arg("-module-cache-path")
        .arg(out.parent().unwrap_or(std::path::Path::new(".")))
        .args(["native/TimePicker.swift", "-o"])
        .arg(out)
        .status()
        .unwrap_or_else(|e| panic!("Could not compile native time picker: {e}"));
    assert!(status.success(), "Native time picker compilation failed");
    let activity =
        PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default()).join("worklog-activity");
    let status = Command::new("xcrun")
        .args(["swiftc", "-O", "-target"])
        .arg(format!("{arch}-apple-macos14.0"))
        .arg("-module-cache-path")
        .arg(activity.parent().unwrap_or(std::path::Path::new(".")))
        .args(["native/ActivityCapture.swift", "-o"])
        .arg(activity)
        .status()
        .unwrap_or_else(|error| panic!("Could not compile activity helper: {error}"));
    assert!(
        status.success(),
        "Native activity helper compilation failed"
    );
}
