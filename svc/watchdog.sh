#!/usr/bin/env bash

trap '' HUP

D=$(realpath "$0")
cd "$(dirname "${D}")/.." || exit 1

[ -f ./etc/watchdog ] && source ./etc/watchdog

[ -z "${INTERVAL}" ] && INTERVAL=60
[ -z "${MAX_TIMEOUT}" ] && MAX_TIMEOUT=2

PIDFILE="./var/watchdog.pid"

echo $$ > "${PIDFILE}"

while true; do
  sleep "${INTERVAL}"
  find "./var/eva.reload" -mmin +5 -exec rm -f {} \; >& /dev/null
  [ -f "./var/eva.reload" ] && continue
  if ! ./sbin/bus ./var/bus.ipc -n watchdog-$$ \
    --timeout "${MAX_TIMEOUT}" rpc call eva.core test >& /dev/null; then
      echo "node not responding, sending restart" 1>&2
      rm -f "${PIDFILE}"
      ./sbin/eva-control restart
      exit
  fi
  echo -n "+"
done
