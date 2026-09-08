fn main() {
    tauri_build::build();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        let ndk = find_ndk();
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

        let ndk_arch = match arch.as_str() {
            "aarch64" => "aarch64",
            "arm" => "arm",
            "x86" => "i386",
            "x86_64" => "x86_64",
            other => other,
        };
        let ndk_triple = match arch.as_str() {
            "aarch64" => "aarch64-linux-android",
            "arm" => "arm-linux-androideabi",
            "x86" => "i686-linux-android",
            "x86_64" => "x86_64-linux-android",
            other => other,
        };

        let prebuilt = std::path::PathBuf::from(&ndk).join("toolchains/llvm/prebuilt");

        let host_path = std::fs::read_dir(&prebuilt)
            .expect("Cannot read NDK prebuilt directory")
            .filter_map(Result::ok)
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("No host toolchain directory found in NDK prebuilt")
            .path();

        for base in &[host_path.join("lib64/clang"), host_path.join("lib/clang")] {
            if let Ok(entries) = std::fs::read_dir(base) {
                for version_dir in entries.filter_map(Result::ok) {
                    for sub in &[format!("lib/linux/{ndk_arch}"), format!("lib/{ndk_triple}")] {
                        let path = version_dir.path().join(sub);
                        if path.exists() {
                            println!("cargo:rustc-link-search=native={}", path.display());
                        }
                    }
                }
            }
        }

        println!("cargo:rustc-link-lib=c++abi");
    }
}

fn find_ndk() -> String {
    for var in &[
        "ANDROID_NDK_HOME",
        "ANDROID_NDK_ROOT",
        "NDK_HOME",
        "ANDROID_NDK_LATEST_HOME",
    ] {
        if let Ok(val) = std::env::var(var) {
            if std::path::Path::new(&val).exists() {
                return val;
            }
        }
    }

    for var in &["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Ok(sdk_root) = std::env::var(var) {
            let ndk_base = std::path::PathBuf::from(&sdk_root).join("ndk");
            if let Ok(entries) = std::fs::read_dir(&ndk_base) {
                let mut versions: Vec<std::path::PathBuf> = entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.path())
                    .collect();
                versions.sort();
                if let Some(latest) = versions.last() {
                    return latest.to_string_lossy().into_owned();
                }
            }
        }
    }

    panic!(
        "Could not find Android NDK. Set ANDROID_NDK_HOME to your NDK path.\n\
         Searched: ANDROID_NDK_HOME, ANDROID_NDK_ROOT, NDK_HOME, \
         ANDROID_NDK_LATEST_HOME, and <ANDROID_SDK_ROOT|ANDROID_HOME>/ndk/*"
    );
}
