#!/usr/bin/env bash

VERSION=4.0.2
BUILD=2024111802

[ -z "${EVA_REPOSITORY_URL}" ] && EVA_REPOSITORY_URL=https://pub.bma.ai/eva4
export EVA_REPOSITORY_URL

if [ -z "$ARCH_SFX" ]; then
  [ -z "$ARCH" ] && ARCH=$(uname -m)
  [[ "$ARCH" == arm* ]] && ARCH=arm
  case $ARCH in
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
fi

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

if [ ! -d ./runtime ]; then
  echo "Runtime dir not found. Please run the script in the folder where EVA ICS is already installed"
  exit 1
fi

if [ ! -x ./svc/eva-node ]; then
  echo "Update from v3 is not supported. Please manually migrate configuration"
  exit 1
fi

if ! command -v jq > /dev/null; then
  echo "Please install jq"
  exit 1
fi

if ! CURRENT_BUILD=$(./svc/eva-node --mode info|jq .build); then
  echo "Can't obtain current build"
  exit 1
fi

if [ ! -f /.eva_container ] && [ "$CURRENT_BUILD" -ge "$BUILD" ]; then
  echo "Your build is ${CURRENT_BUILD}, this script can update EVA ICS to ${BUILD} only"
  exit 1
fi

if [ ! -f /.eva_container ]; then
  rm -rf _update
  echo "- Starting update to ${VERSION} build ${BUILD}"
  mkdir -p _update
  if ! touch _update/test; then
    echo "Unable to write. Read-only file system?"
    exit 1
  fi
  echo "- Downloading new version..."
  DISTRO="eva-${VERSION}-${BUILD}-${ARCH_SFX}.tgz"
  cd _update || exit 1
  if [ -f "../${DISTRO}" ]; then
    echo "Using the existing pre-downloaded tarball"
    cp "../${DISTRO}" .
  else
    curl -L "${EVA_REPOSITORY_URL}/${VERSION}/nightly/${DISTRO}" \
      -o "${DISTRO}" || exit 1
  fi
  echo "- Extracting"
  tar xzf "${DISTRO}" || exit 1
  cd ..
  "./_update/eva-${VERSION}/svc/eva-node" --mode info > /dev/null || exit 2
  echo "- Stopping everything"
  ./sbin/eva-control stop
  if [ -d venv ]; then
    echo "- Installing missing Python modules"
    if ! EVA_DIR=$(pwd) MODS_LIST=./_update/eva-${VERSION}/install/mods.list \
            ./_update/eva-${VERSION}/sbin/venvmgr build; then
      exit 2
    fi
  fi
  if [ "$CHECK_ONLY" = 1 ]; then
    echo
    echo "Checks passed, venv updated. New version files can be explored in the _update dir"
    exit 0
  fi
fi

if [ ! -f /.eva_container ]; then
  echo "- Installing new files"
  rm -f "_update/eva-${VERSION}/update.sh"
  rm -f ./cli/eva-cloud-manager
  rm -rf vendored-apps
  cp -rf "_update/eva-${VERSION}/"* . || exit 1
fi

./prepare || exit 8

rm -f ./svc/sblock.sh

if [ ! -f /.eva_container ]; then
  echo "- Cleaning up"
  rm -rf _update
  CURRENT_BUILD=$(./svc/eva-node --mode info|jq .build)
  if [ "$CURRENT_BUILD" == "${BUILD}" ]; then
    echo "- Current build: ${BUILD}"
    echo "---------------------------------------------"
    echo "Update completed. Starting everything back"
    # update eva4 config for systemd
    if command -v systemctl > /dev/null 2>&1; then
      if systemctl -a |grep " eva4.service " > /dev/null 2>&1; then
        if [ -f ./etc/eva_config ]; then
          if ! grep "^SYSTEMD_EVA4_SERVICE=" ./etc/eva_config > /dev/null 2>&1; then
            echo SYSTEMD_EVA4_SERVICE=eva4.service >> ./etc/eva_config
            systemctl stop eva4.service
          fi
        else
          cp ./etc/eva_config-dist ./etc/eva_config
          echo SYSTEMD_EVA4_SERVICE=eva4.service >> ./etc/eva_config
          systemctl stop eva4.service
        fi
        if [ -f /etc/systemd/system/eva4.service ]; then
          if grep -E "^Restart=no$" /etc/systemd/system/eva4.service > /dev/null 2>&1; then
            sed -i 's/^Restart=no$/Restart=always\nRestartSec=5s/' /etc/systemd/system/eva4.service
            systemctl daemon-reload
          fi
        fi
      fi
    fi
    ./sbin/eva-control start
  else
    echo "Update failed"
    exit 1
  fi
else
  echo "---------------------------------------------"
  echo "Container configuration updated successfully"
fi
