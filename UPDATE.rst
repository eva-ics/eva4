EVA ICS 4.2.0
*************

What is new
===========

* JSON database support (PostgreSQL only)
* EAPI over websockets
* Lots of small bug fixes and performance improvements

See more: https://info.bma.ai/en/actual/eva4/update/4.2.0.html

Update instructions
===================

* MySQL support is dropped both in the core and in the default services.
  Consider migrating to PostgreSQL or SQLite for smaller installations.

* MUSL support is dropped. EVA ICS now requires a GLIBC-based system only, the
  minimal GLIBC version is 2.35 (Ubuntu 22.04 LTS, Debian 12 Bookworm). Custom
  builds may be still provided for Enterprise customers on request.

* On certain embedded and low-power platforms, service startup times may
  increase, consider settings timeout/startup parameters accordingly.

* The EVA ICS 4.1.0 build 2025121801 is the only supported upgrade path to EVA
  ICS 4.2.0. Please make sure you are running this build before upgrading. To
  update the system to the mandatory intermediate version, run the following
  command:

```
EVA_VERSION=4.1.0 EVA_BUILD=2025121801 eva update
# or
EVA_VERSION=4.1.0 EVA_BUILD=2025121801 /opt/eva4/bin/eva-cloud-manager node update
```

The intermediate version is available until 2027-01-01, if updating after,
contact the product vendor or your support representative.

If TRAP controller service is used, install the following extra packages:

```
apt-get update && apt-get install -y --no-install-recommends libbsd-dev libsnmp-dev
```

x86_64
------

On each node execute:
```
EVA_ARCH_SFX=x86_64 eva update
# or
EVA_ARCH_SFX=x86_64 /opt/eva4/bin/eva-cloud-manager node update
```

The further updates can be performed via standard ways (no extra env variables,
remotely, via cloud update etc).

aarch64
-------

On each node execute:
```
EVA_ARCH_SFX=aarch64 eva update
# or
EVA_ARCH_SFX=aarch64 /opt/eva4/bin/eva-cloud-manager node update
```
The further updates can be performed via standard ways (no extra env variables,
remotely, via cloud update etc).
