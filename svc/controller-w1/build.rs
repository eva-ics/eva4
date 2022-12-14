/*
 * This service requires https://owfs.org libraries put in /opt/libow
 */
use std::env;

fn main() {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    //let target = env::var("TARGET").unwrap();
    match os.as_str() {
        "linux" | "freebsd" => {
            println!("cargo:rustc-link-lib=static=owcapi");
            println!("cargo:rustc-link-lib=static=ow");
        }
        _ => unimplemented!(),
    };
    println!("cargo:rustc-link-search=/opt/libow/{}-{}", os, arch);
}
