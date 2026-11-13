// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("Images/tortor.ico");
        res.compile().unwrap();
    } else if target_os == "haiku" {
        // Link HeOS SDK libraries: libheos_fs and libheos_net.
        // The SDK location is not guessed from the folder layout any more: set
        // HEOS_SDK_LIB_DIR to the sdk/lib directory of the HeOS tree. Otherwise the
        // system paths are searched.
        println!("cargo:rerun-if-env-changed=HEOS_SDK_LIB_DIR");
        if let Ok(dir) = env::var("HEOS_SDK_LIB_DIR") {
            println!("cargo:rustc-link-search=native={}", dir);
        } else {
            println!("cargo:rustc-link-search=native=/boot/system/develop/lib");
            println!("cargo:rustc-link-search=native=/boot/system/lib");
        }

        println!("cargo:rustc-link-lib=static=heos_fs");
        println!("cargo:rustc-link-lib=static=heos_net");
        println!("cargo:rustc-link-lib=network");
    }
}