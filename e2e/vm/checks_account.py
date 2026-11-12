"""A non-PAM guest account for self-contained CI and no-service installation checks."""

import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import shlex
import stat

NAME = "hive-checks"
HOME = "/home/hive-checks"
MARKER = "Hive isolated validation"
PATH = HOME + "/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
ASSETS = {"$HOME/src/hive/dist/vm-assets", "$HOME/assets-container"}

PREPARE = r'''set -euo pipefail
origin=$1; assets=$2; commit=$3
name=hive-checks; home=/home/hive-checks
if ! getent passwd "$name" >/dev/null; then
  test ! -e "$home"
  useradd --create-home --home-dir "$home" --shell /usr/sbin/nologin \
    --comment 'Hive isolated validation' --password '!' "$name"
  chmod 700 "$home"
fi
python3 -I - <<'EOF'
import os,pwd,stat
from pathlib import Path
entry=pwd.getpwnam('hive-checks'); home=Path('/home/hive-checks')
info=home.lstat()
if (entry.pw_uid==0 or entry.pw_name!='hive-checks' or entry.pw_dir!=str(home)
    or entry.pw_gecos!='Hive isolated validation' or entry.pw_shell!='/usr/sbin/nologin'
    or not stat.S_ISDIR(info.st_mode) or info.st_uid!=entry.pw_uid
    or stat.S_IMODE(info.st_mode)!=0o700):
    raise RuntimeError('Unexpected isolated validation account')
runtime=Path('/run/user')/str(entry.pw_uid)
if runtime.exists() or runtime.is_symlink():
    info=runtime.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid!=entry.pw_uid or stat.S_IMODE(info.st_mode)!=0o700:
        raise RuntimeError('Unexpected isolated runtime directory')
EOF
uid=$(id -u "$name"); gid=$(id -g "$name")
install -d -m 700 -o "$uid" -g "$gid" "/run/user/$uid"
source="$origin/src/hive"; checkout="$home/src/hive"
test "$(git -c safe.directory="$source" -C "$source" rev-parse HEAD)" = "$commit"
if test ! -e "$checkout"; then
  install -d -m 700 -o "$uid" -g "$gid" "$home/src"
  git -c safe.directory="$source" clone -q --no-hardlinks "$source" "$checkout"
  git -c safe.directory="$checkout" -C "$checkout" checkout -q --detach "$commit"
  chown -hR "$uid:$gid" "$checkout"
fi
if test ! -e "$home/assets"; then
  cp -a "$assets" "$home/assets"
  chown -hR "$uid:$gid" "$home/assets"
fi
diff -qr "$assets" "$home/assets"
setpriv --reuid "$uid" --regid "$gid" --init-groups --bounding-set=-all \
  --inh-caps=-all --ambient-caps=-all --no-new-privs \
  env -i HOME="$home" USER="$name" LOGNAME="$name" XDG_RUNTIME_DIR="/run/user/$uid" \
  PATH="$home/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
  python3 - "$checkout" "$commit" <<'EOF'
import json,os,sys
os.chdir(sys.argv[1]);sys.path.insert(0,sys.argv[1])
from e2e.vm.checks_account import native_context
from release.progress import entry
def main():
    print('HIVE_VM_JSON '+json.dumps(native_context(sys.argv[2])),flush=True)
raise SystemExit(entry(main,name='verify'))
EOF
'''

COLLECT = r'''set -euo pipefail
origin=$1; relative=$2; home=$3
source="$home/src/hive/$relative"
destination="$origin/src/hive/$relative"
if test ! -e "$source" && test ! -L "$source"; then
  printf 'HIVE_VM_NO_SUITE_OUTPUT\n'
  exit 0
fi
test -d "$source" && test ! -L "$source"
test -z "$(find "$source" -type l -print -quit)"
test -z "$(find "$source" ! -type d ! -type f -print -quit)"
test ! -e "$destination" && test ! -L "$destination"
cp -a "$source" "$destination"
identity=$(python3 -I -c 'import os,sys; s=os.stat(sys.argv[1]); print(str(s.st_uid)+":"+str(s.st_gid))' "$origin")
chown -hR "$identity" "$destination"
printf 'HIVE_VM_SUITE_OUTPUT_EXPORTED\n'
'''


def validate_context(value):
    uid, gid = value["uid"], value["gid"]
    if (
        type(uid) is not int or uid <= 0 or type(gid) is not int or gid <= 0
        or value["resuid"] != [uid] * 3 or value["resgid"] != [gid] * 3
        or value["groups"] != [gid] or value["user"] != NAME or value["home"] != HOME
        or value["runtime"] != f"/run/user/{uid}"
        or set(value["capabilities"]) != {"CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb"}
        or any(value["capabilities"].values()) or value["inherited_session"]
    ):
        raise ValueError("Isolated checks require their own unprivileged account context")
    return value


