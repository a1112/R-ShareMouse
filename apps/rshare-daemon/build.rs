fn main() {
    println!("cargo:rerun-if-changed=Info.macos.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let plist = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("Info.macos.plist");
        // The daemon performs UDP/QUIC networking. Preserve its existing signing
        // identifier and purpose even when launched separately from the GUI.
        println!(
            "cargo:rustc-link-arg-bin=rshare-daemon=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
