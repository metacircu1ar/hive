"""Guest confinement, plain release assembly and policy matrix checks."""

import errno
import json
import os
import re
import shlex
import time

from e2e.paths import product_root
from e2e.vm.context import current, emit, fail, guard, host_run, guest_script
from e2e.vm.guest import Guest
from release.files import atomic_write_json, create_directory
from release.process import ProcessFailure, checked
from release.progress import cleanup_all, finalizer
from release.cancellation import pause

#!/usr/bin/env python3


GIB = 1 << 30
MIN_FREE = 10 * GIB

PROBE = (
    "/usr/bin/bwrap",
    "--unshare-user",
    "--unshare-net",
    "--ro-bind",
    "/",
    "/",
    "--",
    "/usr/bin/true",
)
SERVICE = ("systemd-run", "--user", "--wait", "--pipe", "--collect")
RTM_NEWADDR = "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted"
DISTRO_PROFILES = ("bwrap", "unpriv_bwrap")
# Kernel audit records reach the journal asynchronously.
AUDIT_SETTLE_SECONDS = 2.0
SYSCTLS = (
    "kernel.apparmor_restrict_unprivileged_userns",
    "kernel.unprivileged_userns_clone",
    "kernel.apparmor_restrict_unprivileged_unconfined",
    "user.max_user_namespaces",
)
PACKAGES = (
    "bubblewrap",
    "socat",
    "apparmor",
    "apparmor-utils",
    "apparmor-profiles",
    "libapparmor1",
    "systemd",
    "dbus-user-session",
)
PACKAGE_QUERY = "dpkg-query -W -f='${Package}\\t${Version}\\t${db:Status-Abbrev}\\n' " + " ".join(
    PACKAGES
)
PROFILES = "cat /sys/kernel/security/apparmor/profiles"
GUEST_CLOCK = "date +%s%6N"
WATCH_SECONDS = 15
# The isolated account's home is on the guest disk, while /tmp may be a small
# tmpfs. Retain this private per-attempt directory until the VM is deleted.
SUITE_TMPDIR = 'TMPDIR="$(mktemp -d "$HOME/.hive-suite-XXXXXXXX")"\nexport TMPDIR'


# (key, script, root, required)
PREFLIGHT = (
    ("os_release", "cat /etc/os-release", False, True),
    ("kernel", "uname -srvm", False, True),
    ("cmdline", "cat /proc/cmdline", False, True),
    ("lsm", "cat /sys/kernel/security/lsm", True, True),
    ("aa_enabled", "aa-status --enabled", True, False),
    ("aa_profiles", PROFILES, True, True),
    *((f"sysctl:{name}", f"sysctl -n {name}", False, False) for name in SYSCTLS),
    ("bwrap_version", "bwrap --version", False, False),
    ("packages", PACKAGE_QUERY, False, False),
    ("kernel_package", 'dpkg-query -S "/boot/vmlinuz-$(uname -r)"', False, False),
    ("identity", "id", False, True),
    ("linger", 'loginctl show-user "$(id -un)" --property=Linger', False, False),
    ("linger_files", "ls -1 /var/lib/systemd/linger", False, False),
    ("session_xdg_runtime_dir", 'printf "%s\\n" "$HIVE_VM_SESSION_XDG_RUNTIME_DIR"', False, True),
    ("exported_xdg_runtime_dir", 'printf "%s\\n" "$XDG_RUNTIME_DIR"', False, True),
    ("user_manager", "systemctl --user is-system-running", False, False),
    ("systemd_version", "systemctl --version", False, True),
)
CIDATA = r"""
dir=/mnt/lima-cidata
if ! findmnt -rn "$dir" >/dev/null; then
  dir=/run/hive-vm-cidata
  mkdir -p "$dir"
  mount -o ro /dev/disk/by-label/cidata "$dir"
fi
printf 'DIR\t%s\n' "$dir"
find "$dir" -maxdepth 2 | sort
printf 'PLAIN\t%s\n' "$(grep -E '^LIMA_CIDATA_PLAIN=' "$dir/lima.env" || true)"
printf 'MATCHES\n'
grep -rIn -E 'apparmor|userns|sysctl' "$dir" || true
"""
SYSCTL_FILES = r"""
for f in $(grep -rIl -E 'userns' /etc/sysctl.conf /etc/sysctl.d /usr/lib/sysctl.d /run/sysctl.d /usr/local/lib/sysctl.d 2>/dev/null); do
  owner=$(dpkg-query -S "$f" 2>/dev/null) || owner=UNOWNED
  printf 'FILE\t%s\t%s\n' "$f" "${owner%%:*}"
  grep -n userns "$f"
done
"""
# What Lima's non-essential boot scripts write; plain mode skips those scripts.
LIMA_WRITTEN = r"""
for f in /etc/sysctl.d/99-lima.conf /etc/apparmor.d/local/fusermount3 \
    /etc/apparmor.d/usr.local.bin.rootlesskit /etc/systemd/system/user@.service.d/lima.conf; do
  [ ! -e "$f" ] || printf '%s\n' "$f"
done
"""
APPARMOR_UNOWNED = r"""
for f in /etc/apparmor.d/*; do
  [ -f "$f" ] || continue
  dpkg-query -S "$f" >/dev/null 2>&1 || printf '%s\n' "$f"
done
"""
LIMA_CHECKS = (
    (
        "host_mounts",
        "findmnt -rn -o TARGET,FSTYPE,SOURCE -t virtiofs,9p,fuse.sshfs,nfs,nfs4,cifs",
        False,
        False,
    ),
    ("cidata", CIDATA, True, True),
    ("sysctl_userns_files", SYSCTL_FILES, True, True),
    ("apparmor_unowned", APPARMOR_UNOWNED, True, True),
    ("lima_written_files", LIMA_WRITTEN, True, True),
    ("container_binaries", "command -v nerdctl containerd rootlesskit buildkitd", False, False),
    ("usr_local", "ls -1A /usr/local/bin /usr/local/sbin", False, False),
    ("lima_units", "systemctl list-units --all --no-legend --plain 'lima*'", False, False),
)


def lines(text):
    return [line for line in text.splitlines() if line.strip()]


def iso_now():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def profile_modes(text):
    modes = {}
    for line in lines(text):
        name, separator, mode = line.rpartition(" (")
        if not separator or not mode.endswith(")"):
            fail("Unexpected AppArmor profile listing", line=line[:160])
        modes[name] = mode[:-1]
    return modes


def audit_lines(guest, name, start_us, end_us):
    """AppArmor records from every journal transport inside one probe's window."""
    script = (
        f"journalctl --no-pager -o json --since @{start_us // 1_000_000}"
        f" --until @{end_us // 1_000_000 + 1}"
    )
    record, done = guest.run(name, script, root=True, keep_output=False)
    found = []
    for raw in lines(done.stdout):
        row = json.loads(raw)
        message = row.get("MESSAGE")
        if isinstance(message, list):
            message = bytes(message).decode("utf-8", errors="replace")
        stamp = int(row["__REALTIME_TIMESTAMP"])
        if isinstance(message, str) and "apparmor=" in message and start_us <= stamp <= end_us:
            found.append(
                {"realtime_us": stamp, "transport": row.get("_TRANSPORT"), "message": message}
            )
    return {"record": record, "lines": found}


