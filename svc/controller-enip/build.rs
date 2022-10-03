/*
 * This service requires https://libplctag.github.io libraries put in /opt/libplctag
 */
use std::env;

fn main() {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    match os.as_str() {
        "linux" | "freebsd" => println!("cargo:rustc-link-lib=static=plctag"),
        "windows" => println!("cargo:rustc-link-lib=plctag"),
        _ => unimplemented!(),
    };
    println!("cargo:rustc-link-search=/opt/libplctag/{}-{}", os, arch);
    println!("cargo:rustc-link-arg=-lc");
}
