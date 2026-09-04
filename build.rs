use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement building if libplacebo doesn't exist (windows moment)
    let dependencies = system_deps::Config::new().probe()?;

    let mut bindgen = bindgen::Builder::default()
        .header("src/libplacebo.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .allowlist_file("^.*placebo.*$")
        .allowlist_recursively(true)
        .clang_macro_fallback()
        .prepend_enum_name(false)
        .default_macro_constant_type(bindgen::MacroTypeVariation::Signed);

    for (_, lib) in dependencies.iter() {
        for path in lib.include_paths.iter().filter_map(|path| path.to_str()) {
            bindgen = bindgen.clang_arg("-I").clang_arg(path);
        }
    }

    let bindings = bindgen.generate()?;
    let out_path = PathBuf::from(std::env::var("OUT_DIR")?);
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");

    Ok(())
}
