//! Generate `include/scah.h` from the scah-ffi public C API.

fn main() {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config_path = crate_dir.join("cbindgen.toml");
    let header_path = crate_dir.join("include").join("scah.h");

    let config = cbindgen::Config::from_file(&config_path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", config_path.display());
    });

    std::fs::create_dir_all(header_path.parent().unwrap()).expect("create include dir");

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("unable to generate scah.h")
        .write_to_file(&header_path);

    println!("wrote {}", header_path.display());
}
