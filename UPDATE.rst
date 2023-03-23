EVA ICS 4.0.1
*************

What is new
===========

* deny_read in ACL schemas
* various time series data frame fixes

Update instructions
===================

The update alters ACL logic of the default ACLs:

* "deny" field has been renamed into "deny_write" (backward-compatible aliases
  to "deny")

* "deny_read" field is introduced to block read-only access to items (exclude
  them from allow lists)

What is affected:

* if PVT/RPVT access is controlled with "deny" field in certain ACLs, move it
  to "deny_read" section after the update.

* To get access to the new features, rebuild custom Rust services with eva-sdk
  0.2
