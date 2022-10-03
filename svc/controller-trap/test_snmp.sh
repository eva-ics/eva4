#!/bin/sh

snmptrap -m ./ibmConvergedPowerSystems.mib -v 2c -c public 127.0.0.1:1162 '' IBM-CPS-MIB::problemTrap cpsSystemSendTrap s "hello i am here"
