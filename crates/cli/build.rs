use std::path::PathBuf;

fn main() {
    deno_napi::print_linker_flags("meow");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let snapshot_src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../target/meow-snapshot.bin");
    let snapshot_dst = out_dir.join("meow-snapshot.bin");

    if snapshot_src.exists() {
        std::fs::copy(&snapshot_src, &snapshot_dst).unwrap_or_else(|e| {
            panic!(
                "failed to copy snapshot from {} to {}: {}",
                snapshot_src.display(),
                snapshot_dst.display(),
                e
            );
        });
        println!("cargo:rerun-if-changed={}", snapshot_src.display());
    } else {
        // Create an empty placeholder so the include_bytes! always succeeds.
        std::fs::write(&snapshot_dst, []).unwrap_or_else(|e| {
            panic!(
                "failed to write empty snapshot placeholder to {}: {}",
                snapshot_dst.display(),
                e
            );
        });
        println!(
            "cargo:warning=V8 startup snapshot not found at {}. Building without snapshot (slower startup). \
             Run 'cargo run -p meow-snapshot -- --output target/meow-snapshot.bin' to create one.",
            snapshot_src.display()
        );
    }
}
