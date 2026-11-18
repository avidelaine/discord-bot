//! Piper (feature `piper`) phonemizes with espeak-ng, built from source and linked statically.
//! Same commit as piper-tts 1.8.0 (piper1-gpl CMakeLists.txt, GIT_TAG 724808c) and the same CMake switches,
//! so phoneme ids match the Python package. Source: `ESPEAK_NG_SRC` (a checkout), else cloned into OUT_DIR.
//! The static CRT (/MT) follows `.cargo/config.toml` through the `cmake` crate.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ESPEAK_NG_SRC");
    #[cfg(feature = "piper")]
    espeak::build();
}

#[cfg(feature = "piper")]
mod espeak {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const COMMIT: &str = "724808c";

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").args(args).current_dir(dir).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "git {args:?} failed in {}", dir.display());
    }

    fn source() -> PathBuf {
        if let Some(p) = std::env::var_os("ESPEAK_NG_SRC") {
            return p.into();
        }
        let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let dir = out.join("espeak-ng-src");
        if !dir.join("CMakeLists.txt").exists() {
            let _ = std::fs::remove_dir_all(&dir);
            git(&out, &["clone", "--filter=blob:none", "https://github.com/espeak-ng/espeak-ng.git", "espeak-ng-src"]);
            git(&dir, &["checkout", COMMIT]);
        }
        dir
    }

    fn find_libs(dir: &Path, names: &[&str], found: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                find_libs(&p, names, found);
            } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| names.contains(&n)) {
                found.push(p.parent().unwrap().to_path_buf());
            }
        }
    }

    pub fn build() {
        let src = source();
        let ucd_inc = src.join("src/ucd-tools/src/include");
        let flags = format!("-D_FILE_OFFSET_BITS=64 -I{}", ucd_inc.display());
        let mut cfg = cmake::Config::new(&src);
        cfg.profile("Release")
            .define("BUILD_SHARED_LIBS", "OFF")
            .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
            .define("ENABLE_TESTS", "OFF")
            .define("USE_ASYNC", "OFF")
            .define("USE_MBROLA", "OFF")
            .define("USE_LIBSONIC", "OFF")
            .define("USE_LIBPCAUDIO", "OFF")
            .define("USE_KLATT", "OFF")
            .define("USE_SPEECHPLAYER", "OFF")
            .define("EXTRA_cmn", "ON")
            .define("EXTRA_ru", "ON")
            .cflag(&flags)
            .cxxflag(&flags)
            .build_target("espeak-ng");
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            cfg.define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW").define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded");
        }
        let dst = cfg.build();
        let mut dirs = Vec::new();
        find_libs(&dst.join("build"), &["espeak-ng.lib", "libespeak-ng.a", "ucd.lib", "libucd.a"], &mut dirs);
        dirs.sort();
        dirs.dedup();
        assert!(!dirs.is_empty(), "espeak-ng static libraries not found under {}", dst.display());
        for d in dirs {
            println!("cargo:rustc-link-search=native={}", d.display());
        }
        println!("cargo:rustc-link-lib=static=espeak-ng");
        println!("cargo:rustc-link-lib=static=ucd");
        if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
            println!("cargo:rustc-link-lib=m");
        }
    }
}
