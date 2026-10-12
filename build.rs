fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let mut resource = winresource::WindowsResource::new();
        resource.set("ProductName", "WarDogs Earplugs");
        resource.set("FileDescription", "Windows mixer volume hotkey for WarDogs");
        resource.set("OriginalFilename", "wardogs-earplugs.exe");
        resource.set("LegalCopyright", "Copyright 2026 Daniel Grao. MIT license.");
        resource.set_manifest_file("app.manifest");
        resource
            .compile()
            .expect("Could not embed Windows application resources");
    }
}
