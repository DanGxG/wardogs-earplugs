fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/earplugs.ico");
    println!("cargo:rerun-if-changed=assets/earplugs-small.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let mut resource = winresource::WindowsResource::new();
        resource.set("CompanyName", "Daniel Grao");
        resource.set("ProductName", "WarDogs Earplugs");
        resource.set(
            "FileDescription",
            "WarDogs Earplugs - Windows game volume utility",
        );
        resource.set("FileVersion", &format!("{}.0", env!("CARGO_PKG_VERSION")));
        resource.set("InternalName", "wardogs-earplugs");
        resource.set("OriginalFilename", "wardogs-earplugs.exe");
        resource.set(
            "LegalCopyright",
            "Copyright (c) 2026 Daniel Grao. MIT license.",
        );
        resource.set("Comments", "Author: Daniel Grao (DanGxG). https://danielgrao.me | Source: https://github.com/DanGxG/wardogs-earplugs | MIT license. Independent utility; not affiliated with WarDogs.");
        resource.set_icon("assets/earplugs.ico");
        resource.set_icon_with_id("assets/earplugs-small.ico", "2");
        resource.set_manifest_file("app.manifest");
        resource
            .compile()
            .expect("Could not embed Windows application resources");
    }
}