def probe(guest, context, label):
    """The exact Bubblewrap command, its context's label and its audit records."""
    prefix = SERVICE if context == "service" else ()
    start = int(guest.value(f"{label}-clock", GUEST_CLOCK))
    attr, attr_done = guest.run(
        f"{label}-label",
        "exec " + shlex.join([*prefix, "/bin/cat", "/proc/self/attr/current"]),
        require=False,
    )
    result, done = guest.run(
        f"{label}-bwrap", "exec " + shlex.join([*prefix, *PROBE]), require=False
    )
    pause(AUDIT_SETTLE_SECONDS)
    end = int(guest.value(f"{label}-clock-end", GUEST_CLOCK))
    audit = audit_lines(guest, f"{label}-audit", start, end)
    return {
        "context": context,
        "command": shlex.join([*prefix, *PROBE]),
        "returncode": done.returncode,
        "stdout": done.stdout,
        "stderr": done.stderr,
        "bwrap_lines": [line for line in lines(done.stderr) if line.startswith("bwrap:")],
        "attr_current": attr_done.stdout.strip() if attr_done.returncode == 0 else None,
        "attr_record": attr,
        "record": result,
        "audit": audit["lines"],
        "audit_record": audit["record"],
        "window_us": [start, end],
    }


def reproduces(row):
    return row["returncode"] != 0 and RTM_NEWADDR in row["stderr"]


def facts(guest, table, prefix):
    records = {}
    for key, script, root, required in table:
        records[key] = guest.run(
            f"{prefix}-{key.replace(':', '-')}", script, root=root, require=required
        )[0]
    return records


def os_release(text):
    fields = {}
    for line in lines(text):
        key, _, value = line.partition("=")
        fields[key] = value.strip('"')
    return fields


def packages(text):
    found = {}
    for line in lines(text):
        name, version, status = (line.split("\t") + ["", ""])[:3]
        found[name] = {"version": version, "status": status.strip()}
    return found


def instance_disk(status):
    allocated = apparent = 0
    for root, _, names in os.walk(status["dir"]):
        for name in names:
            stat = os.lstat(os.path.join(root, name))
            allocated += stat.st_blocks * 512
            apparent += stat.st_size
    return {"dir": status["dir"], "allocated_bytes": allocated, "apparent_bytes": apparent}


def provision(guest, report):
    user = guest.value("whoami", "id -un").strip()
    cloud_init = guest.run(
        "cloud-init-wait", "cloud-init status --wait", root=True, require=False, timeout=900
    )[0]
    before = {
        "packages": packages(guest.value("packages-before", PACKAGE_QUERY, require=False)),
        "profiles": profile_modes(guest.value("profiles-before", PROFILES, root=True)),
    }
    started = time.monotonic()
    record = guest.run(
        "provision-base", guest_script("provision.sh"), root=True, args=("base", user), timeout=1800
    )[0]
    seconds = round(time.monotonic() - started, 1)
    manager = guest.run(
        "user-manager-wait",
        "timeout 120 systemctl --user is-system-running --wait",
        require=False,
        timeout=180,
    )[0]
    after = {
        "packages": packages(guest.value("packages-after", PACKAGE_QUERY, require=False)),
        "profiles": profile_modes(guest.value("profiles-after", PROFILES, root=True)),
    }
    report["provision"] = {
        "user": user,
        "cloud_init": cloud_init,
        "script": "vm/provision.sh base",
        "record": record,
        "seconds": seconds,
        "user_manager_wait": manager,
        "packages_before": before["packages"],
        "packages_after": after["packages"],
        "profiles_added_by_packages": sorted(set(after["profiles"]) - set(before["profiles"])),
        "profiles_removed_by_packages": sorted(set(before["profiles"]) - set(after["profiles"])),
    }
    return user, after["profiles"]


def preflight(guest, report):
    records = facts(guest, PREFLIGHT, "preflight")
    release = os_release(records["os_release"]["stdout"])
    profiles = profile_modes(records["aa_profiles"]["stdout"])
    sysctls = {
        name: records[f"sysctl:{name}"]["stdout"].strip()
        if records[f"sysctl:{name}"]["returncode"] == 0
        else None
        for name in SYSCTLS
    }
    report["preflight"] = {
        "release": release.get("PRETTY_NAME"),
        "version_id": release.get("VERSION_ID"),
        "kernel": records["kernel"]["stdout"].strip(),
        "cmdline": records["cmdline"]["stdout"].strip(),
        "lsm": records["lsm"]["stdout"].strip(),
        "apparmor_enabled": records["aa_enabled"]["returncode"] == 0,
        "profiles": profiles,
        "profile_count": len(profiles),
        "distro_bwrap_profiles_loaded": [name for name in DISTRO_PROFILES if name in profiles],
        "sysctls": sysctls,
        "bwrap_version": records["bwrap_version"]["stdout"].strip(),
        "packages": packages(records["packages"]["stdout"]),
        "kernel_package": records["kernel_package"]["stdout"].strip(),
        "identity": records["identity"]["stdout"].strip(),
        "linger": records["linger"]["stdout"].strip(),
        "linger_files": lines(records["linger_files"]["stdout"]),
        "session_xdg_runtime_dir": records["session_xdg_runtime_dir"]["stdout"].strip(),
        "exported_xdg_runtime_dir": records["exported_xdg_runtime_dir"]["stdout"].strip(),
        "user_manager": records["user_manager"]["stdout"].strip(),
        "systemd_version": lines(records["systemd_version"]["stdout"])[0],
        "records": records,
    }
    return profiles


def lima_additions(guest, report):
    records = facts(guest, LIMA_CHECKS, "lima")
    cidata = lines(records["cidata"]["stdout"])
    matches = cidata[cidata.index("MATCHES") + 1 :] if "MATCHES" in cidata else cidata
    sysctl_files = [
        line.split("\t")[1:]
        for line in lines(records["sysctl_userns_files"]["stdout"])
        if line.startswith("FILE\t")
    ]
    found = {
        "host_mounts": lines(records["host_mounts"]["stdout"]),
        "unowned_apparmor_files": lines(records["apparmor_unowned"]["stdout"]),
        "userns_sysctl_files": sysctl_files,
        "unowned_userns_sysctl_files": [path for path, owner in sysctl_files if owner == "UNOWNED"],
        "container_binaries": lines(records["container_binaries"]["stdout"]),
        "lima_written_files": lines(records["lima_written_files"]["stdout"]),
        "cidata_plain": [line.split("\t", 1)[1] for line in cidata if line.startswith("PLAIN\t")]
        == ["LIMA_CIDATA_PLAIN=1"],
        "cidata_policy_lines": matches,
        "cidata_userns_or_apparmor_lines": [
            line for line in matches if "userns" in line or "apparmor" in line
        ],
        "lima_units": lines(records["lima_units"]["stdout"]),
        "records": records,
    }
    # cidata lines are evidence for review: in plain mode boot.sh runs only the
    # (Alpine-only) boot.essential scripts and skips boot.Linux, which would add a
    # sysctl file, AppArmor rules and containerd. The installed effects decide.
    found["clean"] = found["cidata_plain"] and not (
        found["host_mounts"]
        or found["unowned_apparmor_files"]
        or found["unowned_userns_sysctl_files"]
        or found["container_binaries"]
        or found["lima_written_files"]
    )
    report["lima_additions"] = found


def distribution_check(guest, report, user, baseline):
    """Temporarily load the distribution profile, retaining this guest's baseline."""
    policy = guest_script("host-policy.sh")
    files = guest.value(
        "distribution-existing", "test ! -e /etc/apparmor.d/bwrap-userns-restrict && echo absent"
    )
    if any("bwrap" in name for name in baseline) or files.strip() != "absent":
        fail("Refusing to replace pre-existing stock Bubblewrap policy")
    with finalizer(
        lambda: guest.run(
            "distribution-revert", policy, root=True, args=("distribution-revert", user)
        )
    ):
        guest.run("distribution-load", policy, root=True, args=("distribution", user))
        loaded = profile_modes(guest.value("distribution-profiles", PROFILES, root=True))
        repaired = probe(guest, "service", "distribution-service")
    restored = profile_modes(guest.value("distribution-reverted-profiles", PROFILES, root=True))
    again = probe(guest, "service", "distribution-reverted-service")
    result = {
        "loaded_profiles": {name: loaded.get(name) for name in DISTRO_PROFILES},
        "service_probe": repaired,
        "restored_profile_set": restored == baseline,
        "reverted_service_probe": again,
    }
    result["passed"] = (
        all(result["loaded_profiles"].values())
        and repaired["returncode"] == 0
        and result["restored_profile_set"]
        and probe_class(again) == probe_class(report["probes"]["service"])
    )
    report["distribution_check"] = result


