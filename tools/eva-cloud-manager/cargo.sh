#!/usr/bin/env bash

ARCH=$(uname -m)
ARCH=$(echo "${ARCH}" | sed 's/arm.*/arm/g')
case $ARCH in
  arm)
    ARCH_SFX=armv7
    ;;
  x86_64)
    ARCH_SFX=x86_64-musl
    ;;
  aarch64)
    ARCH_SFX=aarch64-musl
    ;;
  *)
    echo "Unsupported CPU architecture. Please build the distro manually"
    exit 13
    ;;
esac

export ARCH_SFX

cargo $@
