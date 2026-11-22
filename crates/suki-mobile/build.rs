fn main() {
    // Android devices can use 4 KB or 16 KB pages. NDK r27 needs explicit flags.
    // Keep this on the final Android binary; dx sets its own target rustflags.
    println!("cargo:rerun-if-changed=native/SessionVault.kt");
    println!("cargo:rerun-if-env-changed=WRY_ANDROID_KOTLIN_FILES_OUT_DIR");
    println!("cargo:rerun-if-env-changed=WRY_ANDROID_PACKAGE");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo::rustc-link-arg=-Wl,-z,max-page-size=16384");
        println!("cargo::rustc-link-arg=-Wl,-z,common-page-size=16384");
        if let (Ok(directory), Ok(package)) = (
            std::env::var("WRY_ANDROID_KOTLIN_FILES_OUT_DIR"),
            std::env::var("WRY_ANDROID_PACKAGE"),
        ) {
            let directory = std::path::Path::new(&directory);
            std::fs::create_dir_all(directory).expect("create Android source directory");
            let source = include_str!("native/SessionVault.kt").replace("__PACKAGE__", &package);
            std::fs::write(directory.join("SessionVault.kt"), source)
                .expect("install Android vault binding");
            std::fs::write(
                directory.join("proguard-suki-session.pro"),
                format!("-keep class {package}.SessionVault {{ public static *; }}\n"),
            )
            .expect("keep the Android JNI bridge in release builds");
            println!("cargo:rustc-env=SUKI_ANDROID_PACKAGE={package}");
        }
    }
}