# Units finish in order and are recorded in OUT/state.json; `--resume` skips the
# finished ones after a stop for a busy host. Policy steps are idempotent.
PARTS = ("hive", "hive-core", "comb", "comb-llama", "hive-install")
CONFIGS = ("stock", "distribution", "user")
SETUP_UNITS = (
    "provision",
    "rust",
    "providers",
    "sources",
    *(f"build:{part}" for part in PARTS),
    "test-build",
    "package",
    "install",
)
# The whole campaign in order; `--through NAME` stops after NAME, `--resume` continues.
SEQUENCE = (*SETUP_UNITS, *CONFIGS, "finish")
SEQUENCE_PHASES = {
    "provision": "install",
    "rust": "install",
    "providers": "install",
    "sources": "install",
    "build": "build",
    "test-build": "build",
    "package": "package",
    "install": "install",
    "stock": "verify",
    "distribution": "verify",
    "user": "verify",
    "finish": "verify",
}
SRC = '"$HOME/src/hive"'
CARGO_PATH = "$HOME/.cargo/bin:$HOME/providers/bin:/usr/local/bin:/usr/bin:/bin"
SCOPED_DIR = "/usr/local/libexec/hive-userns/$(id -un)"
HIVECTL = '"$HOME/hive-prefix/bin/hivectl"'
JSON_MARK = "HIVE_VM_JSON "
CLAUDE_BASE = "https://downloads.claude.ai/claude-code-releases"
CLAUDE_FLOOR = (2, 1, 318)
GLIBC_FLOOR = (2, 35)
# Fallback assets may predate HEAD only by release-contract files, never by code.
CONTRACT_ONLY = re.compile(r"(^|/)compatibility(\.rs$|/)|^compat/|^docs/|^AGENTS\.md$")
# Exact setup flags from docs/setup.md; models are its examples, unused without login.
SETUP_FLAGS = (
    "--provider",
    "codex,claude",
    "--default-provider",
    "codex",
    "--login",
    "skip",
    "--codex-model",
    "gpt-6-luna",
    "--codex-effort",
    "medium",
    "--claude-model",
    "sonnet",
    "--claude-effort",
    "low",
    "--no-comb",
)
ACCEPTANCE = {
    "codex": (
        "HIVE_NATIVE_CODEX",
        "$HOME/providers/bin/codex",
        "native_sandbox::native_sandbox_acceptance_without_authentication",
    ),
    "claude": (
        "HIVE_NATIVE_CLAUDE",
        "$HOME/providers/bin/claude",
        "native_sandbox::claude_sandbox_acceptance_without_authentication",
    ),
}

CONTEXTS = {
    "shell": "",
    "service": shlex.join(SERVICE) + ' --working-directory="$HOME/src/hive"',
    "vscode": "/usr/bin/aa-exec -p vscode --",
}
NOBLE_EXPECTED = {
    "stock": {
        "probe": "fail_rtm_newaddr",
        "native_sandbox:daemon": "error",
        "claude_sandbox:daemon": "error",
        "codex": "fail",
        "claude": "fail",
        "release_install": "pass",
    },
    "distribution": {
        "probe": "pass",
        "native_sandbox:daemon": "ok",
        "claude_sandbox:daemon": "error",
        "codex": "pass",
        "claude": "fail",
        "helper_system_bwrap": "setgroups_denied",
    },
    "user": {
        "probe": "pass",
        "native_sandbox:daemon": "ok",
        "claude_sandbox:daemon": "ok",
        "codex": "pass",
        "claude": "pass",
        "helper_system_bwrap": "setgroups_denied",
        "controls": {"write": errno.EROFS, "unix_socket": errno.EPERM, "userns": errno.EPERM},
    },
}

PROVIDERS = r"""set -euo pipefail
bin="$HOME/providers/bin"; work="$HOME/providers/downloads"
rm -rf "$work"; mkdir -p "$bin" "$work"; cd "$work"
started=$(date -u +%FT%TZ)
curl -fsSL -o codex-release.json https://api.github.com/repos/openai/codex/releases/latest
python3 -I - codex-release.json "$1" > codex.select <<'EOF'
import json, sys
release = json.load(open(sys.argv[1]))
[asset] = [a for a in release['assets'] if a['name'] == sys.argv[2]]
print(release['tag_name'], asset['browser_download_url'], asset['digest'], asset['size'])
EOF
read -r tag url digest size < codex.select
case "$digest" in sha256:*) ;; *) echo "no published Codex digest" >&2; exit 3;; esac
curl -fsSL -o codex.tar.gz "$url"
printf '%s  codex.tar.gz\n' "${digest#sha256:}" | sha256sum -c -
mkdir codex && tar -xzf codex.tar.gz -C codex
member=$(find codex -type f -name 'codex*' -print -quit)
install -m 0755 "$member" "$bin/codex"
version=$(curl -fsSL "$2/latest")
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "unexpected Claude version" >&2; exit 4; }
curl -fsSL -o claude-manifest.json "$2/$version/manifest.json"
python3 -I -c 'import json, sys; p = json.load(open(sys.argv[1]))["platforms"][sys.argv[2]]; print(p["checksum"], p["size"])' \
  claude-manifest.json "$3" > claude.select
read -r checksum csize < claude.select
curl -fsSL -o claude "$2/$version/$3/claude"
printf '%s  claude\n' "$checksum" | sha256sum -c -
install -m 0755 claude "$bin/claude"
codex_version=$("$bin/codex" --version)
claude_version=$(DISABLE_AUTOUPDATER=1 "$bin/claude" --version)
python3 -I -c 'import json, sys; print("HIVE_VM_JSON " + json.dumps(dict(zip(sys.argv[1::2], sys.argv[2::2]))))' \
  started "$started" codex_tag "$tag" codex_url "$url" codex_digest "$digest" codex_bytes "$size" \
  codex_member "${member#codex/}" codex_version "$codex_version" claude_channel latest \
  claude_release "$version" claude_sha256 "$checksum" claude_bytes "$csize" claude_version "$claude_version"
cd "$HOME"; rm -rf "$work"
"""
BUILD_RECORD = r"""python3 -I - "$1" <<'EOF'
import json, sys
b = json.load(open(sys.argv[1]))
print('HIVE_VM_JSON ' + json.dumps({
    'build_seconds': b['build_seconds'], 'source': b['source'],
    'binaries': {name: {'sha256': r['sha256'], 'bytes': r['bytes'], 'version': r['version'],
                        'required_glibc': r['dependencies']['required_glibc']}
                 for name, r in b['binaries'].items()}}))
EOF
"""
# Inside the Claude helper: a forbidden write, a Unix socket and a nested user namespace.
CONTROLS = """import ctypes, json, os, socket
result = {}
def attempt(name, operation):
    try:
        operation()
        result[name] = 0
    except OSError as error:
        result[name] = error.errno
attempt('write', lambda: open('/var/tmp/hive-vm-control', 'w'))
attempt('unix_socket', lambda: socket.socket(socket.AF_UNIX, socket.SOCK_STREAM))
def nested_userns():
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.unshare(0x10000000) != 0:
        raise OSError(ctypes.get_errno(), 'user namespace refused')
attempt('userns', nested_userns)
print('HIVE_VM_CONTROLS ' + json.dumps(result))
"""
HELPER = (
    "--unshare-user",
    "--unshare-net",
    "--unshare-pid",
    "--cap-drop",
    "ALL",
    "--ro-bind",
    "/",
    "/",
    "--dev",
    "/dev",
    "--proc",
    "/proc",
    "--",
    "/usr/bin/env",
    "ARGV0=apply-seccomp",
)


