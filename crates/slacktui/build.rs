//! Windows built with MinGW: OpenH264's build script links `stdc++` as a library, which the
//! linker resolves to MinGW's DLL (`libstdc++-6.dll`, not on a stock Windows). Put a folder
//! holding only the static `libstdc++.a` first on the search path, so the .exe needs no MinGW
//! DLLs. Nothing happens for other targets.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !(target.contains("windows") && target.ends_with("-gnu")) {
        return;
    }
    let compiler = format!("{}-w64-mingw32-g++", target.split('-').next().unwrap_or("x86_64"));
    let found = std::process::Command::new(&compiler).arg("-print-file-name=libstdc++.a").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let Some(lib) = found.filter(|p| std::path::Path::new(p).is_absolute()) else {
        println!("cargo:warning=static libstdc++ not found with {compiler}; the .exe will need libstdc++-6.dll");
        return;
    };
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("static-libstdcxx");
    std::fs::create_dir_all(&out).expect("create the static libstdc++ folder");
    std::fs::copy(&lib, out.join("libstdc++.a")).expect("copy libstdc++.a");
    println!("cargo:rustc-link-search=native={}", out.display());
}
