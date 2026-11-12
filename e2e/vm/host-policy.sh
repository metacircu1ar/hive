#!/bin/bash
# Guest host-policy configurations for the Lima validation leg; run as root.
#
# e2e/scripts/vm_validation.py pipes this file to the guest as
# `sudo bash -s -- CONFIG USER`. It changes only the disposable guest, never the
# host. stock is the stock policy and needs no step. The configurations
# are additive and applied in order on one guest: stock, then distribution, then user. Applying a
# configuration again is a no-op reload; existing files must match exactly.
#
#   distribution         Load the distribution's bwrap-userns-restrict profile, the Codex
#              documentation's repair.
#   user         distribution plus the per-user Bubblewrap copy and exact-path profile from
#              Hive's docs/setup.md ("An administrator may explicitly grant a
#              per-user exception").
#   user-revert  Unload and remove exactly that exception.
#   distribution-revert  Unload and remove exactly the distribution copy.
set -euo pipefail

config=${1:?config}
user=${2:?user}
[ "$(id -u)" = 0 ] || { echo "host policy needs root" >&2; exit 2; }

distro=/usr/share/apparmor/extra-profiles/bwrap-userns-restrict
loaded=/etc/apparmor.d/bwrap-userns-restrict
scoped_root=/usr/local/libexec/hive-userns
scoped_dir=$scoped_root/$user
scoped_profile=/etc/apparmor.d/hive-$user-bwrap
scoped_name=hive_${user}_bwrap
profile_text="abi <abi/4.0>,
include <tunables/global>

profile $scoped_name $scoped_dir/bwrap flags=(unconfined) {
  userns,
}"

is_loaded() {
  grep -q "^$1 (" /sys/kernel/security/apparmor/profiles
}

case "$config" in
  distribution)
    [ -f "$distro" ] || { echo "missing $distro" >&2; exit 3; }
    if [ -e "$loaded" ]; then
      cmp -s "$distro" "$loaded" || { echo "refusing to replace $loaded" >&2; exit 3; }
    else
      install -m 0644 -o root -g root "$distro" "$loaded"
    fi
    apparmor_parser -r "$loaded"
    ;;
  distribution-revert)
    if [ -e "$loaded" ]; then
      cmp -s "$distro" "$loaded" || { echo "$loaded is not the distribution copy" >&2; exit 3; }
      if is_loaded bwrap; then apparmor_parser -R "$loaded"; fi
      rm "$loaded"
    fi
    ;;
  user|user-stock)
    if [ "$config" = user ]; then
      cmp -s "$distro" "$loaded" || { echo "user needs distribution loaded first" >&2; exit 3; }
    else
      is_loaded bwrap && is_loaded unpriv_bwrap \
        || { echo "user-stock needs both stock profiles loaded" >&2; exit 3; }
      [ ! -e "$loaded" ] || dpkg-query -S "$loaded" >/dev/null 2>&1 \
        || { echo "refusing unowned distribution profile" >&2; exit 3; }
    fi
    # Root-owned, non-writable parents; read/execute only for root and USER.
    install -d -m 0755 -o root -g root "$scoped_root"
    if [ -e "$scoped_dir" ]; then
      cmp /usr/bin/bwrap "$scoped_dir/bwrap"
    else
      install -d -m 0700 -o root -g root "$scoped_dir"
      install -m 0700 -o root -g root /usr/bin/bwrap "$scoped_dir/bwrap"
    fi
    setfacl -m "u:$user:rx" "$scoped_dir" "$scoped_dir/bwrap"
    if [ -e "$scoped_profile" ]; then
      [ "$(cat "$scoped_profile")" = "$profile_text" ] \
        || { echo "refusing to replace $scoped_profile" >&2; exit 3; }
    else
      printf '%s\n' "$profile_text" > "$scoped_profile"
      chmod 0644 "$scoped_profile"
    fi
    apparmor_parser -r "$scoped_profile"
    ;;
  user-revert)
    if [ -e "$scoped_profile" ]; then
      [ "$(cat "$scoped_profile")" = "$profile_text" ] \
        || { echo "$scoped_profile is not this exception" >&2; exit 3; }
      if is_loaded "$scoped_name"; then apparmor_parser -R "$scoped_profile"; fi
      rm "$scoped_profile"
    fi
    if [ -e "$scoped_dir" ]; then
      cmp /usr/bin/bwrap "$scoped_dir/bwrap"
      rm "$scoped_dir/bwrap"
      rmdir "$scoped_dir"
    fi
    if [ -d "$scoped_root" ]; then rmdir "$scoped_root"; fi
    ;;
  *)
    echo "unknown config: $config" >&2
    exit 2
    ;;
esac