def parse_json(text, step):
    try:
        return json.loads(text)
    except ValueError:
        fail("A guest step did not return valid JSON", step=step)


def marked_json(text):
    found = [line[len(JSON_MARK) :] for line in text.splitlines() if line.startswith(JSON_MARK)]
    if len(found) != 1:
        fail("A guest step did not report exactly one JSON record")
    return parse_json(found[0], "marked record")


def version_tuple(text):
    match = re.search(r"(\d+)\.(\d+)\.(\d+)", text)
    return tuple(int(group) for group in match.groups()) if match else None


def glibc_tuple(text):
    return tuple(int(part) for part in text.split(".")) if text else (0,)


class Campaign:
    def __init__(self, args, out, state):
        self.args, self.out, self.state = args, out, state
        attempt = len(state["attempts"]) + 1
        state["attempts"].append(
            {"number": attempt, "started_at": iso_now(), "outcome": "incomplete"}
        )
        create_directory(out / f"attempt-{attempt}" / "logs")
        self.guest = Guest(current(), out / f"attempt-{attempt}", guard)
        self.attempt = attempt
        self.user = None

    def save(self):
        atomic_write_json(self.out / "state.json", self.state)

    def finished(self, name):
        if name in CONFIGS:
            return name in self.state["configs"]
        return name in self.state["units"] or (
            name == "finish" and self.state["finish"] is not None
        )

    def step(self, name):
        if self.finished(name):
            return
        if name in CONFIGS:
            self.snapshot_baseline()
            emit(config=name, status="started")
            self.state["configs"][name] = self.config(name)
        elif name == "finish":
            self.state["finish"] = self.finish()
        else:
            work = {
                "provision": self.provision,
                "rust": self.rust,
                "providers": self.providers,
                "sources": self.sources,
                "test-build": self.test_build,
                "package": self.package,
                "install": self.install,
            }.get(name) or (lambda: self.build(name.split(":", 1)[1]))
            self.unit(name, work)
            return
        self.save()

    def policy_state(self, label):
        profiles = profile_modes(self.guest.value(f"{label}-profiles", PROFILES, root=True))
        leftovers = lines(
            self.guest.value(
                f"{label}-policy-files",
                "ls -d /etc/apparmor.d/bwrap-userns-restrict "
                "/etc/apparmor.d/hive-*-bwrap /usr/local/libexec/hive-userns 2>/dev/null || true",
            )
        )
        return {
            "profile_count": len(profiles),
            "policy_files": leftovers,
            "bwrap_profiles": sorted(name for name in profiles if "bwrap" in name),
            "sysctls": {
                name: self.guest.value(
                    f"{label}-{name}", f"sysctl -n {name}", require=False
                ).strip()
                for name in SYSCTLS[:2]
            },
        }

    def revert_policy(self, label):
        policy = guest_script("host-policy.sh")

        def revert(name):
            if self.state["policy_owned"][name]:
                self.guest.run(
                    f"{label}-{name}-revert", policy, root=True, args=(name + "-revert", self.user)
                )
                self.state["policy_owned"][name] = False
                self.save()

        cleanup_all(lambda: revert("user"), lambda: revert("distribution"))

    def snapshot_baseline(self):
        if self.state["baseline_profiles"] is None:
            self.state["baseline_profiles"] = profile_modes(
                self.guest.value("baseline-profiles", PROFILES, root=True)
            )
            self.state["baseline_policy_files"] = self.policy_state("baseline")["policy_files"]
            self.save()

    def unit(self, name, work):
        if name in self.state["units"]:
            return self.state["units"][name]
        emit(unit=name, status="started")
        started = time.monotonic()
        record = work()
        record["seconds"] = round(time.monotonic() - started, 1)
        record["attempt"] = self.attempt
        self.state["units"][name] = record
        self.save()
        emit(unit=name, seconds=record["seconds"])
        return record

    # Setup units.
    def provision(self):
        script = guest_script("provision.sh")
        before = profile_modes(self.guest.value("provision-profiles-before", PROFILES, root=True))
        base = self.guest.run(
            "provision-base", script, root=True, args=("base", self.user), timeout=1800
        )[0]
        build = self.guest.run(
            "provision-build", script, root=True, args=("build-root", self.user), timeout=1800
        )[0]
        after = profile_modes(self.guest.value("provision-profiles-after", PROFILES, root=True))
        return {
            "base": base["seconds"],
            "build_root": build["seconds"],
            "profiles_added_by_packages": sorted(set(after) - set(before)),
            "profiles_removed_by_packages": sorted(set(before) - set(after)),
            "packages": packages(
                self.guest.value("provision-packages", PACKAGE_QUERY, require=False)
            ),
        }

    def rust(self):
        script = guest_script("provision.sh")
        self.guest.run("rustup", script, args=("build-user", self.user), timeout=1800)
        version = self.guest.value(
            "rust-version", f'export PATH="{CARGO_PATH}"\ncd "$HOME" && rustc -Vv && cargo -V'
        )
        return {"toolchain": lines(version)}

    def sources(self):
        from functools import partial
        from e2e.scripts.local_ci import bundle_head
        from release.process import run

        root = product_root()
        commit = current().state["source_commit"]
        target = self.out / f"attempt-{self.attempt}"
        bundle = bundle_head(
            root, target, commit, partial(run, pass_fds=(current().admission.lock_fd,))
        )
        home = self.guest.value("guest-home", 'printf "%s" "$HOME"').strip()
        if not home.startswith("/") or ":" in home or "\n" in home:
            fail("Unexpected guest home")
        self.guest.run(
            "sources-dir", 'mkdir -p "$HOME/src" && rm -rf "$HOME/src/hive" "$HOME/src/head.bundle"'
        )
        host_run(
            ["limactl", "copy", str(bundle), f"{self.args.instance}:{home}/src/head.bundle"],
            timeout=900,
        )
        self.guest.run(
            "sources-clone",
            f"""set -euo pipefail
 git clone -q "$HOME/src/head.bundle" "$HOME/src/hive"
 git -C "$HOME/src/hive" checkout -q --detach {commit}
 test "$(git -C "$HOME/src/hive" rev-parse HEAD)" = {commit}
 test -z "$(git -C "$HOME/src/hive" status --porcelain)"
 rm "$HOME/src/head.bundle"
 """,
            timeout=600,
        )
        self.state["commit"] = commit
        return dict(commit=commit, bundle_bytes=bundle.stat().st_size, source="committed HEAD")

    def providers(self):
        arch = current().state["config"]["arch"]
        codex = "codex-" + arch + "-unknown-linux-musl.tar.gz"
        claude = "linux-arm64" if arch == "aarch64" else "linux-x64"
        done = self.guest.run(
            "providers", PROVIDERS, args=(codex, CLAUDE_BASE, claude), timeout=1800
        )[1]
        found = marked_json(done.stdout)
        version = version_tuple(found["claude_version"])
        if version is None or version < CLAUDE_FLOOR:
            fail("The native Claude distribution lacks the required helper")
        return found

    def build(self, part):
        script = (
            f'''set -euo pipefail
export PATH="{CARGO_PATH}" CARGO_TERM_COLOR=never
cd {SRC}
rm -rf dist/vm-frozen/{part}
mkdir -p dist/vm-frozen
python3 scripts/build_release.py --part {part} --output-dir dist/vm-frozen/{part}
set -- dist/vm-frozen/{part}/build.json
'''
            + BUILD_RECORD
        )
        return marked_json(self.guest.run(f"build-{part}", script, timeout=5400)[1].stdout)

    def package(self):
        floors = {
            name: record["required_glibc"]
            for part in PARTS
            for name, record in self.state["units"][f"build:{part}"]["binaries"].items()
        }
        script = f'''set -uo pipefail
export PATH="{CARGO_PATH}" CARGO_TERM_COLOR=never
cd {SRC}
rm -rf dist/vm-assets
for part in {" ".join(PARTS)}; do
  python3 scripts/package_release.py --build-dir "dist/vm-frozen/$part" --output-dir dist/vm-assets \\
    || {{ echo "PACKAGE_FAILED $part"; exit 10; }}
done
cd dist/vm-assets && sha256sum -c SHA256SUMS'''
        record, done = self.guest.run("package", script, require=False, timeout=1800)
        result = {
            "required_glibc": floors,
            "guest_package_returncode": done.returncode,
            "guest_package_failed": [
                line.split()[1] for line in lines(done.stdout) if line.startswith("PACKAGE_FAILED ")
            ],
        }
        if done.returncode == 0:
            result.update(assets="$HOME/src/hive/dist/vm-assets", source="guest build")
            return result
        above = sorted(name for name, floor in floors.items() if glibc_tuple(floor) > GLIBC_FLOOR)
        if not above or self.args.fallback_assets is None:
            fail("Packaging the guest build failed", log=record["log"], above_glibc_floor=above)
        result.update(above_glibc_floor=above, **self.fallback_assets())
        return result

    def fallback_assets(self):
        from e2e.vm.assets import accepted

        architecture = "arm64" if current().state["config"]["arch"] == "aarch64" else "amd64"
        binding = accepted(self.args.fallback_assets, architecture)
        built, head, root = binding["commit"], self.state["commit"], str(product_root())
        changed = []
        if built != head:
            host_run(["git", "-C", root, "merge-base", "--is-ancestor", built, head])
            changed = lines(host_run(["git", "-C", root, "diff", "--name-only", built, head]))
            if any(not CONTRACT_ONLY.search(name) for name in changed):
                fail("Accepted fallback assets differ from the candidate by code changes")
        home = self.guest.value("guest-home", 'printf "%s" "$HOME"').strip()
        if not home.startswith("/") or ":" in home or "\n" in home:
            fail("Unexpected guest home")
        self.guest.run("fallback-assets-dir", 'rm -rf "$HOME/assets-container"')
        host_run(
            [
                "limactl",
                "copy",
                "-r",
                binding["source"],
                f"{self.args.instance}:{home}/assets-container",
            ],
            timeout=900,
        )
        checksum = binding["checksums_sha256"]
        self.guest.run(
            "fallback-assets-verify",
            f"""set -euo pipefail
 cd "$HOME/assets-container"
 printf '%s  SHA256SUMS\n' {checksum} | sha256sum -c -
 sha256sum -c SHA256SUMS""",
        )
        return dict(
            assets="$HOME/assets-container",
            source="accepted native CI assets",
            acceptance=binding,
            changed_since_fallback=changed,
        )

    def test_build(self):
        script = f'''set -euo pipefail
cd {SRC}
env -i HOME="$HOME" PATH="{CARGO_PATH}" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" CARGO_TERM_COLOR=never \\
  cargo test --release --locked -p hive --test runtime --no-run'''
        record = self.guest.run("test-build", script, timeout=5400)[0]
        return {
            "command": "cargo test --release --locked -p hive --test runtime --no-run",
            "guest_seconds": record["seconds"],
        }

    def install(self):
        assets = self.state["units"]["package"]["assets"]
        flags = shlex.join(SETUP_FLAGS)
        script = f'''set -euo pipefail
export PATH="$HOME/providers/bin:$HOME/hive-prefix/bin:/usr/local/bin:/usr/bin:/bin"
sh "{assets}/install.sh" --from "{assets}" --prefix "$HOME/hive-prefix" --yes --no-setup'''
        self.guest.run("install", script, timeout=1800)
        setup_script = f'''set -euo pipefail
export PATH="$HOME/providers/bin:$HOME/hive-prefix/bin:/usr/local/bin:/usr/bin:/bin"
{HIVECTL} setup {flags} --preserve-stopped-services --yes'''
        record, done = self.guest.run("setup", setup_script, require=False, timeout=1800)
        setup = setup_result(done.returncode, parse_json(done.stdout, "setup"))
        setup["log"] = record["log"]
        return {
            "commands": [
                "sh ASSETS/install.sh --from ASSETS --prefix $HOME/hive-prefix --yes --no-setup",
                f"hivectl setup {flags} --preserve-stopped-services --yes",
            ],
            "assets": assets,
            "setup": setup,
            "service": self.service_state("install"),
        }

    # Per-configuration checks.
    def service_state(self, label):
        status = self.guest.run(
            f"{label}-service-status", f"{HIVECTL} service status", require=False
        )[1]
        units = self.guest.value(
            f"{label}-service-units",
            "systemctl --user list-units --all --type=service --no-legend --plain | grep -i hive || true",
        )
        return {
            "returncode": status.returncode,
            "status": status.stdout.strip()[-4000:],
            "units": lines(units),
        }

    def doctor(self, label, *, command=HIVECTL):
        done = self.guest.run(
            f"{label}-doctor", f'export PATH="$HOME/hive-prefix/bin:{CARGO_PATH}"\n{command} doctor',
            require=False, timeout=300,
        )[1]
        report = parse_json(done.stdout, f"{label}-doctor")
        return doctor_result(done.returncode, report, done.stderr)

    def acceptance(self, label, provider, context, path):
        if context == "vscode":
            profiles = profile_modes(
                self.guest.value(f"{label}-side-profiles", PROFILES, root=True)
            )
            if "vscode" not in profiles:
                return dict(passed=None, returncode=None, evidence=[], unavailable=True)
        variable, binary, test = ACCEPTANCE[provider]
        cargo = shlex.join(
            [
                "cargo",
                "test",
                "--release",
                "--locked",
                "-p",
                "hive",
                "--test",
                "runtime",
                test,
                "--",
                "--ignored",
                "--exact",
            ]
        )
        script = (
            f'cd {SRC}\nexec {CONTEXTS[context]} /usr/bin/env -i HOME="$HOME" PATH="{path}"'
            f' XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" RUST_TEST_THREADS=4 CARGO_TERM_COLOR=never'
            f' {variable}="{binary}" {cargo}'
        )
        done = self.guest.run(f"{label}-{provider}-{context}", script, require=False, timeout=1800)[
            1
        ]
        output = done.stdout + done.stderr
        passed = done.returncode == 0 and "test result: ok. 1 passed" in output
        failure = None
        if not passed:
            if (
                context == "vscode"
                and "test result:" not in output
                and any(
                    line.startswith("aa-exec:")
                    and ("Permission denied" in line or "insufficient permissions" in line)
                    for line in output.splitlines()
                )
            ):
                return dict(
                    passed=None,
                    returncode=done.returncode,
                    evidence=lines(output)[-8:],
                    unavailable=True,
                    reason="profile_transition_denied",
                )
            for line in output.splitlines():
                if '"outcome"' not in line or '"executable"' not in line or '"code"' not in line:
                    continue
                candidate = line[line.find("{") :].strip()
                row = parse_json(candidate, "native acceptance diagnostic")
                if set(row) != {"outcome", "executable", "code"}:
                    fail("Native acceptance diagnostic has an unknown shape")
                failure = row
            if (
                failure is None
                or failure["outcome"] != "failed"
                or failure["code"] != "sandbox_permission_denied"
            ):
                fail(
                    "Native acceptance failed without a verified sandbox denial",
                    provider=provider,
                    context=context,
                )
        evidence = [
            line.strip()[:600]
            for line in output.splitlines()
            if any(
                key in line
                for key in ("outcome", "panicked", "test result", "assertion", "Running as")
            )
        ]
        return dict(
            passed=passed, returncode=done.returncode, evidence=evidence[-8:], denial=failure
        )

    def helper(self, label, bwrap, command):
        argv = [bwrap, *HELPER, "$CLAUDE", *command]
        inner = shlex.join(argv).replace("'$CLAUDE'", '"$HOME/providers/bin/claude"')
        if bwrap == "SCOPED":
            inner = inner.replace("SCOPED", f'"{SCOPED_DIR}/bwrap"', 1)
        script = (
            f'home=$(mktemp -d)\ncd "$home"\nexec {CONTEXTS["service"]} /usr/bin/env -i HOME="$home"'
            f" PATH=/usr/bin:/bin {inner}"
        )
        done = self.guest.run(label, script, require=False)[1]
        controls = [
            line.split(" ", 1)[1]
            for line in done.stdout.splitlines()
            if line.startswith("HIVE_VM_CONTROLS ")
        ]
        return {
            "returncode": done.returncode,
            "controls": controls_result(done.returncode, controls, label),
            "marker": "HIVE_VM_MARKER" in done.stdout,
            "setgroups_denied": "setgroups" in done.stderr and "Permission denied" in done.stderr,
            "stderr": [
                line
                for line in lines(done.stderr)
                if not line.startswith(
                    (
                        "Running as",
                        "Finished",
                        "Main processes",
                        "Service runtime",
                        "CPU time",
                        "Memory",
                    )
                )
            ][-6:],
        }

    def config(self, config):
        label = config
        start = int(self.guest.value(f"{label}-clock", GUEST_CLOCK))
        prerequisites = self.prerequisites(config)
        path = f"{SCOPED_DIR}:{CARGO_PATH}" if config == "user" else CARGO_PATH
        result = dict(config=config, started_at=iso_now(), prerequisites=prerequisites)
        profiles = profile_modes(self.guest.value(f"{label}-profiles", PROFILES, root=True))
        result["profiles"] = dict(
            count=len(profiles),
            bwrap={
                name: profiles.get(name) for name in (*DISTRO_PROFILES, f"hive_{self.user}_bwrap")
            },
        )
        result["probes"] = {
            context: probe(self.guest, context, f"{label}-probe-{context}")
            for context in ("shell", "service")
        }
        if config == "stock" and self.state.get("initial_stock") is None:
            self.state["initial_stock"] = result["probes"]["service"]
        result["service"] = self.service_state(label)
        result["doctor"] = self.doctor(label)
        result["acceptance"] = {
            provider: {
                context: self.acceptance(label, provider, context, path) for context in CONTEXTS
            }
            for provider in ACCEPTANCE
        }
        result["helper_system_bwrap"] = self.helper(
            f"{label}-helper-system", "/usr/bin/bwrap", ["/bin/echo", "HIVE_VM_MARKER"]
        )
        if config == "user":
            result["helper_scoped"] = self.helper(
                f"{label}-helper-scoped", "SCOPED", ["/bin/echo", "HIVE_VM_MARKER"]
            )
            result["controls"] = self.helper(
                f"{label}-controls", "SCOPED", ["/usr/bin/python3", "-I", "-c", CONTROLS]
            )
        if config == "stock":
            result["release_install"] = self.release_install(label)
        end = int(self.guest.value(f"{label}-clock-end", GUEST_CLOCK))
        result["audit"] = audit_summary(self.guest, f"{label}-audit", start, end)
        result.update(finished_at=iso_now(), guest_seconds=round((end - start) / 1e6, 1))
        return result

    def prerequisites(self, config):
        if config not in CONFIGS:
            fail("Unknown guest policy")
        self.snapshot_baseline()
        self.guest.run(
            f"{config}-remove-service",
            f"{HIVECTL} service uninstall",
            timeout=300,
        )
        self.revert_policy(f"restore-{config}")
        policy = guest_script("host-policy.sh")
        stock_distribution = stock_policy(self.state) == "restricted_distribution"
        if stock_distribution and config == "user" and (
            any(
                name.startswith("hive_") and name.endswith("_bwrap")
                for name in self.state["baseline_profiles"]
            )
            or any(
                path != "/etc/apparmor.d/bwrap-userns-restrict"
                for path in self.state["baseline_policy_files"]
            )
        ):
            fail("Refusing to replace a pre-existing scoped Bubblewrap policy")
        if config in ("distribution", "user") and not stock_distribution:
            if (
                any("bwrap" in name for name in self.state["baseline_profiles"])
                or self.state["baseline_policy_files"]
            ):
                fail("Refusing to replace pre-existing stock Bubblewrap policy")
            self.state["policy_owned"]["distribution"] = True
            self.save()
            self.guest.run(
                f"{config}-restore-distribution",
                policy,
                root=True,
                args=("distribution", self.user),
            )
        if config == "user":
            self.state["policy_owned"]["user"] = True
            self.save()
            self.guest.run(
                "user-restore-scoped", policy, root=True,
                args=("user-stock" if stock_distribution else "user", self.user),
            )
        base = "$HOME/hive-prefix/bin:$HOME/providers/bin:/usr/local/bin:/usr/bin:/bin"
        service_path = f"{SCOPED_DIR}:" + base if config == "user" else base
        self.guest.run(
            f"{config}-restore-service",
            f"""set -euo pipefail
 {HIVECTL} service install --service-path "{service_path}"
 {HIVECTL} service start""",
            timeout=300,
        )
        record = dict(
            config=config,
            service_path=service_path,
            attempt=self.attempt,
            service=self.service_state(f"{config}-restored"),
        )
        self.state["prerequisites"].append(record)
        self.save()
        return record

    def release_install(self, label):
        from e2e.vm import checks_account

        output = f"dist/vm-installed-checks-{self.attempt}-{int(time.time())}"
        script = f'''set -euo pipefail
export PATH="{CARGO_PATH}"
cd {SRC}
python3 scripts/check_release_install.py --assets "$HOME/assets" --output-dir {output}
python3 -I -c 'import json, sys; r = json.load(open(sys.argv[1])); print("HIVE_VM_JSON " + json.dumps({{k: {{"seconds": v.get("seconds")}} for k, v in r["checks"].items()}}))' {output}/results.json'''
        record, done, context = checks_account.run(
            self, f"{label}-release-install", script, timeout=3600
        )
        return {
            "passed": done.returncode == 0,
            "returncode": done.returncode,
            "checks": marked_json(done.stdout) if done.returncode == 0 else None,
            "log": record["log"],
            "execution_context": context,
        }

    def finish(self):
        self.prerequisites("stock")
        service = self.guest.run(
            "finish-service",
            f"{HIVECTL} service stop && {HIVECTL} service uninstall",
            require=False,
            timeout=300,
        )[0]
        self.revert_policy("finish")
        profiles = profile_modes(self.guest.value("finish-profiles", PROFILES, root=True))
        leftovers = sorted(
            set(self.policy_state("finish")["policy_files"])
            - set(self.state["baseline_policy_files"])
        )
        again = probe(self.guest, "service", "finish-probe-service")
        baseline = self.state["baseline_profiles"]
        return dict(
            service_uninstall_returncode=service["returncode"],
            profiles_restored=profiles == baseline,
            profile_count=len(profiles),
            leftover_policy_files=leftovers,
            restored_probe_class=probe_class(again),
            stock_probe_class=probe_class(self.state["initial_stock"]),
            probe_bwrap=again["bwrap_lines"],
            service=self.service_state("finish"),
            guest_disk=lines(
                self.guest.value("finish-guest-disk", "df -B1 --output=size,used,avail /")
            ),
        )

    def suite_comparison(self):
        from e2e.vm import checks_account

        output = f"dist/vm-suite-{self.attempt}"
        platform = (
            "linux-arm64" if current().state["config"]["arch"] == "aarch64" else "linux-amd64"
        )
        fallback = self.state.get("units", {}).get("package", {}).get("source") == (
            "accepted native CI assets"
        )
        asset_option = ", installed_assets=Path.home() / 'assets'" if fallback else ""
        script = f"""set -euo pipefail
 export PATH="{CARGO_PATH}" RUST_TEST_THREADS=4 CARGO_BUILD_JOBS=4
 {SUITE_TMPDIR}
 cd {SRC}
 python3 - <<'EOF'
from pathlib import Path
import json
from e2e.ci.runner import Runner
from release.progress import entry
def main():
    result=Runner({platform!r}, Path.cwd(), Path({output!r}), full=True{asset_option}).run()
    print('HIVE_VM_JSON '+json.dumps(result),flush=True)
    if not result['passed']: raise SystemExit(1)
raise SystemExit(entry(main,name='verify'))
EOF
 python3 e2e/run.py unit
 """
        record, done, context = checks_account.run(
            self, "suite-comparison", script, timeout=4 * 3600, rust=True
        )
        found = [
            line[len(JSON_MARK) :]
            for line in done.stdout.splitlines()
            if line.startswith(JSON_MARK)
        ]
        result, exported = None, False

        def export():
            nonlocal exported
            exported = checks_account.collect(
                self, context, output,
                verify_assets=done.returncode == 0 and result is not None and result["passed"],
            )
            if not exported:
                return
            home = context["origin_home"]
            host_run(
                [
                    "limactl", "copy", "-r",
                    f"{self.args.instance}:{home}/src/hive/{output}",
                    str(self.out / f"guest-suite-{self.attempt}"),
                ],
                timeout=900,
            )
        try:
            with finalizer(export):
                checked(done)
                result = parse_json(found[0], "suite comparison") if len(found) == 1 else None
                if result is not None:
                    result["execution_context"] = context
        except ProcessFailure as failure:
            if failure.result is not done or getattr(failure, "cleanup_errors", ()):
                raise
        return dict(
            passed=exported and done.returncode == 0 and result is not None and result["passed"],
            returncode=done.returncode,
            workflow=result,
            log=record["log"],
            retained_logs=f"guest-suite-{self.attempt}" if exported else None,
            execution_context=context,
        )

    def embedding_smoke(self):
        from e2e.vm.context import free_bytes

        if current().state["memory_gib"] < 20:
            fail("CPU embedding smoke needs a fresh VM with --memory-gib 20")
        if free_bytes() < MIN_FREE + 8 * GIB:
            fail("The host needs model-growth headroom in addition to its 10 GiB floor")
        script = f"""set -euo pipefail
 export PATH="{CARGO_PATH}"
 cd {SRC}
 python3 - <<'EOF'
import shutil
if shutil.disk_usage('.').free < 10*1024**3:
    raise RuntimeError('Guest needs model and build headroom')
EOF
 python3 e2e/run.py prepare --tier embeddings --install
 python3 -m e2e.vm.embedding --output-dir dist/vm-embedding-{self.attempt}
 """
        record, done = self.guest.run("embedding-smoke", script, require=False, timeout=2 * 3600)
        found = [
            line[len(JSON_MARK) :]
            for line in done.stdout.splitlines()
            if line.startswith(JSON_MARK)
        ]
        result = parse_json(found[0], "embedding smoke") if len(found) == 1 else None
        return dict(
            passed=done.returncode == 0 and result is not None,
            returncode=done.returncode,
            result=result,
            log=record["log"],
        )


