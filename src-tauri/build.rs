fn main() {
    // UNIRAY_STORE=play: the Google Play edition (src/edition.rs), as the
    // Android project's build.gradle.kts reads the same variable.
    println!("cargo:rerun-if-env-changed=UNIRAY_STORE");
    println!("cargo:rustc-check-cfg=cfg(play_store)");
    if std::env::var("UNIRAY_STORE").as_deref() == Ok("play") {
        println!("cargo:rustc-cfg=play_store");
    }
    tauri_build::build()
}
