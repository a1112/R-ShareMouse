fn main() {
    println!("cargo:rerun-if-changed=native/macos.cpp");
    println!("cargo:rerun-if-changed=../../drivers/audio-common/rshare_audio_bridge.h");
    println!("cargo:rerun-if-changed=../../drivers/macos/rshare-audio/broker_client.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .flag("-fblocks")
            .file("native/macos.cpp")
            .compile("rshare_audio_native");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
