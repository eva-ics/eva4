EVA ICS 4.0.2
*************

What is new
===========

* new unit logic

Update instructions
===================

The update changes unit logic: status register is now used for hardware status
only, equal to sensors: 1 for good, -1 for error (default).

If unit status is used for logic, DO NOT APPLY this update, stay on 4.0.1 until
migrated. If eva-shell and Python macros controller are installed, fix their
versions in venv until migrated.

The behaviour changed in 4.0.2:

* action.toggle method switches between unit value (0/1)

* HMI service API "action" method requires "value" parameter and ignores status

* Python macros "action" method requires "value" parameter and ignores status

* eva-shell actions require "value" parameter. "item set" parameter require
  value but status is optional

* native UDP traps in trap svc require actions to be started as "a unit:...
  VALUE"

* external action scripts (controller-sr) no longer accept status

* the following services no longer accept status in pull mapping, action map
  has been simplified to accept values only (specify mapping directly, with no
  "value" field): controller-ads, controller-opcua, controller-w1,
  controller-enip, controller-sr, controller-modbus

* if used in cluster, all v4 nodes must be updated to 4.0.2 to properly accept
  the new action format

HMI service API and Python macros controller output warnings if status
parameter is specified. Consider removing the parameter from all calls as it
will be not accepted in further 4.0.2 builds.
