fn main() {
    println!("cargo:rerun-if-changed=native/macos_display.mm");
    println!("cargo:rerun-if-changed=native/macos_lan.mm");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .file("native/macos_display.mm")
            .file("native/macos_lan.mm")
            .compile("rshare_macos_display");
        for framework in [
            "Foundation",
            "AppKit",
            "CoreGraphics",
            "CoreMedia",
            "CoreVideo",
            "ScreenCaptureKit",
            "VideoToolbox",
            "Network",
            "Security",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