def audit_summary(guest, name, start_us, end_us):
    counts = {}
    for row in audit_lines(guest, name, start_us, end_us)["lines"]:
        fields = {
            key: quoted or bare
            for key, quoted, bare in re.findall(r'(\w+)=(?:"([^"]*)"|(\S+))', row["message"])
        }
        key = tuple(
            (field, fields[field])
            for field in (
                "apparmor",
                "operation",
                "profile",
                "comm",
                "capname",
                "target",
                "requested",
            )
            if field in fields
        )
        counts[key] = counts.get(key, 0) + 1
    return [{**dict(key), "count": count} for key, count in sorted(counts.items())]


def acceptance_outcome(row):
    return "pass" if row["passed"] else "fail"


def doctor_result(returncode, report, stderr):
    rows = report.get("checks", [])
    checks = {item["name"]: item for item in rows}
    if len(checks) != len(rows):
        fail("Doctor returned duplicate checks")
    errors = [name for name, item in checks.items() if item["status"] == "error"]
    matches = (returncode == 0) == (not errors)
    names = ("native_sandbox:daemon", "claude_sandbox:daemon")
    if (
        not matches
        or any(name not in checks for name in names)
        or any(item.get("status") not in ("ok", "warning", "error", "info") for item in checks.values())
    ):
        fail("Doctor output/exit does not describe both daemon sandbox checks")
    for name in names:
        diagnostic_status(checks[name])
    return dict(
        returncode=returncode,
        exit_matches_errors=matches,
        errors=errors,
        warnings=[name for name, item in checks.items() if item["status"] == "warning"],
        sandbox={
            name: dict(status=checks[name]["status"], detail=checks[name]["detail"])
            for name in (*names, "native_sandbox:caller_local", "claude_sandbox:caller_local")
            if name in checks
        },
        stderr_tail=lines(stderr)[-12:],
    )


