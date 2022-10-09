/*
 * This service requires http://www.net-snmp.org/ libraries put in /opt/libnetsnmp
 */
use std::env;

fn main() {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let target = env::var("TARGET").unwrap();
    if target == "x86_64-unknown-linux-gnu" {
        println!("cargo:rustc-link-search=/opt/libnetsnmp/{}-{}", os, arch);
        println!("cargo:rustc-link-lib=netsnmp");
        println!("cargo:rustc-link-arg=-lc");
        println!("cargo:rustc-link-arg=-lbsd");
    } else {
        match os.as_str() {
            "linux" | "freebsd" => println!("cargo:rustc-link-lib=static=netsnmp"),
            "windows" => println!("cargo:rustc-link-lib=netsnmp"),
            _ => unimplemented!(),
        };
        println!("cargo:rustc-link-search=/opt/libnetsnmp/{}-{}", os, arch);
        println!("cargo:rustc-link-arg=-lc");
    }
}
