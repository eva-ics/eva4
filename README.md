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

* Install cross-rs:

```
cargo install cross --git https://github.com/cross-rs/cross
```

* Install certain required dependencies

```
apt-get -y install \
    libssl-dev libbsd-dev pkg-config build-essential cmake git autoconf\
    libtool libsnmp-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
```

* Prepare cross-build docker images

(Assumes you have Docker or compatible installed and running):

```
make docker-cross
```

* Build EVA ICS v4:

```
# for x86_64
cross build --target x86_64-unknown-linux-gnu --release
# for aarch64
cross build --target aarch64-unknown-linux-gnu --release
```

If building on the local machine, the following libraries must be manually
downloaded, installed and/or compiled:

* **svc/controller-enip** requires [libplctag](https://libplctag.github.io)

* **svc/controller-trap** requires [libnetsnmp](http://www.net-snmp.org)

* **svc/controller-w1** requires [libowcapi](https://owfs.org/) and libow

The following services are not open-sourced (the source can be provided for
mission-critical projects under an additional agreement):

* **eva-aaa-msad** - Active Directory authentication service

* **eva-zfrepl** Zero-failure replication service

* **eva-kioskman** HMI Kiosk manager service

* **eva-aaa-accounting** Event accounting and audit

* **eva-videosrv** Video recording server

## Troubleshooting

- For any OpenSSL build errors, use `-F openssl-vendored` to build with the
  vendored OpenSSL (note: this disables FIPS).

## About the authors

[Bohemia Automation](https://www.bohemia-automation.com) /
[Altertech](https://www.altertech.com) is a group of companies with 20+ years
of experience in the enterprise automation and industrial IoT. Our setups
include power plants, factories and urban infrastructure. Largest of them have
1M+ sensors and controlled devices and the bar raises higher and higher every
day.
