//! Gives the executable the Onyx icon and its name in Explorer and Task Manager.

fn main() {
    google_client();
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/onyx.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let version = env!("CARGO_PKG_VERSION");
    let numbers: Vec<&str> = version.split('.').chain(["0"]).take(4).collect();
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {n}
PRODUCTVERSION {n}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "Onyx"
      VALUE "ProductName", "Onyx"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/"),
        n = numbers.join(","),
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("onyx.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}

/// Onyx's own Google sign-in app (an OAuth client of type Desktop app, as
/// downloaded from Google Cloud), baked in so users only press Connect. It is
/// read from `secrets/google-client.json` (ignored by git), or the file
/// `ONYX_GOOGLE_CLIENT` names; without one, the build has no Gmail sign-in.
fn google_client() {
    println!("cargo:rerun-if-env-changed=ONYX_GOOGLE_CLIENT");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::var_os("ONYX_GOOGLE_CLIENT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("secrets/google-client.json"));
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("google-client.json");
    std::fs::write(out, text).unwrap();
}