def diagnostic_status(row):
    detail = row.get("detail")
    diagnostic = detail.get("diagnostic") if isinstance(detail, dict) else None
    if (
        not isinstance(detail, dict)
        or set(detail) != {"context", "diagnostic"}
        or detail["context"] != "daemon"
        or not isinstance(diagnostic, dict)
        or set(diagnostic) != {"outcome", "executable", "code"}
        or not isinstance(diagnostic["executable"], str)
        or not diagnostic["executable"].startswith("/")
    ):
        fail("Doctor did not verify sandbox execution in the daemon context")
    observed = (row.get("status"), diagnostic["outcome"], diagnostic["code"])
    if observed not in (
        ("ok", "passed", "native_command_completed"),
        ("error", "failed", "sandbox_permission_denied"),
    ):
        fail("Doctor sandbox diagnostic is an infrastructure failure or is unverified")
    return observed[0] == "ok"


def setup_result(returncode, report):
    """Classify setup without hiding the sandbox-denied outcome."""
    if not isinstance(report, dict) or report.get("configured") is not True:
        fail("Setup did not configure the installed service")
    doctor = report.get("doctor")
    rows = doctor.get("checks") if isinstance(doctor, dict) else None
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        fail("Setup did not return doctor checks")
    names = [row.get("name") for row in rows]
    if any(not isinstance(name, str) for name in names) or len(set(names)) != len(names):
        fail("Setup returned invalid or duplicate doctor checks")
    checks = dict(zip(names, rows, strict=True))
    if any(row.get("status") not in ("ok", "warning", "error", "info") for row in rows):
        fail("Setup returned an invalid doctor status")
    service = checks.get("services", {})
    if service.get("status") != "ok" or not isinstance(service.get("detail"), dict) or service["detail"].get("ready") is not True:
        fail("Setup did not start the service")
    sandbox_names = ("native_sandbox:daemon", "claude_sandbox:daemon")
    if any(name not in checks for name in sandbox_names):
        fail("Setup omitted a daemon sandbox check")
    sandbox = [diagnostic_status(checks[name]) for name in sandbox_names]
    allowed = set(sandbox_names)
    for name in ("auth:codex-main", "auth:claude-main"):
        item = checks.get(name, {})
        if item.get("status") == "error" and isinstance(item.get("detail"), dict) and item["detail"].get("state") == "missing":
            allowed.add(name)
    errors = [name for name in names if checks[name]["status"] == "error"]
    if any(name not in allowed for name in errors):
        fail("Setup reported an unexpected error")
    if returncode == 0 and all(sandbox):
        outcome = "success"
    elif returncode == 1 and not all(sandbox):
        outcome = "sandbox_denied"
    else:
        fail("Setup outcome does not match its daemon sandbox checks")
    return dict(
        returncode=returncode, outcome=outcome, errors=errors,
        sandbox=dict(zip(sandbox_names, sandbox, strict=True)),
    )


