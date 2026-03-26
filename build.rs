use std::error::Error;
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use std::{env, str};

use cargo_metadata::MetadataCommand;

const LLVM_MAJOR_VERSION: usize = 22;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // When building with x.py, LLVM_CONFIG is set by bootstrap and points directly to the
    // llvm-config binary. Use its parent as bin_dir. Otherwise fall back to cargo_metadata.
    let bin_dir: PathBuf = if let Ok(llvm_config) = env::var("LLVM_CONFIG") {
        PathBuf::from(&llvm_config).parent().unwrap().to_path_buf()
    } else {
        let metadata = MetadataCommand::new().exec().unwrap();
        let target_dir: PathBuf = metadata.target_directory.into();
        target_dir.join("build/llvm-build/build/bin")
    };

    let version = llvm_config(bin_dir.as_path(), "--version")?;

    if !version.starts_with(&format!("{LLVM_MAJOR_VERSION}.")) {
        return Err(format!(
            "failed to find correct version ({LLVM_MAJOR_VERSION}.x.x) of llvm-config (found {version})",
        )
        .into());
    }

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=cc");
    println!("cargo:rustc-link-search={}", llvm_config(bin_dir.as_path(), "--libdir")?);

    build_c_library(bin_dir.as_path())?;

    for name in llvm_config(bin_dir.as_path(), "--libnames")?.trim().split(' ') {
        println!("cargo:rustc-link-lib=static={}", parse_library_name(name)?);
    }

    for flag in llvm_config(bin_dir.as_path(), "--system-libs")?.trim().split(' ') {
        let flag = flag.trim_start_matches("-l");

        if flag.starts_with('/') {
            // llvm-config returns absolute paths for dynamically linked libraries.
            let path = Path::new(flag);

            println!("cargo:rustc-link-search={}", path.parent().unwrap().display());
            println!(
                "cargo:rustc-link-lib={}",
                parse_library_name(path.file_name().unwrap().to_str().unwrap())?
            );
        } else {
            println!("cargo:rustc-link-lib={}", flag);
        }
    }

    if let Some(name) = get_system_libcpp() {
        println!("cargo:rustc-link-lib={name}");
    }

    bindgen::builder()
        .header("wrapper.h")
        .clang_arg("-Icc/include")
        .clang_arg(format!("-I{}", llvm_config(bin_dir.as_path(), "--includedir")?))
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()?
        .write_to_file(Path::new(&env::var("OUT_DIR")?).join("bindings.rs"))?;

    Ok(())
}

fn build_c_library(bin_dir: &Path) -> Result<(), Box<dyn Error>> {
    unsafe { env::set_var("CXXFLAGS", llvm_config(bin_dir, "--cxxflags")?) };
    unsafe { env::set_var("CFLAGS", llvm_config(bin_dir, "--cflags")?) };

    cc::Build::new()
        .cpp(true)
        .files(
            read_dir("cc/lib")?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && path.extension() == Some(OsStr::new("cpp"))),
        )
        .include("cc/include")
        .include(llvm_config(bin_dir, "--includedir")?)
        .flag(if cfg!(target_env = "msvc") { "/WX" } else { "-Werror" })
        .std("c++17")
        .compile("CTableGen");

    Ok(())
}

fn get_system_libcpp() -> Option<&'static str> {
    if cfg!(target_env = "msvc") {
        None
    } else if cfg!(target_os = "macos") {
        Some("c++")
    } else {
        Some("stdc++")
    }
}

fn llvm_config(bin_dir: &Path, argument: &str) -> Result<String, Box<dyn Error>> {
    let call = format!("{} --link-static {argument}", bin_dir.join("llvm-config").display());

    Ok(str::from_utf8(
        &if cfg!(target_os = "windows") {
            Command::new("cmd").args(["/C", &call]).output()?
        } else {
            Command::new("sh").arg("-c").arg(&call).output()?
        }
        .stdout,
    )?
    .trim()
    .to_string())
}

fn parse_library_name(name: &str) -> Result<&str, String> {
    name.strip_prefix("lib")
        .and_then(|name| name.split('.').next())
        .ok_or_else(|| format!("failed to parse library name: {name}"))
}
