EVA ICS 4.2.0
*************

What is new
===========

* JSON database support (PostgreSQL only)
* EAPI over websockets
* Lots of small bug fixes and performance improvements

Update instructions
===================

* MySQL support is dropped both in the core and in the default services.
  Consider migrating to PostgreSQL or SQLite for smaller installations.

* MUSL support is dropped. EVA ICS now requires a glibc-based system only.
  Custom builds may be still provided for Enterprise customers on request.

* The EVA ICS 4.1.0 build 2025121801 is the only supported upgrade path to EVA
  ICS 4.2.0. Please make sure you are running this build before upgrading.

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
