#!/bin/sh

if [ "$(id -u)" != "0" ]; then
  echo "Please run this script as root"
  exit 11
fi

AUTOSTART=
LOGROTATE=
CREATE_SYMLINKS=
MODE=0
PREPARE_ONLY=
HMI=
PREFIX=/opt/eva4
REPO=https://pub.bma.ai/eva4
INSTALL_TEST=
ID=
ID_LIKE=
[ -z "$EVA_USER" ] && EVA_USER=eva

on_exit() {
  err=$?
  if [ $err -ne 0 ]; then
    echo
    echo "FAILED, CODE: $err"
  fi
}

deploy_svc() {
  svc=$1
  TPL="share/svc-tpl/svc-tpl-$(echo "$svc"|cut -d: -f1).yml"
  NAME="eva.$(echo "$svc"|cut -d: -f2)"
  echo " - $TPL -> $NAME"
  ( echo "svcs:" ; echo "- id: ${NAME}"; echo "  params:" ; \
    sed 's/^/   /g' "${TPL}") | \
      ./bin/yml2mp | \
      ./sbin/bus -s ./var/bus.ipc rpc call eva.core svc.deploy - > /dev/null || exit 12
}

trap on_exit EXIT

while [ "$1" ]; do
  key="$1"
  case $key in
    --autostart)
      AUTOSTART=1
      shift
      ;;
    --logrotate)
      LOGROTATE=1
      shift
      ;;
    --symlinks)
      CREATE_SYMLINKS=1
      shift
      ;;
    --prefix)
      PREFIX=$2
      shift
      shift
      ;;
    --mode)
      MODE=$2
      shift
      shift
      ;;
    --prepare-only)
      PREPARE_ONLY=1
      shift
      ;;
    --hmi)
      HMI=1
      shift
      ;;
    --test)
      INSTALL_TEST=_test
      shift
      ;;
    --force-os)
      shift
      ID=$1
      shift
      case $ID in
        debian)
          ID=debian
          ID_LIKE=debian
          ;;
        ubuntu)
          ID=ubuntu
          ID_LIKE=debian
          ;;
        raspbian)
          ID=raspbian
          ID_LIKE=debian
          ;;
        fedora)
          ID=fedora
          ID_LIKE=fedora
          ;;
        rhel)
          ID=rhel
          ID_LIKE=fedora
          ;;
        centos)
          ID=centos
          ID_LIKE=fedora
          ;;
        *)
          echo "Invalid option"
          exit 12
          ;;
      esac
      ;;
    -a)
      MODE=1
      AUTOSTART=1
      LOGROTATE=1
      CREATE_SYMLINKS=1
      shift
      ;;
    -h|--help)
      cat <<EOF
Arguments:

 --autostart        Configure auto start on system boot (via systemd)
 --logrotate        Install logrotate and configure log rotation
 --symlinks         Create symlink to eva shell in /usr/local/bin
 --prefix DIR       Installation prefix (default: /opt/eva4)
 --hmi              Create HMI aaa and web services
 --mode MODE        0: minimal (default)
                    1: + Python and EVA ICS shell
                    2: + gcc, g++, development headers
                    3: + Rust compiler, more headers/libs
 --prepare-only     Prepare the system and quit
 --force-os         Force OS distribution (disable auto-detect): debian, ubuntu,
                    fedora, raspbian, rhel, centos
 -a                 Automatic setup, equal to autostart+logrotate+symlinks+mode 1
                    Mode can be overriden by an additional --mode arg
EOF
      exit 0
      ;;
    *)
      echo "Invalid arg $key"
      echo "-h or --help for help"
      exit 12
      ;;
  esac
done

if [ ! -f /etc/os-release ]; then
  echo "No /etc/os-release. Can not detect Linux distribution"
  exit 12
fi

[ -z "$ID_LIKE" ] && . /etc/os-release
[ -z "$ID_LIKE" ] && ID_LIKE=$ID

[ -z "$OS_VERSION_MAJOR" ] && \
  OS_VERSION_MAJOR=$(grep "^VERSION=" /etc/os-release|tr -d '"'|cut -d= -f2|awk '{ print $1 }'|cut -d. -f1)

