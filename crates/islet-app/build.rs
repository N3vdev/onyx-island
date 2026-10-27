//! Gives the executable the Onyx icon and its name in Explorer and Task Manager.

fn main() {
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
