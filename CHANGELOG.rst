EVA ICS CHANGELOG
*****************

4.0.2-stable
============

* 2024-08-09 build 2024080901: Core fixes
* 2024-08-08 build 2024080801: HMI login improvements, ADS improvements
* 2024-08-01 build 2024080101: HMI user dirs, optional time in raw events
* 2024-07-25 build 2024072501: Stability improvements
* 2024-07-16 build 2024071601: Advanced EAPI RAW events, sum value function in DBs, bar charts in OpCentre
* 2024-06-18 build 2024061801: Stability fixes, HMI improvements
* 2024-06-12 build 2024061201: ZF-replication multiple heartbeats
* 2024-06-05 build 2024060501: HMI SPA built-in support
* 2024-06-02 build 2024060201: Alarm service SQLite fixes
* 2024-05-27 build 2024052701: Alarm system
* 2024-05-14 build 2024051401: Service log level overriding
* 2024-05-02 build 2024050201: Replication OID masks fixes
* 2024-04-27 build 2024042703: SR update pipes, "phone" profile field, Py bus methods, OIDs exclude
* 2024-04-02 build 2024040201: Vendored apps print fixes, OPC-UA logging improvements
* 2024-03-26 build 2024032601: Data objects
* 2024-03-18 build 2024031801: InventoryDB2 and HMI API bug fixes, stability improvements
* 2024-03-12 build 2024031201: New core inventory database engine, vendored apps improvements
* 2024-03-06 build 2024030601: Modbus improvements, reports in vendored apps, eva-shell updates
* 2024-02-27 build 2024022701: Modbus improvements, EVA_CONSOLE_LOG_NO_TIMESTAMP option
* 2024-02-21 build 2024022101: Pub/Sub extra topics, security improvements
* 2024-02-12 build 2024021201: System controller improvements, formulas in OpCentre components
* 2024-02-08 build 2024020801: System Controller service, sdash audit trail, bug fixes
* 2024-02-01 build 2024020103: HMI, TwinCAT fixes, data search in accounting
* 2024-01-30 build 2024013001: ZF-replication self-repair, Cloud-wide accounting
* 2024-01-18 build 2024011801: Security updates, HMI API call filters
* 2024-01-10 build 2024011001: User profiles, password policies, mailer svc improvements
* 2023-12-13 build 2023121301: Stability fixes, Vendored apps navigation
* 2023-12-12 build 2023121203: Stability fixes, OpCentre improvements
* 2023-12-06 build 2023120601: FFI svc fixes for non-native builds, Pub/Sub (MQTT) controller
* 2023-12-05 build 2023120501: Operation Centre vendored UI app
* 2023-12-01 build 2023120101: pvt management HTTP API methods
* 2023-11-28 build 2023112801: bulk raw bus events, En/IP fixes
* 2023-11-20 build 2023112001: item auto-creation, svcs ABI, C++ SDK
* 2023-11-14 build 2023111401: various En/IP and ACL bug fixes, data generator "time"
* 2023-11-08 build 2023110801: zfrepl and InfluxDB connector fixes
* 2023-11-07 build 2023110701: Kiosk improvements, skip_disconnected in dbs, db.list HMI method
* 2023-10-31 build 2023103101: HMI user data, deploy fixes, vendored apps fixes
* 2023-10-26 build 2023102601: Font fixes in vendored apps
* 2023-10-24 build 2023102401: Forbid empty ACLs option
* 2023-10-23 build 2023102301: Strict ACL option, ECM env variables
* 2023-10-19 build 2023101902: File writer svc custom cols
* 2023-10-19 build 2023101901: TimescaleDB hyper-table automatic setup
* 2023-10-18 build 2023101802: TimescaleDB optimization, Python SDK 0.2.18
* 2023-10-17 build 2023101702: Vendored app: node system dashboard
* 2023-10-12 build 2023101201: TimescaleDB dedicated svc, filemgr "list" EAPI method
* 2023-10-07 build 2023100701: ADS improvements: array fixes, strings, wstrings
* 2023-09-28 build 2023092801: PostgreSQL optimizations in SQL databases svc
* 2023-09-17 build 2023091701: Bus UDP bridge service
* 2023-09-15 build 2023091502: Less deploy svc tests, "sources" (generators) renamed to "generator_sources"
* 2023-09-15 build 2023091501: Docker application launcher
* 2023-09-11 build 2023091101: TimescaleDB 14+ fixes
* 2023-09-04 build 2023090401: Fill param in generator planning
* 2023-09-01 build 2023090101: Data generators, OPC-UA auth improvements
* 2023-08-12 build 2023081201: Fixed memory overhead in database services connected to slow dbs
* 2023-08-01 build 2023080101: TwinCAT ADS improvements
* 2023-07-11 build 2023071101: File writer service updates (*dedup_lines* param, cuts broken lines)
* 2023-07-10 build 2023071003: Optimized the default memory footprint for embedded devices
* 2023-07-10 build 2023071001: Modbus controller IEEE754 big-endian support
* 2023-07-03 build 2023070301: virtual controller svc PLC var simulation
* 2023-06-18 build 2023061803: new status logic (see the update manifest)

4.0.1-stable
============

* 2023-06-18 build 2023061801: HMI svc web socket unsubscribe.state method
* 2023-06-15 build 2023061501: core.sysinfo EAPI method
* 2023-06-13 build 2023061301: SNMP and UDP traps bug fixes, ACL fixes
* 2023-06-07 build 2023060703: Extended HMI API key functions, API key fixes
* 2023-06-02 build 2023060201: Array ranges and dimensions in OPC-UA
* 2023-06-01 build 2023060101: OPC-UA support
* 2023-05-30 build 2023053001: HMI dev mode, Python SDK 0.2.10
* 2023-05-26 build 2023052601: Python SDK update to 0.2.9
* 2023-05-22 build 2023052201: new core EAPI method: bus.publish
* 2023-05-17 build 2023051702: ACLs fixes, Modbus and database pool fixes
* 2023-05-11 build 2023051101: OTP auth tolerance (1 forward/back period), BUS/RT 0.4.5
* 2023-05-01 build 2023050101: filewriter svc fixes and more verbose errors
* 2023-04-25 build 2023042501: fixed state event ordering for units
* 2023-04-18 build 2023041801: added uppercase WORD types in En/IP, debug log level on launch
* 2023-04-12 build 2023041201: systemd startup updates
* 2023-04-04 build 2023040402: SDK fixes for data frame filling
* 2023-03-29 build 2023032901: InfluxDB time series fixes, ML kit-ready
* 2023-03-23 build 2023032301: "deny_read" in the default ACLs, various time series data frame fixes

4.0.0-stable
============

* 2023-03-12 build 2023031201: Multi-level replication
* 2023-02-11 build 2023021601: Python SDK 0.0.40 (help and bug fixes)
* 2023-02-11 build 2023021101: Python SDK 0.0.39 (HMI X calls)
* 2023-02-10 build 2023021002: ADS bridge stores ADS state into a sensor
* 2023-01-31 build 2023013102: use node timeout for remote actions
* 2023-01-30 eva-js-framework 0.3.45: bulk API calls
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