for I in $ID_LIKE; do
  case $I in
    debian|fedora)
      ID_LIKE=$I
      break
      ;;
  esac
done

case $ID in
  debian|fedora|ubuntu|raspbian|rhel|centos|alpine)
    ;;
  *)
    echo "Unsupported Linux distribution. Please install EVA ICS manually"
    exit 12
    ;;
esac

[ -z "$ARCH" ] && ARCH=$(uname -m)
ARCH=$(echo "${ARCH}" | sed 's/arm.*/arm/g')
case $ARCH in
  #arm)
    #ARCH_SFX=armv7
    #;;
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

if [ -d "$PREFIX" ] && [ -z "${PREPARE_ONLY}" ]; then
  echo "Directory $PREFIX already exists, aborting"
  exit 9
fi

if [ $ID_LIKE = "debian" ]; then
  apt-get update || exit 10
  if [ ! -f /etc/localtime ]; then
    env DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y tzdata || exit 10
    ln -sf /usr/share/zoneinfo/Etc/UTC /etc/localtime
  fi
fi

if [ $MODE -ge 1 ]; then
  if [ "$ID" = "rhel" ]; then
    echo "Installing EPEL for RedHat Enterprise Linux ${OS_VERSION_MAJOR}..."
    yum install -y \
      "https://dl.fedoraproject.org/pub/epel/epel-release-latest-${OS_VERSION_MAJOR}.noarch.rpm" || exit 10
    if [ "$OS_VERSION_MAJOR" -ge 8 ]; then
      echo "Enabling codeready-builder..."
      ARCH=$( /bin/arch )
      subscription-manager repos --enable "codeready-builder-for-rhel-${OS_VERSION_MAJOR}-${ARCH}-rpms"
    fi
  elif [ "$ID" = "centos" ]; then
    yum install -y epel-release
    dnf -y install dnf-plugins-core
    dnf config-manager --set-enabled powertools
  fi
fi

addgroup --system "${EVA_USER}"
if ! id "${EVA_USER}" > /dev/null 2>&1; then
  adduser --system --no-create-home --home "${PREFIX}" --ingroup eva eva
fi

case $ID_LIKE in
  debian)
    apt-get install -y --no-install-recommends \
      bash jq curl procps ca-certificates tar gzip || exit 10
    if [ $MODE -ge 1 ]; then
      apt-get install -y --no-install-recommends \
        python3 || exit 10
      apt-get install -y --no-install-recommends python3-venv # no dedicated deb in some distros
    fi
    if [ $MODE -ge 2 ]; then
      apt-get install -y --no-install-recommends gcc g++ make python3-dev || exit 10
    fi
    if [ $MODE -ge 3 ]; then
      apt-get install -y --no-install-recommends libjpeg-dev libz-dev libssl-dev libffi-dev || exit 10
    fi
    if [ "$LOGROTATE" ]; then
      apt-get install -y --no-install-recommends logrotate
    fi
    ;;
  alpine)
    apk update || exit 10
    apk add bash jq curl tar || exit 10
    if [ $MODE -ge 1 ]; then
      apk add python3
    fi
    if [ $MODE -ge 2 ]; then
      apk add gcc g++ make libc-dev musl-dev python3-dev linux-headers || exit 10
      ln -sf /usr/include/locale.h /usr/include/xlocale.h || exit 10
    fi
    if [ $MODE -ge 3 ]; then
      apk add libjpeg jpeg-dev libjpeg-turbo-dev libpng-dev libffi-dev openssl-dev freetype-dev || exit 10
    fi
    if [ "$LOGROTATE" ]; then
      apk add logrotate
    fi
    ;;
  fedora)
    yum install -y bash jq curl procps ca-certificates tar gzip hostname which || exit 10
    if [ $MODE -ge 1 ]; then
      yum install -y python3
    fi
    if [ $MODE -ge 2 ]; then
      yum install -y gcc python3-devel || exit 10
      yum install -y g++ || yum install -y gcc-c++ || exit 10
    fi
    if [ $MODE -ge 3 ]; then
      yum install -y libffi-devel openssl-devel libjpeg-devel zlib-devel || exit 10
    fi
    if [ "$LOGROTATE" ]; then
      yum install -y logrotate
    fi
    ;;
esac

