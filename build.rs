fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/vader5.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/vader5.ico")
            .compile()
            .expect("embed Windows application icon");
    }
}
