fn main() {
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let manifest_path = std::path::Path::new("app.manifest")
        .canonicalize()
        .expect("Failed to find app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=resources/swepr.rc");
    println!("cargo:rerun-if-changed=resources/swepr.ico");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifest_path.display()
    );

    let resources_dir = manifest_dir.join("resources");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("swepr.res");
    let rc = find_resource_compiler().expect("Windows Resource Compiler (rc.exe) is required to embed the application icon");
    let status = std::process::Command::new(rc)
        .current_dir(&resources_dir)
        .args(["/nologo", "/fo"])
        .arg(&output)
        .arg("swepr.rc")
        .status()
        .expect("Failed to run Windows Resource Compiler");
    assert!(status.success(), "Failed to compile resources/swepr.rc");
    println!("cargo:rustc-link-arg={}", output.display());
}

fn find_resource_compiler() -> Option<std::path::PathBuf> {
    if let Ok(output) = std::process::Command::new("where.exe").arg("rc.exe").output() {
        if output.status.success() {
            if let Some(path) = String::from_utf8_lossy(&output.stdout).lines().next() {
                return Some(path.into());
            }
        }
    }

    let mut roots = Vec::new();
    if let Some(sdk_dir) = std::env::var_os("WindowsSdkDir") {
        roots.push(std::path::PathBuf::from(sdk_dir).join("bin"));
    }
    roots.push(std::path::PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin"));
    for root in roots {
        let Ok(versions) = std::fs::read_dir(root) else { continue; };
        let mut versions: Vec<_> = versions.flatten().map(|entry| entry.path()).collect();
        versions.sort();
        for version in versions.into_iter().rev() {
            let compiler = version.join("x64").join("rc.exe");
            if compiler.is_file() { return Some(compiler); }
        }
    }
    None
}