def stock_policy(state):
    """Classify the recorded stock policy without adopting foreign profiles."""
    host = state.get("units", {}).get("host-truth")
    if not isinstance(host, dict):
        return None
    preflight = host.get("preflight", {})
    restriction = preflight.get("sysctls", {}).get("kernel.apparmor_restrict_unprivileged_userns")
    profiles = preflight.get("profiles", {})
    modes = [profiles.get(name) for name in DISTRO_PROFILES]
    if restriction in (None, "0") and modes == [None, None]:
        return "unrestricted"
    if restriction == "1" and modes == [None, None]:
        return "restricted"
    unowned = host.get("lima_additions", {}).get("unowned_apparmor_files")
    if restriction == "1" and modes == ["enforce", "enforce"] and unowned == []:
        return "restricted_distribution"
    return "unknown"


def controls_result(returncode, records, label):
    if returncode != 0 or len(records) != 1:
        return None
    return control_values(returncode, parse_json(records[0], label))


def control_values(returncode, value):
    if (
        returncode != 0
        or not isinstance(value, dict)
        or set(value) != {"write", "unix_socket", "userns"}
        or any(type(item) is not int for item in value.values())
    ):
        return None
    return value


def probe_class(row):
    return (
        "fail_rtm_newaddr"
        if reproduces(row)
        else "pass"
        if row["returncode"] == 0
        else "fail_other"
    )


