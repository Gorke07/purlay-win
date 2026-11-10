fn main() {
    // gpui embeds the application manifest (DPI awareness, common controls) itself;
    // this resource only adds the version info and the icon.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "Purlay");
        res.set("FileDescription", "Purlay for Windows");
        res.set("CompanyName", "Purlay");
        res.set("LegalCopyright", "AGPL-3.0-only");
        if std::path::Path::new("assets/purlay.ico").exists() {
            res.set_icon("assets/purlay.ico");
        }
        if let Err(e) = res.compile() {
            println!("cargo:warning=resource compile failed: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/purlay.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
