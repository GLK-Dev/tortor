use std::env;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("Images/tortor.ico");
        res.compile().unwrap();
    } else if target_os == "haiku" {
        // Link HeOS SDK libraries: libheos_fs and libheos_net
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let default_sdk_lib = manifest_dir.join("../HeOS/haiku-master/sdk/lib");

        if let Ok(dir) = env::var("HEOS_SDK_LIB_DIR") {
            println!("cargo:rustc-link-search=native={}", dir);
        } else if default_sdk_lib.exists() {
            println!("cargo:rustc-link-search=native={}", default_sdk_lib.display());
        } else {
            println!("cargo:rustc-link-search=native=/boot/system/develop/lib");
            println!("cargo:rustc-link-search=native=/boot/system/lib");
        }

        println!("cargo:rustc-link-lib=static=heos_fs");
        println!("cargo:rustc-link-lib=static=heos_net");
        println!("cargo:rustc-link-lib=network");
    }
}