//! Incrusta el manifiesto, el icono y la información de versión en el ejecutable.
//! La versión sale siempre de `Cargo.toml`, así el manifiesto, las propiedades del
//! archivo y el paquete de publicación nunca quedan desalineados.

fn main() {
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=resources/swepr.rc");
    println!("cargo:rerun-if-changed=resources/swepr.ico");

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let dotted = parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".");
    let commas = parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");

    // Manifiesto con la versión del paquete.
    let manifest = std::fs::read_to_string(manifest_dir.join("app.manifest")).expect("Failed to read app.manifest");
    let manifest = manifest.replace("version=\"1.0.0.0\"", &format!("version=\"{dotted}\""));
    let manifest_path = out_dir.join("app.manifest");
    std::fs::write(&manifest_path, manifest).expect("Failed to write app.manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest_path.display());

    // Recursos: el .rc del proyecto (icono) más un bloque VERSIONINFO generado.
    let resources_dir = manifest_dir.join("resources");
    let base_rc = std::fs::read_to_string(resources_dir.join("swepr.rc")).expect("Failed to read resources/swepr.rc");
    let rc = format!(
        "{base_rc}\n\
1 VERSIONINFO\n\
FILEVERSION {commas}\n\
PRODUCTVERSION {commas}\n\
FILEOS 0x40004\n\
FILETYPE 0x1\n\
BEGIN\n\
  BLOCK \"StringFileInfo\"\n\
  BEGIN\n\
    BLOCK \"040904B0\"\n\
    BEGIN\n\
      VALUE \"CompanyName\", \"SWEPR\"\n\
      VALUE \"FileDescription\", \"SWEPR - Wallpaper Selector\"\n\
      VALUE \"FileVersion\", \"{version}\"\n\
      VALUE \"InternalName\", \"swepr\"\n\
      VALUE \"LegalCopyright\", \"GPL-3.0-only\"\n\
      VALUE \"OriginalFilename\", \"swepr.exe\"\n\
      VALUE \"ProductName\", \"SWEPR\"\n\
      VALUE \"ProductVersion\", \"{version}\"\n\
    END\n\
  END\n\
  BLOCK \"VarFileInfo\"\n\
  BEGIN\n\
    VALUE \"Translation\", 0x409, 1200\n\
  END\n\
END\n"
    );
    let rc_path = out_dir.join("swepr.rc");
    std::fs::write(&rc_path, rc).expect("Failed to write swepr.rc");

    let output = out_dir.join("swepr.res");
    let rc_exe = find_resource_compiler().expect("Windows Resource Compiler (rc.exe) is required to embed the application icon");
    // Se compila desde resources/ para que la ruta relativa del icono siga funcionando.
    let status = std::process::Command::new(rc_exe)
        .current_dir(&resources_dir)
        .args(["/nologo", "/fo"])
        .arg(&output)
        .arg(&rc_path)
        .status()
        .expect("Failed to run Windows Resource Compiler");
    assert!(status.success(), "Failed to compile the resource script");
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
