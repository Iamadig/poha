fn main() {
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-fapple-link-rtlib");

    #[cfg(target_os = "macos")]
    build_poha_diarizer();

    tauri_build::build()
}

#[cfg(target_os = "macos")]
fn build_poha_diarizer() {
    let triple = std::env::var("TARGET").expect("TARGET");
    println!("cargo:rustc-env=POHA_TARGET_TRIPLE={triple}");

    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let package_dir = manifest_dir.join("diarizer");
    let swift_src = package_dir.join("Sources/PohaDiarizer/main.swift");
    let package = package_dir.join("Package.swift");
    let binaries_dir = manifest_dir.join("binaries");
    let dst = binaries_dir.join(format!("poha-diarizer-{triple}"));
    let build_dir = manifest_dir
        .join("../../..")
        .join("target")
        .join("poha-diarizer-swift")
        .join(&triple);

    println!("cargo:rerun-if-changed={}", package.display());
    println!("cargo:rerun-if-changed={}", swift_src.display());

    if std::env::var_os("POHA_SKIP_DIARIZER_BUILD").is_some() {
        return;
    }

    std::fs::create_dir_all(&binaries_dir).expect("create binaries/");
    std::fs::create_dir_all(&build_dir).expect("create swift build dir");

    let status = std::process::Command::new("swift")
        .args(["build", "-c", "release", "--package-path"])
        .arg(&package_dir)
        .arg("--build-path")
        .arg(&build_dir)
        .status()
        .expect("failed to run swift build for poha-diarizer");

    assert!(status.success(), "swift build failed for poha-diarizer");

    let built = build_dir.join("release").join("poha-diarizer");
    std::fs::copy(&built, &dst).unwrap_or_else(|error| {
        panic!(
            "failed copying poha-diarizer {} -> {}: {error}",
            built.display(),
            dst.display()
        )
    });
}

#[cfg(not(target_os = "macos"))]
fn build_poha_diarizer() {}
