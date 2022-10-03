#!/usr/bin/env bash

trap '' HUP
trap '' INT

if [ -z "$1" ]; then
  echo "No params given"
  exit 10
fi
echo $$ > "$1"
shift
while true; do
  "$@"
  sleep 0.1
done
