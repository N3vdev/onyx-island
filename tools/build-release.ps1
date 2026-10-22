# Builds the shipping Islet executable with the static MSVC runtime so it
# does not depend on a separately installed VC++ redistributable.
$ErrorActionPreference = "Stop"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release -p islet-app