if [ $MODE -ge 3 ]; then
  if [ ! -f "$HOME/.cargo/env" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh /dev/stdin -y -c rustc || exit 10
  fi
  . "$HOME/.cargo/env" || exit 10
fi

if [ "$PREPARE_ONLY" ]; then
  echo
  echo "System configured"
  exit 0
fi

rm -f /tmp/eva-dist.tgz

if ! curl -Ls ${REPO}/update_info${INSTALL_TEST}.json \
  -o /tmp/eva_update_info.json; then
  echo "Unable to download EVA ICS update info"
  exit 7
fi

VERSION=$(jq -r .version /tmp/eva_update_info.json)
BUILD=$(jq -r .build /tmp/eva_update_info.json)
rm -f /tmp/eva_update_info.json

echo "Installing EVA ICS version ${VERSION} build ${BUILD}"

if [ -z "$VERSION" ] || [ -z "$BUILD" ]; then
  echo "Unable to get EVA ICS version / build info"
  exit 7
fi

if ! curl -L "${REPO}/${VERSION}/nightly/eva-${VERSION}-${BUILD}-${ARCH_SFX}.tgz" \
  -o /tmp/eva-dist.tgz; then
  echo "Unable to download EVA ICS distribution"
  exit 7
fi

mkdir -p "$PREFIX"
tar xzf /tmp/eva-dist.tgz -C "$PREFIX" || exit 7
chown -R 0:0 "$PREFIX" || exit 7
rm -f /tmp/eva-dist.tgz
cd "${PREFIX}/eva-${VERSION}" || exit 7
mv ./* .. || exit 7
cd ..
rmdir "eva-${VERSION}"

./prepare || exit 10

chgrp eva pvt
chmod 750 pvt

if [ "$AUTOSTART" ]; then
  if [ "$ID_LIKE" = "alpine" ]; then
    sed "s|/opt/eva4|${PREFIX}|g" ./etc/openrc/eva4 > /etc/init.d/eva4
    chmod +x /etc/init.d/eva4 || exit 9
    rc-update add eva4 default || exit 9
    # do not check exit code, as the installer can be executed in chroot
    rc-service eva4 start > /dev/null 2>&1
    ./sbin/eva-control start || exit 11
  else
    if ! command -v systemctl > /dev/null; then
      echo "[!] systemctl is not installed. Skipping auto-startup setup"
    else
      sed "s|/opt/eva4|${PREFIX}|g" ./etc/systemd/eva4.service > /etc/systemd/system/eva4.service
      systemctl enable eva4 || exit 9
      echo "Starting EVA ICS with systemctl..."
      systemctl restart eva4 || exit 11
      TEST="./sbin/bus ./var/bus.ipc rpc call eva.core test"
      C=0
      while [ "$result" != "0" ]; do
        if [ $C -gt 20 ]; then
          echo "Failed!"
          exit 1
        fi
        ${TEST} > /dev/null 2>&1
        result=$?
        if [ "$result" = "0" ]; then
          break
        fi
        C=$((C+2))
        sleep 0.5
      done
      fi
  fi
else
  ./sbin/eva-control start || exit 11
fi

echo "Deploying the default services..."
for svc in filemgr:filemgr.main; do
  deploy_svc $svc
done

if [ "$HMI" ]; then
  [ "$DEFAULTKEY" ] || DEFAULTKEY=$( (tr -cd '[:alnum:]' < /dev/urandom | head -c64) 2>/dev/null)
  echo "Deploying ACL and local auth services..."
  for svc in aaa-acl:aaa.acl aaa-localauth:aaa.localauth; do
    deploy_svc $svc
  done
  echo "Deplying the default ACLs..."
  (
  cat <<EOF
acls:
- id: default
  read: { items: [ "#" ] }
  write: { items: [ "#" ] }
EOF
  ) | ./bin/yml2mp | \
    ./sbin/bus -s ./var/bus.ipc rpc call eva.aaa.acl acl.deploy - > /dev/null || exit 12
  echo "Deplying API keys..."
  (
  cat <<EOF
keys:
- id: default
  key: ${DEFAULTKEY}
  acls: [ "default" ]
EOF
  ) | ./bin/yml2mp | \
    ./sbin/bus -s ./var/bus.ipc rpc call eva.aaa.localauth key.deploy - > /dev/null || exit 12
fi

if [ "$HMI" ]; then
  echo "Deploying HMI services..."
  [ "$ADMINKEY" ] || ADMINKEY=$( (tr -cd '[:alnum:]' < /dev/urandom | head -c64) 2>/dev/null)
  [ "$OPKEY" ] || OPKEY=$( (tr -cd '[:alnum:]' < /dev/urandom | head -c64) 2>/dev/null)
  [ "$OPPASSWD" ] || OPPASSWD=$( (tr -cd '[:alnum:]' < /dev/urandom | head -c16) 2>/dev/null)
  OPPASSWD_HASHED=$(./sbin/bus -s ./var/bus.ipc rpc call eva.aaa.localauth password.hash password=${OPPASSWD} algo=pbkdf2 |jq -r .hash)
  for svc in hmi:hmi.default; do
    deploy_svc $svc
  done
  echo "Deplying HMI ACLs..."
  (
  cat <<EOF
acls:
- id: admin
  admin: true
- id: operator
  read: { items: [ "#" ], pvt: [ "#" ], rpvt: [ "#" ] }
  write: { items: [ "#" ] }
EOF
  ) | ./bin/yml2mp | \
    ./sbin/bus -s ./var/bus.ipc rpc call eva.aaa.acl acl.deploy - > /dev/null || exit 12
  echo "Deplying HMI API keys..."
  (
  cat <<EOF
keys:
- id: admin
  key: ${ADMINKEY}
  acls: [ "admin" ]
- id: operator
  key: ${OPKEY}
  acls: [ "operator" ]
EOF
  ) | ./bin/yml2mp | \
    ./sbin/bus -s ./var/bus.ipc rpc call eva.aaa.localauth key.deploy - > /dev/null || exit 12
  echo "Deplying HMI local users..."
  (
  cat <<EOF
users:
- login: operator
  password: ${OPPASSWD_HASHED}
  acls: [ "operator" ]
EOF
  ) | ./bin/yml2mp | \
    ./sbin/bus -s ./var/bus.ipc rpc call eva.aaa.localauth user.deploy - > /dev/null || exit 12
fi

if [ $MODE -ge 1 ]; then
  echo "Preparing Python venv and installing eva-shell..."
  EXTRA="[\"eva-shell\""
  EXTRA="${EXTRA}]"
  ./sbin/eva-registry-cli get eva/config/python-venv | \
    jq ".extra += ${EXTRA}" | \
    ./sbin/eva-registry-cli set eva/config/python-venv - -p json > /dev/null || exit 12
  ./sbin/venvmgr build || exit 12
fi

if [ "$LOGROTATE" ]; then
  if [ ! -d /etc/logrotate.d ]; then
    echo "[!] logrotate is not installed. Skipping log rotation setup"
  else
    sed "s|/opt/eva4|${PREFIX}|g" ./etc/logrotate.d/eva4 > /etc/logrotate.d/eva4
  fi
fi

if [ "$CREATE_SYMLINKS" ]; then
  ln -sf "$PREFIX"/bin/eva /usr/local/bin/eva
fi

if ! CURRENT_VERSION=$(./svc/eva-node --mode info|jq -r .version); then
  echo "Can't obtain current version"
  exit 1
fi
if ! CURRENT_BUILD=$(./svc/eva-node --mode info|jq -r .build); then
  echo "Can't obtain current build"
  exit 1
fi

echo
echo "EVA ICS version ${CURRENT_VERSION} build ${CURRENT_BUILD} installed. Type"
echo
[ "$CREATE_SYMLINKS" ] && echo "    eva" || echo "    $PREFIX/bin/eva"
echo
echo "to start EVA shell or use the bus client $PREFIX/sbin/bus to manage the node"

if [ "$HMI" ]; then
  echo
  echo "Controller default key: ${DEFAULTKEY}"
  echo
fi

if [ "$HMI" ]; then
  echo "HMI service is available at http://$(hostname):7727"
  echo
  echo "Admin key: ${ADMINKEY}"
  echo "Operator key: ${OPKEY}"
  echo
  echo "Default user: operator"
  echo "Password: ${OPPASSWD}"
fi

echo

exit 0
