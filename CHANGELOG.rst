EVA ICS CHANGELOG
*****************

4.0.0-stable
============

* 2022-02-10 build 2023021002: ADS bridge stores ADS state into a sensor
* 2022-01-31 build 2023013102: use node timeout for remote actions
* 2022-01-30 eva-js-framework 0.3.45: bulk API calls
* 2023-01-10 build 2023011001: minor fixes
* 2023-01-10 build 2023011001: public_api_log HMI svc option
* 2022-12-12 v4 installer: Fedora/RHEL installer fixes
* 2022-12-06 build 2022120601: File manager service "sh" EAPI method
* 2022-12-04 build 2022120401: HMI api_log.get HTTP method
* 2022-12-04 build 2022120401: API call "params" in HTTP API log
* 2022-11-26 build 2022112601: mailer svc STARTTLS fix, "ssl" option
* 2022-11-26 build 2022112601: filemgr svc url fetch fix for non-200-OK responses
* 2022-11-10 build 2022111002: max records limit for memory log
* 2022-11-09 build 2022110901: state processor lock to prevent data racing from actions and deploys
* 2022-11-09 build 2022110901: EAPI call tracing (experimental)
* 2022-11-08 eva-js-framework 0.3.44: event processing fixes, full objects in watch callbacks
* 2022-11-07 build 2022110701: filewriter svc syncs dirs on open/rename ops
* 2022-11-07 build 2022110701: deploy files from URLs fetched by remote nodes
* 2022-11-07 build 2022110701: file manager svc fetch files from URLs
* 2022-11-07 build 2022110701: MSAD nested groups support
* 2022-11-06 eva-js-framework 0.3.43: certain OTP fixes for set_normal and others
* 2022-11-06 build 2022110601: force register services on the broker (drops prev. instance)
* 2022-11-06 build 2022110601: "rotated_path" in file writer svc
* 2022-11-06 build 2022110601: Optional restart of ADS bridge on ADS controller panic
* 2022-11-06 build 2022110601: HMI accounting improvements: login attempts, api_log filters
* 2022-11-06 build 2022110601: MSAD cache delete/purge
* 2022-10-27 build 2021102701: API version in all variations of HMI login
* 2022-10-27 eva-js-framework 0.3.42: OTP fixes
* 2022-10-25 eva4-repl-legacy 0.0.25: lightweight pings
* 2022-10-25 build 2022102501: custom time-based file names in filewriter svc
* 2022-10-20 build 2021102001: ACL fixes: items/deny must keep read-only access
* 2022-10-13 eva-js-framework 0.3.41: API version auto-detect
* 2022-10-13 build 2022101301: API version in HMI login method response
* 2022-10-10 eva-shell 0.0.88: "untrusted" arg for "node append"
* 2022-10-10 build 2021101001: untrusted nodes, secure bulk replication topics
* 2022-10-09 switch arch: https://info.bma.ai/en/actual/eva4/security.html#switching-to-native
* 2022-10-09 installer fixes: fixed initial svc startup on slow systems
* 2022-10-09 build 2022100903: Ubuntu 20.04 LTS dedicated build
* 2022-10-09 build 2022100903: FIPS-140 mode

4.0.0 (2022-10-05)
==================

Common
------

    * New-generation cloud-SCADA/automation platform
