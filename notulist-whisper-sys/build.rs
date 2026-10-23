use std::env;
use std::path::PathBuf;

const MINIMUM_WHISPER_VERSION: &str = "1.9.0";

fn main() {
    let whisper_library = pkg_config::Config::new()
        .atleast_version(MINIMUM_WHISPER_VERSION)
        .probe("whisper")
        .expect("whisper.cpp with whisper.pc must be installed; enter the nix dev shell");

    let include_arguments = whisper_library
        .include_paths
        .iter()
        .map(|include_path| format!("-I{}", include_path.display()));

    let bindings = bindgen::Builder::default()
        .header_contents(
            "notulist_whisper_wrapper.h",
            "#include <whisper.h>\n#include <ggml-backend.h>\n",
        )
        .clang_args(include_arguments)
        .allowlist_function("whisper_.*")
        .allowlist_function("ggml_backend_load_all")
        .allowlist_type("whisper_.*")
        .allowlist_var("WHISPER_.*")
        .rustified_enum("whisper_sampling_strategy")
        .layout_tests(false)
        .generate()
        .expect("bindgen can read whisper.h");

    let output_directory = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    bindings
        .write_to_file(output_directory.join("whisper_bindings.rs"))
        .expect("bindings can be written to OUT_DIR");
}
