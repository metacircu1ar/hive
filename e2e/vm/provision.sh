#!/bin/bash
# Guest provisioning for the Lima host-context validation leg.
#
# e2e/scripts/vm_validation.py pipes this file to the guest as
# `bash -s -- PROFILE USER`: root profiles go through sudo, the user profile does
# not. The guest stays a stock Ubuntu cloud image plus the named packages. This
# script loads no AppArmor profile, changes no sysctl and copies no credential.
#
#   base        probe and later: Bubblewrap, socat and the AppArmor tools and extra
#               profiles (installed, not loaded); linger for USER.
#   build-root  full run: the CI container's package list, plus acl for user.
#   build-user  full run: the pinned Rust toolchain through rustup.
set -euo pipefail

profile=${1:?profile}
user=${2:?user}
export DEBIAN_FRONTEND=noninteractive
# First boot may still be running apt-daily or unattended-upgrades.
apt=(apt-get -o DPkg::Lock::Timeout=600 -o APT::Get::Assume-Yes=true)

case "$profile" in
  base)
    [ "$(id -u)" = 0 ] || { echo "base needs root" >&2; exit 2; }
    "${apt[@]}" update
    "${apt[@]}" install --no-install-recommends bubblewrap socat apparmor-utils apparmor-profiles
    loginctl enable-linger "$user"
    ;;
  build-root)
    [ "$(id -u)" = 0 ] || { echo "build-root needs root" >&2; exit 2; }
    # Mirrors e2e/ci/Dockerfile; acl is for the user scoped exception.
    "${apt[@]}" install --no-install-recommends \
      build-essential pkg-config libssl-dev cmake git curl ca-certificates \
      python3 python3-venv tmux xz-utils unzip file procps acl
    ;;
  build-user)
    [ "$(id -u)" != 0 ] || { echo "build-user must not run as root" >&2; exit 2; }
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain 1.98.1 --component rustfmt,clippy
    ;;
  *)
    echo "unknown profile: $profile" >&2
    exit 2
    ;;
esac
