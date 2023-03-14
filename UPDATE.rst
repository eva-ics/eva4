EVA ICS 4.0.1
*************

What is new
===========

* deny_read in ACL schemas

Update instructions
===================

The update alters ACL logic of the default ACLs:

* "deny" field has been renamed into "deny_write" (backward-compatible aliases
  to "deny")

* "deny_read" field is introduced to block read-only access to items (exclude
  them from allow lists)

What is affected:

* if PVT/RPVT access is controlled with "deny" field in certain ACLs, rename it
  to "deny_read" after the update.
