fn main() {
    let manifest_path = std::path::Path::new("app.manifest")
        .canonicalize()
        .expect("Failed to find app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifest_path.display()
    );
}