def native_context(commit):
    """Observe the identity that actually executes the no-service checks."""
    from release import asset_set
    from release.process import run

    uid, gid = os.getuid(), os.getgid()
    account = pwd.getpwuid(uid)
    if account.pw_name != NAME or account.pw_dir != HOME or account.pw_gecos != MARKER:
        raise ValueError("Isolated checks account differs from its passwd entry")
    capabilities = {}
    for line in Path("/proc/self/status").read_text().splitlines():
        key, separator, value = line.partition(":")
        if separator and key in {"CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb"}:
            capabilities[key] = int(value.strip(), 16)
    for name in ("HOME", "XDG_RUNTIME_DIR"):
        info = Path(os.environ[name]).lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != uid or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError("Isolated checks need private account-owned directories")
    value = validate_context(dict(
        uid=uid, gid=gid, resuid=list(os.getresuid()), resgid=list(os.getresgid()),
        groups=os.getgroups(), user=os.environ["USER"], home=os.environ["HOME"],
        runtime=os.environ["XDG_RUNTIME_DIR"], capabilities=capabilities,
        inherited_session=any(name in os.environ for name in (
            "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS", "XDG_SESSION_ID", "SSH_AUTH_SOCK",
        )),
    ))
    actual = run(["git", "rev-parse", "HEAD"], text=True, check=True).stdout.strip()
    dirty = run(["git", "status", "--porcelain"], check=True).stdout
    if actual != commit or dirty:
        raise ValueError("Isolated checks do not have the clean pinned source")
    assets = Path(HOME) / "assets"
    if asset_set.load(assets) is None:
        raise ValueError("Isolated checks lack the exact packaged asset ledger")
    return dict(value, commit=actual, assets_sha256=hashlib.sha256((assets / "assets.json").read_bytes()).hexdigest())


def prepare(campaign):
    from e2e.vm.checks import marked_json

    origin = campaign.guest.value("checks-origin-home", 'printf "%s" "$HOME"').strip()
    if not origin.startswith("/") or ":" in origin or "\n" in origin:
        raise ValueError("Unexpected guest home")
    assets = campaign.state["units"]["package"]["assets"]
    if assets not in ASSETS:
        raise ValueError("Unknown packaged assets for isolated checks")
    commit = campaign.state["commit"]
    if re.fullmatch("[a-f0-9]{40}", commit) is None:
        raise ValueError("Isolated checks require a pinned source commit")
    _, done = campaign.guest.run(
        "checks-account", PREPARE, root=True,
        args=(origin, assets.replace("$HOME", origin, 1), commit), timeout=300,
    )
    value = validate_context(marked_json(done.stdout))
    if value["commit"] != commit:
        raise ValueError("Isolated checks returned a different source commit")
    return dict(value, origin_home=origin)


def run(campaign, label, script, *, timeout, rust=False):
    context = prepare(campaign)
    uid, gid = context["uid"], context["gid"]
    rust_setup = r'''if test ! -x "$HOME/.cargo/bin/cargo"; then
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
  --default-toolchain 1.98.1 --component rustfmt,clippy
fi
''' if rust else ""
    if "HIVE_ISOLATED_CHECKS_END" in script:
        raise ValueError("Isolated checks script contains its reserved delimiter")
    body = f'''set -euo pipefail
setpriv --reuid {uid} --regid {gid} --init-groups --bounding-set=-all \\
 --inh-caps=-all --ambient-caps=-all --no-new-privs \\
 env -i HOME={shlex.quote(HOME)} USER={NAME} LOGNAME={NAME} \\
 XDG_RUNTIME_DIR=/run/user/{uid} PATH={shlex.quote(PATH)} \\
 RUST_TEST_THREADS=4 CARGO_BUILD_JOBS=4 bash -s <<'HIVE_ISOLATED_CHECKS_END'
set -euo pipefail
cd "$HOME/src/hive"
{rust_setup}{script}
HIVE_ISOLATED_CHECKS_END
'''
    record, done = campaign.guest.run(label, body, root=True, require=False, timeout=timeout)
    return record, done, context


def collect(campaign, context, relative, *, verify_assets):
    if re.fullmatch(r"dist/vm-suite-[1-9][0-9]*", relative) is None:
        raise ValueError("Isolated checks export requires its exact suite directory")
    _, done = campaign.guest.run(
        "checks-export", COLLECT, root=True,
        args=(context["origin_home"], relative, HOME), timeout=300,
    )
    receipt = done.stdout.strip()
    if receipt == "HIVE_VM_NO_SUITE_OUTPUT":
        return False
    if receipt != "HIVE_VM_SUITE_OUTPUT_EXPORTED":
        raise ValueError("Isolated checks export lacks its completion receipt")
    if verify_assets:
        campaign.guest.run("checks-export-assets", f'''set -euo pipefail
cd "$HOME/src/hive"
python3 - {shlex.quote(relative)} <<'EOF'
from pathlib import Path
import sys
sys.path.insert(0,'scripts')
from release import asset_set
from release.progress import entry
def main():
    root=Path(sys.argv[1])/'assets'
    if not root.is_dir() or asset_set.load(root) is None:
        raise ValueError('Exported CI assets lack a coherent ledger')
raise SystemExit(entry(main,name='verify'))
EOF
''', timeout=120)
    return True