def evaluate(state):
    rows = []
    policy = stock_policy(state)
    noble = state["ubuntu"] == "24.04" or (
        state["ubuntu"] == "26.04" and policy == "restricted"
    )

    def record(config, name, expected, observed, evidence, *, side=False):
        rows.append(
            dict(
                config=config,
                check=name,
                expected=expected,
                observed=observed,
                match=None if expected is None else expected == observed,
                side_check=side,
                evidence=evidence,
            )
        )

    for config in CONFIGS:
        result = state["configs"].get(config)
        if result is None:
            continue
        expected = NOBLE_EXPECTED[config] if noble else (
            {key: value for key, value in NOBLE_EXPECTED["user"].items()
             if key != "helper_system_bwrap"} if config == "user" else {}
        )
        for context, row in result["probes"].items():
            record(config, f"probe:{context}", expected.get("probe"), probe_class(row), row)
            record(config, f"probe_valid:{context}", True, probe_class(row) != "fail_other", row)
        for name in ("native_sandbox:daemon", "claude_sandbox:daemon"):
            item = result["doctor"]["sandbox"][name]
            record(config, f"doctor:{name}", expected.get(name), item["status"], item)
        for provider in ACCEPTANCE:
            for context, row in result["acceptance"][provider].items():
                side = context == "vscode"
                observed = "unavailable" if row.get("unavailable") else acceptance_outcome(row)
                wanted = None if side else expected.get(provider)
                record(config, f"acceptance:{provider}:{context}", wanted, observed, row, side=side)
            native = result["acceptance"][provider]["service"]
            key = "native_sandbox:daemon" if provider == "codex" else "claude_sandbox:daemon"
            correlated = diagnostic_status(result["doctor"]["sandbox"][key]) == native["passed"]
            record(
                config,
                f"doctor_acceptance:{provider}",
                True,
                correlated,
                dict(doctor=result["doctor"]["sandbox"][key], acceptance=native),
            )
            if config == "stock":
                shell = result["acceptance"][provider]["shell"]
                record(config, f"doctor_acceptance:{provider}:shell", True,
                       diagnostic_status(result["doctor"]["sandbox"][key]) == shell["passed"],
                       dict(doctor=result["doctor"]["sandbox"][key], acceptance=shell))
            if config == "stock" and "helper_system_bwrap" in result:
                host_passed = (probe_class(result["probes"]["service"]) == "pass"
                               if provider == "codex" else result["helper_system_bwrap"]["marker"])
                record(config, f"host_acceptance:{provider}", host_passed,
                       diagnostic_status(result["doctor"]["sandbox"][key]),
                       dict(host_passed=host_passed, doctor=result["doctor"]["sandbox"][key]))
        if "helper_system_bwrap" in expected:
            item = result["helper_system_bwrap"]
            record(
                config,
                "helper:/usr/bin/bwrap",
                expected["helper_system_bwrap"],
                "setgroups_denied" if item["setgroups_denied"] else "other",
                item,
            )
        if "controls" in expected:
            item = result["controls"]
            record(
                config,
                "helper:controls",
                expected["controls"],
                control_values(item["returncode"], item["controls"]),
                item,
            )
        if "release_install" in result:
            item = result["release_install"]
            record(config, "check_release_install", True, item["passed"], item)
    stock = state["configs"].get("stock")
    if stock is not None:
        if policy is not None:
            record("stock", "known policy", True, policy != "unknown", dict(policy=policy))
            if state["ubuntu"] in ("22.04", "24.04"):
                record("stock", "policy class", "unrestricted" if state["ubuntu"] == "22.04" else "restricted",
                       policy, dict(policy=policy))
        statuses = {name: diagnostic_status(stock["doctor"]["sandbox"][name]) for name in
                    ("native_sandbox:daemon", "claude_sandbox:daemon")}
        if state["ubuntu"] == "22.04":
            record("stock", "provider sandboxes", True, all(statuses.values()), statuses)
        if "install" in state["units"]:
            setup = state["units"]["install"]["setup"]
            record("install", "setup sandbox agreement", statuses, setup["sandbox"], setup)
        if state.get("finish") is not None and not all(statuses.values()):
            record("user", "remedy required", True, "user" in state["configs"], statuses)
            if policy == "restricted_distribution":
                record("distribution", "stock profile retained", True,
                       state["units"].get("policy-distribution", {}).get("reason") == "provided by stock"
                       and not state["policy_owned"]["distribution"], dict(policy=policy))
        if policy == "restricted_distribution" and "user" in state["configs"]:
            before = stock["helper_system_bwrap"]
            after = state["configs"]["user"]["helper_system_bwrap"]
            record("user", "system helper unchanged", (before["marker"], before["setgroups_denied"]),
                   (after["marker"], after["setgroups_denied"]), dict(before=before, after=after))
        if "user" in state["configs"]:
            helper = state["configs"]["user"].get("helper_scoped", {})
            record("user", "scoped helper", True, helper.get("marker") is True, helper)
    for name in ("suite-comparison", "embedding-smoke"):
        if name in state["units"]:
            record(name, "passed", True, state["units"][name]["passed"], state["units"][name])
    finish = state.get("finish")
    if finish is not None:
        restored = (
            finish["profiles_restored"]
            and not finish["leftover_policy_files"]
            and finish["restored_probe_class"] == finish["stock_probe_class"]
            and finish["service_uninstall_returncode"] == 0
        )
        record("finish", "stock policy restored", True, restored, finish)
    return rows
