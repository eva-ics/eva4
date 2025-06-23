# EVA ICS v4

<img src="https://img.shields.io/badge/license-Apache%202.0-green" /> <img
src="https://img.shields.io/badge/rust-2021-pink.svg" />

<img src="https://raw.githubusercontent.com/eva-ics/eva4/main/logo.png"
width="200" />

[EVA ICS®](https://www.eva-ics.com) v4 is a new-generation Industrial-IoT
platform for Industry-4.0 automated control systems.

* The world-first and only Enterprise automation platform, written completely
  in Rust: extremely fast, secure and stable.

* Allows to handle millions of objects on a single node.

* Provides the real control of objects: actions and various automation
  scenarios can be executed, both locally and remotely.

* The new v4 micro-core architecture is completely scalable and allows to build
  complex setups for any industrial needs: factories, power plants, military
  sector etc.

* Built-in API and servers for web SCADA applications.

* Real-time event replication and interaction between cluster nodes and web HMI
  applications.

## Installation

Read <https://info.bma.ai/en/actual/eva4/install.html>

## Technical documentation

<https://info.bma.ai/en/actual/eva4/>

## Building from source

* [Install Rust](https://www.rust-lang.org/tools/install)

* Set the following environment variables and run cargo:

```
# The variable ARCH_SFX must be set either to "x86_64-musl" or to
# "aarch64-musl". If updates from the official repository and/or cloud manager
# CLI are not required, the variable can be set to any value.
export ARCH_SFX=x86_64-musl
cargo build --release
```

For OpenSSL v3 FIPS 140, build with "openssl3" feature enabled. In case of
any problems with FIPS 140 support in OpenSSL, enable "openssl-no-fips"
feature.

The following libraries must be manually downloaded, installed and/or compiled:

* **svc/controller-enip** requires [libplctag](https://libplctag.github.io)

* **svc/controller-trap** requires [libnetsnmp](http://www.net-snmp.org)

* **svc/controller-w1** requires [libowcapi](https://owfs.org/) and libow

The "ffi" service (*svc/ffi*) must be built separately and never as a static
binary, as it dynamically loads service libraries in runtime.

The "eva-videosink" service (*svc/videosink*) must be built separately and
never as a static binary, as it dynamically loads service libraries in runtime.

The controller-system service (svc/controller-system) must be built separately
with a feature to set required mode (to build as EVA ICS service set "service"
feature).

The following services are not open-sourced (the source can be provided for
mission-critical projects under an additional agreement):

* **eva-aaa-msad** - Active Directory authentication service

* **eva-zfrepl** Zero-failure replication service

* **eva-kioskman** HMI Kiosk manager service

* **eva-aaa-accounting** Event accounting and audit

## Troubleshooting

- If the build fails with "could not find `fips` in `openssl`", add `-F
  eva-common/openssl3` to enable FIPS on OpenSSL v3, or `-F
  eva-common/openssl-no-fips` to disable FIPS in OpenSSL completely.

- For any OpenSSL build errors, use `-F openssl-vendored` to build with the
  vendored OpenSSL.

## About the authors

[Bohemia Automation](https://www.bohemia-automation.com) /
[Altertech](https://www.altertech.com) is a group of companies with 15+ years
of experience in the enterprise automation and industrial IoT. Our setups
include power plants, factories and urban infrastructure. Largest of them have
1M+ sensors and controlled devices and the bar raises higher and higher every
day.
