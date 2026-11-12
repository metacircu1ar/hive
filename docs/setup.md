# Set up Hive and Comb

Hive runs Codex and Claude agents using isolated subscription logins. Comb is
an optional local knowledge service. Setup connects the installed parts, signs
in, saves provider defaults and installs user login services. It does not require
Python. The [platform guide](platforms-and-installation.md) lists supported builds
and source-build requirements.

## Quick install

Download `install.sh` from the latest release with `curl`; the installer fetches
the selected parts from the same release. `HIVE_RELEASE` in the commands below
is that release URL; it can also be a local directory holding a downloaded
release:

```sh
HIVE_RELEASE=https://github.com/metacircu1ar/hive/releases/download/v0.1.0
curl -fsSLO "$HIVE_RELEASE/install.sh"
# Hive; the wizard asks for provider, login and model choices.
sh install.sh --from "$HIVE_RELEASE"
# Hive with Comb and its bundled llama-server for local semantic search.
sh install.sh --from "$HIVE_RELEASE" --comb
# Hive without any Comb integration.
sh install.sh --from "$HIVE_RELEASE" --core
# Standalone Comb, without installing Hive.
sh install.sh --from "$HIVE_RELEASE" --standalone-comb
```

The default prefix is `~/.local`; add its `bin` to your shell's `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
hivectl doctor
hivectl spawn --cwd . --body 'Describe this repository. Do not change files.'
```

Use `--prefix /absolute/directory` to install elsewhere. An HTTPS asset base URL
can replace the local `--from` directory. The script verifies checksums, and the
native helper validates every selected archive before publishing it. Unrelated
commands and edited owned files are refused.

Before switching versions, the installer asks setup's questions and checks the
complete setup plan, including existing service settings and user skill files.
A planning refusal leaves the installed version and user data unchanged. After
activation, setup rechecks the saved choices and applies them without asking those
questions again. Native sign-in may still require interaction, and changed state
or an external operation can still cause setup to fail after installation.
Suspending a setup child with Ctrl-Z cancels that step and releases the terminal.

## What setup does

1. Detects configuration, provider CLIs, existing login profiles and installed parts.
2. Checks native provider versions. It installs missing provider CLIs only with
   explicit consent or `--install-provider codex,claude` (npm must be available).
3. Verifies or signs in to isolated Hive profiles. Existing verified profiles are
   reused. Native login requires a controlling terminal; `--yes` does not make
   browser/device authentication unattended.
4. Discovers models and efforts and saves provider defaults. New agents can then
   omit provider/auth/model/effort flags. Explicit spawn flags still override.
5. When available, configures Comb, a private bridge credential and ordinary
   `curator`/`comb-reviewer` roles using bundled skills. Collectors are opt-in.
6. Installs and starts per-user login services, unless `--no-services` is chosen.
7. Runs `hivectl doctor` and returns JSON with `configured`, `ready`, `changed`,
   `pending` actions and a first-spawn command. Progress goes to stderr.

Re-running an identical setup preserves the configuration and model selection.
Setup adds missing managed keys; conflicting existing values stop it. It preserves
unrelated YAML and comments and backs up a real configuration edit before atomic
publication. It follows an existing config symlink to its checked target without
replacing the symlink. A running daemon keeps its loaded configuration until an
explicit restart; setup reports `restart_pending` even on subsequent unchanged runs.

| File or directory | Purpose |
| --- | --- |
| `$XDG_CONFIG_HOME/hive/config.yaml` (default `~/.config/hive/config.yaml`) | Provider profiles/defaults, optional roles and Comb binding; override with global `--config`. |
| `$XDG_STATE_HOME/hive` (default `~/.local/state/hive`) | Isolated authentication, native conversations, recovery evidence and runtime; override with global `--state-dir`. |
| `~/Comb` | Visible knowledge pages plus private `.comb` state; override with `--comb-root`. Avoid cloud-synced folders. |
| `$XDG_CONFIG_HOME/hive/comb-bridge.json` (default `~/.config/hive/comb-bridge.json`) | Default mode-0600 Hive bridge credential; use `--bridge-file` to select another path. |
| `ROOT/.comb/serve.json` | Managed Comb embedding and socket selection. |
| `$XDG_CONFIG_HOME/hive/services` | Owned service descriptors and stable scope locks. |
| `$XDG_STATE_HOME/hive-services/logs/<label>.log` | Per-scope stdout/stderr, separate from product data. `service status` identifies the exact path. |
| `~/Library/LaunchAgents/<label>.plist` or `$XDG_CONFIG_HOME/systemd/user/<label>.service` | macOS launchd or Linux systemd user unit. |
| `PREFIX/share/{hive,comb}` | Packaged skills, documentation, build records and notices. |

XDG defaults are `~/.config`, `~/.local/state` and `~/.local/share`. Keep the same
configuration and state paths for setup, authentication, daemon and client commands.
Comb's `--root` explicitly selects a knowledge root; setup defaults to `~/Comb`.

If an earlier development build installed services with logs under `hive/logs`, use that
installed build's `hivectl service uninstall` and, if installed, `comb service uninstall`
before replacing the binaries. Use the same configuration, state and knowledge
paths as the installation. Then install the services with the new build. The new
build refuses old-layout descriptors rather than adopting them. Uninstall retains
your data and logs; unsupported development state still requires a fresh state
directory.

## Scripted setup

`--yes` disables wizard prompts, not native sign-in. Omitted required answers
produce a field-specific input error (exit 2) naming the corresponding flag.
The same applies when no controlling terminal is available. For a fresh profile without a
terminal, choose `--login skip`, provide model/effort explicitly, then sign in later:

```sh
hivectl setup --yes --provider codex --login skip \
  --codex-model gpt-6-luna --codex-effort low --no-comb --no-services
hivectl auth login codex-main
hivectl service install
hivectl service start
hivectl doctor
```

Model names are examples; use `hivectl models --auth codex-main` for your account.
A previously verified profile can be reused headlessly with `--login browser`.
For both providers, also set `--default-provider`:

```yaml
# answers.yaml: strict YAML, kebab-case keys, no unknown keys.
provider: [codex, claude]
default-provider: codex
login: skip
codex-model: gpt-6-luna
codex-effort: low
claude-model: sonnet
claude-effort: low
no-comb: true
no-services: true
```

```sh
hivectl setup --answers answers.yaml --yes --dry-run
hivectl setup --answers answers.yaml --yes
```

Flags override answers-file values. Hive setup's boolean flags accept an explicit
`=false`, for example `--collectors=false` or `--no-services=false`. Dry run does
not sign in, download, initialize roots, publish files or change services; it
identifies native discovery deferred until apply.

Installer flags precede `--`; setup flags follow it. Alternatively prefix a setup
flag with `--setup-`, for example `--setup-provider codex`:

```sh
# HIVE_RELEASE is the release URL from Quick install.
sh install.sh --from "$HIVE_RELEASE" --comb --yes -- \
  --provider codex --login skip --codex-model gpt-6-luna --codex-effort low \
  --no-services
```

### Hive setup flags

Boolean setup switches accept `true` or `false`, for example `--services=false`;
spelling the switch without a value means `true`.

Global Hive flags such as `--config FILE`, `--state-dir DIR`, `--codex-bin PATH`,
`--claude-bin PATH` and `--probe-timeout SECONDS` are described in the
[CLI reference](cli-reference.md#global-options-and-output). They can also be forwarded by
the installer. `--help` displays the command's options.

| Flags | Meaning and example |
| --- | --- |
| `--answers FILE` | Strict answers file, as above. |
| `--provider codex,claude` | Providers to configure; one or both. |
| `--default-provider codex` | Default provider for spawns without a role/provider. |
| `--login browser\|device\|skip` | Native login flow; device is Codex-only. Skip leaves authentication pending when unverified. |
| `--codex-profile NAME`, `--claude-profile NAME` | Profile names; new defaults are `codex-main` and `claude-main`. |
| `--codex-model MODEL`, `--claude-model MODEL` | Provider model defaults, validated through discovery when authenticated. Required with skip if not already configured. |
| `--codex-effort E`, `--claude-effort E` | Corresponding effort defaults, for example `low`. |
| `--install-provider codex,claude` | Consent to install missing provider CLIs; does not silently upgrade existing CLIs. |
| `--comb-root DIR` | Knowledge root. |
| `--embedding-config FILE` | HTTP embedding YAML in a readable regular file; omit for text search. Symlinks to regular files are supported. |
| `--bridge-file FILE` | Exact bridge credential; existing broken or foreign credentials are refused. |
| `--comb-bin PATH` | Explicit Comb executable. |
| `--no-comb` | Skip Comb configuration; core builds require this route without Comb options. |
| `--no-comb-roles` | Do not add the curator/reviewer roles. |
| `--collectors` | Enable shared learning evidence collection. Off by default. |
| `--user-skills codex,claude` | Copy the bundled Hive guide and linked docs into user skill folders; refuse user-edited copies. |
| `--skills-dir DIR` | Override the bundled skills directory. |
| `--services`, `--no-services` | Install/start login services (default), or provision only. |
| `--restart-daemon` | Explicitly permit restarting the owned daemon to load the new configuration. Can interrupt work. Manual daemons require manual restart. |
| `--service-environment FILE` | Private mode-0600 `KEY=literal-value` file for service credentials/proxies; no shell evaluation. |
| `--service-path PATH` | Service executable search path; defaults to valid absolute directories in the current shell's PATH. |
| `--yes`, `--dry-run` | Suppress prompts or inspect without applying changes. |
| `--uninstall`, `--prefix DIR` | Remove installed Hive commands/services, normally inferring the prefix from the executable. |
| `--purge`, `--purge-comb-root DIR` | Explicit removal controls; see below. Whole-root deletion requires selecting Comb in the native installer. |

With `hivectl setup --uninstall`, the accepted controls are `--answers`, `--yes`,
`--dry-run`, `--prefix` and `--purge`. Explicit `--config`, `--state-dir`,
`--comb-root`, `--bridge-file` and `--purge-comb-root` require
`--purge`; implicit config/state defaults remain allowed. Other setup options
are rejected before invoking the installer, including explicit false values and
values in the answers file. Provider executable and timeout overrides are also
rejected. Use a removal-specific answers file instead of reusing wizard answers.

Answers must be an owned regular UTF-8 YAML file of at most 1 MiB, not writable by
other users. Symlinks, pipes, duplicate keys, anchors, aliases and tags are rejected.
Errors identify `--answers` and the affected key when known; flags override the
same keys. An explicit `--service-path` must contain only absolute existing
directories, separated by colons, without empty entries or control characters.
Inherited shell PATH entries are still filtered when no service PATH is supplied.

```sh
hivectl setup --uninstall --yes
hivectl --config /absolute/config.yaml --state-dir /absolute/private/state \
  setup --uninstall --purge --yes
hivectl setup --uninstall --prefix /absolute/installation --dry-run
```

Hive reads `--embedding-config` in separate Comb preview and apply processes.
Keep that regular file unchanged during setup. A single-consumer configuration
pipe is supported only by direct `comb setup`, which retains its parsed input.

### Comb setup and embedding flags

Use global `--root DIR` and optional `--socket PATH` before the Comb
subcommand. Standalone setup has no provider login:

```sh
comb --root "$HOME/Comb" setup --yes
comb --root "$HOME/Comb" setup --for-hive --reviewer-role comb-reviewer
```

| Comb setup flags | Meaning |
| --- | --- |
| `--embedding-config FILE` | Select a validated HTTP embedding configuration. Omission preserves an existing selection; a new root uses text search. |
| `--local-embeddings` | Select local embeddings: download the pinned 8 GB Qwen model and run the bundled llama-server, as `model use qwen3-local` does. Interactive setup of a fresh root asks instead; the default stays text search. |
| `--llama-server PATH`, `--llama-server-arg ARG` | With `--local-embeddings`, run your own llama-server and append arguments to it (repeat `--llama-server-arg`). |
| `--for-hive`, `--bridge-file FILE` | Create/reuse the exact setup bridge. |
| `--reviewer-role ROLE` | Repeat for roles allowed to review knowledge; requires `--for-hive`. |
| `--services`, `--no-services` | Install/start user service by default, or leave manual control. |
| `--service-environment FILE`, `--service-path PATH` | Service environment and toolchain path. |
| `--yes`, `--dry-run`, `--help` | Noninteractive apply, read-only preview, help. |

A root created by `comb init` has no saved selection. Setup refuses it until you
choose: `comb --root ROOT model use none` for text search, `--local-embeddings`,
or `--embedding-config FILE`. Change the selection later with the model commands:

```sh
comb --root "$HOME/Comb" model use qwen3-local
comb --root "$HOME/Comb" model use qwen3-local --llama-server /opt/llama.cpp/bin/llama-server \
  --llama-server-arg=--n-gpu-layers --llama-server-arg=99
comb --root "$HOME/Comb" model use-http --url http://127.0.0.1:8081/v1/embeddings \
  --model embedding-model --dimensions 1024
comb --root "$HOME/Comb" model use none
```

| Model command flags | Meaning |
| --- | --- |
| `use none` | Serve text search only. |
| `use qwen3-local` | Local embeddings: checks the bundled server and the memory floor (16 GiB physical on macOS; 16 GiB available after use and cgroup limits on Linux), downloads and verifies the pinned 8 GB model, then probes and selects the server. A 16 GiB Linux host is insufficient; allocate 20 GiB or more to a VM and leave 16 GiB available. |
| `--llama-server PATH` | With `qwen3-local`, run your own llama-server binary; it chooses its own device. A version other than the bundled one is only a warning. |
| `--llama-server-arg ARG` | One extra argument for that binary, after Comb's own; repeat it once per argument, as in the example above. |
| `use-http --url URL --model MODEL --dimensions N` | Required HTTP endpoint identity and vector dimensions (1–4096); model identifier is 1–4096 UTF-8 bytes. |
| `--revision REV` | HTTP operator-managed revision; pin it and change it when weights change. Default `operator-managed`; 1–4096 UTF-8 bytes. |
| `--query-prefix TEXT`, `--document-prefix TEXT` | HTTP prompts, empty by default, at most 1024 UTF-8 bytes each. |
| `--api-key-env NAME` | Read an HTTP bearer secret from this environment variable, never a CLI secret. |
| `--timeout-seconds N` | HTTP request timeout, 1–120 seconds, default 60. |
| `--request-dimensions` | Send the configured dimensions to endpoints that support this parameter. |
| `--tokenizer FILE`, `--max-tokens N` | Optional exact tokenizer and limit (16–8192) for local HTTP request chunking; supply both together. |
| `--max-request-bytes N` | Optional bound on total UTF-8 input bytes per HTTP request, including prefixes (64–65536). Must fit the document prefix plus the minimal `test` probe. Documents are split to fit; oversized queries are rejected. Each bounded group has its own request timeout. |
| `--help` | Help for the model subcommand or any nested command. |

## Parts and HTTP embeddings

Core Hive orchestrates agents without Comb tools. Default Hive can connect to
Comb when configured. Standalone Comb works with other MCP clients too; see
[Comb's client guide](../comb/README.md).

Text search works immediately. Setup never waits for semantic backfill; doctor
reports coverage and progress. Embedding models run in a separate HTTP server.

Local embeddings need no separate installation. The installer adds Comb's
bundled `comb-llama-server` with Comb, and `comb model use qwen3-local` downloads
the pinned Qwen3-Embedding-8B model, then lets Comb's service start and stop the
server: Metal on macOS, the CPU on Linux. Your own llama-server binary, such as a
CUDA or Vulkan build, works the same way with `--llama-server PATH`; see
[local embeddings](../comb/README.md#local-embeddings). Setup never replaces an
existing selection unless you choose another one. If the server stops, Comb
keeps serving text search and reports `embeddings_unavailable`; it never restarts
the server itself. Restart Comb's service, or rerun `comb model use qwen3-local`,
to start it again.

For a model server you run yourself, save its configuration. For Qwen on a
server you started, `qwen.yaml` is:

```yaml
backend: http
url: http://127.0.0.1:8081/v1/embeddings
model: qwen3-embedding-8b
revision: q8_0-REPLACE_WITH_GGUF_SHA256
dimensions: 4096
query_prefix: "Instruct: Given a question about our project, retrieve the knowledge pages that answer it\nQuery:"
document_prefix: ""
timeout_seconds: 60
```

```sh
hivectl setup --embedding-config qwen.yaml
# Standalone alternative:
comb --root "$HOME/Comb" setup --embedding-config qwen.yaml
# Or select an endpoint directly:
comb --root "$HOME/Comb" model use-http \
  --url http://127.0.0.1:8081/v1/embeddings \
  --model qwen3-embedding-8b --dimensions 4096 --revision q8_0-FILE_SHA256 \
  --query-prefix "$(printf 'Instruct: Retrieve relevant project knowledge\nQuery:')"
```

Plain HTTP is allowed only on loopback; external endpoints require HTTPS. For a
secret endpoint, add `api_key_env: EMBEDDING_TOKEN` and supply a private environment
file with `--service-environment`. Values are literal, including `$`, quotes and
backslashes. Keep a server you run yourself running; Comb starts only the
llama-server of a local selection.

## Services

Services are per user and per configuration/state pair for Hive,
and per knowledge root for Comb. They start at user login, not as system-wide daemons.
They capture the setup shell's usable PATH; they do not source your shell startup
files. Inspect the reported executable, label, environment and log path:

```sh
hivectl service status
hivectl service stop
hivectl service start
comb --root "$HOME/Comb" service status
comb --root "$HOME/Comb" service stop
comb --root "$HOME/Comb" service start
# Remove only the service registration, retaining binaries and data:
hivectl service uninstall
comb --root "$HOME/Comb" service uninstall
```

Manual service installation accepts `--environment-file FILE`, `--program PATH`
and `--service-path PATH`. Use stable installed `PREFIX/bin` paths, never immutable
generation paths. Changed service settings require uninstall/reinstall. An old
owned unit may be refreshed while stopped. Edited/unowned units are refused.

Comb service logging accepts `comb service install --log-level
off|error|warn|info|debug|trace` (default `info`),
`comb service install --log-file-mib N` (1–1,024; default 10) and
`comb service install --log-files N` (2–64; default 5).

API-key profiles can use `api_key_env` or `api_key_file`; see
[API authentication](cli-reference.md#api-key-profiles) for registration and
permissions. An environment source must be supplied in the service's private
environment file: a variable exported in your terminal does not update an existing
service. A private key file is read on new provider acquisitions and can be
replaced atomically without changing service registration. Restart the service
after changing environment-file values. Neither method copies the key into the
Hive YAML. API inference is paid usage; configure the provider's spending limits.

Without an available user service manager, setup reports manual commands and
leaves the configuration ready to start manually. In WSL, enable a systemd user
manager or use `setup --no-services`. To run foreground services:

```sh
comb --root "$HOME/Comb" serve
# In another terminal:
hivectl daemon run
```

`hivectl daemon start/stop/status` remain available for manually managed daemons.
Do not start a second manual instance over an owned running service.

## Upgrade and uninstall

Re-run the installer with the desired archives/variants. It stages and validates
binaries before stopping affected owned services, then switches to the new
programs. Only services that were loaded before upgrade restart automatically;
stopped scopes stay stopped. An interrupted upgrade can be retried.
Use `--yes` to consent to interrupting those services. Use `--no-setup` to install
or upgrade binaries without running the setup wizard.

Installation routes must be owned by you or trusted system directories; existing
user-owned group-writable directories are supported. The `lib/hive` manager must
be private to the installing user. Hive refuses unsafe ownership, symlinked managed
directories and changed file identities rather than changing their permissions.

Interrupted preparations may remain under the manager as `.preparing-*` or
`.pointer-*` directories. Retrying installation preserves unknown preparations;
image purge refuses them with recovery guidance. Retain the manager and its pending
installation journal, inspect the reported evidence, and complete recovery before
manually removing a preparation. Its name alone does not prove it is disposable.

| Installer flags | Meaning |
| --- | --- |
| `--from DIR\|HTTPS_URL`, `--prefix DIR` | Release assets and installation prefix (default `~/.local`). |
| `--core`, `--comb`, `--standalone-comb` | Select builds; standalone excludes Hive. Core and standalone conflict. Comb brings its bundled `comb-llama-server`, and removing Comb removes it. |
| `--no-comb-llama` | With `--comb` or `--standalone-comb`, skip the bundled llama-server; local embeddings then need `--llama-server PATH`. Refused while the bundled server is installed, since the flag does not remove it: uninstall Comb with `--uninstall --standalone-comb` first, then install it again with the flag. |
| `--no-setup`, `--yes` | Skip wizard; approve displayed service interruption/removal and forward no-prompts to setup. |
| `--setup-FLAG VALUE`, `-- SETUP_ARGS` | Forward wizard/global flags exactly, including spaces. |
| `--uninstall`, `--dry-run` | Remove selected components, or preview removal. |
| `--config FILE`, `--state-dir DIR`, `--comb-root DIR`, `--bridge-file FILE` | Require `--purge`; select the data to purge. Setup equivalents go after `--`. Service removal is prefix-wide for each selected component. |
| `--purge`, `--purge-comb-root DIR` | Delete selected owned data; whole knowledge root additionally requires explicit exact root. |
| `-h`, `--help` | Installer usage and examples. Native `hive-install --version` reports helper version. |

Installer options may appear once. The prefix must be absolute, with no `..` or
leaf symlink. Pass `--prefix` when `HOME` is unset. `--no-setup` conflicts with
`--uninstall`; setup arguments conflict with either, since neither runs the wizard.
Invalid installer input exits with status 2, JSON on stdout and an explanation
with examples on stderr, before downloading assets or changing the prefix.
Runtime installation and asset failures exit with status 1.

The native helper accepts a local asset directory; `install.sh` also accepts HTTPS:

```sh
hive-install --from /private/assets --prefix /private/hive --comb --no-setup
hive-install --prefix /private/hive --uninstall --yes
hive-install --prefix /private/hive --uninstall --purge --dry-run
```

```sh
# Hive only; preserve user data and Comb installation.
hivectl setup --uninstall --yes
# HIVE_RELEASE is the release URL from Quick install.
# Both installed parts, preserving data:
sh install.sh --from "$HIVE_RELEASE" --uninstall --comb --yes
# Preview exact data purge; no deletion:
sh install.sh --from "$HIVE_RELEASE" --uninstall --comb --purge --dry-run
# Apply after reviewing paths:
sh install.sh --from "$HIVE_RELEASE" --uninstall --comb --purge --yes
# Also delete knowledge pages: explicitly repeat the exact selected root.
sh install.sh --from "$HIVE_RELEASE" --uninstall --standalone-comb \
  --purge --comb-root "$HOME/Comb" --purge-comb-root "$HOME/Comb" --yes
```

Removing a component stops and unregisters **all its owned service scopes** that
use that installation prefix, including other Hive configurations or Comb
roots. The data-path flags narrow data cleanup; they do not narrow service removal.

Plain uninstall removes selected commands, units and descriptors, retaining data
and old program versions. Purge additionally removes the selected Hive config
and runtime records/output, exact setup bridge and selected service logs/diagnostic
records. It retains authentication profiles, native
provider conversation history and recovery evidence, config backups, user skill
copies and the small files needed to keep concurrent operations safe. Comb's pages, provenance, trust and index remain
unless `--purge-comb-root` explicitly selects the whole root. A bridge that cannot be proved/revoked is retained
with a recovery command. Orphan service output without an attributable
ownership descriptor is retained rather than guessed.

Programs and services are uninstalled before data cleanup. If purge fails, run
its printed retry command unchanged. Do not delete the files named by that command
to force progress. Purge refuses paths that were recreated. Old program versions are removed only by a full
purge when no component remains and no other process is observed using them.
Unknown contents or inability to check running programs stop that cleanup; purge
never kills processes. Keep the release asset helper available for retry after
public commands have been removed.

On Linux, protected processes owned by your user, including some SSH and PAM
session processes, can prevent inspection of `/proc/PID/exe`. Full image purge
then refuses with a permission error and retains the program versions. Plain
uninstall still works. Retain the helper and retry when the process inventory
can be inspected; Hive does not weaken the host's process protections.

## Troubleshooting

`hivectl doctor` returns JSON checks, repair commands and a short stderr summary. Its
exit status is nonzero when any check is an error. Warnings identify degraded or
pending capability without hiding it. `comb doctor` inspects the knowledge root.

| Check/problem | Action |
| --- | --- |
| Provider missing/unsupported | Follow the exact install/version hint; choose consent with `--install-provider`. |
| Login pending or expired | `hivectl auth status PROFILE`, then `hivectl auth login PROFILE` in a terminal. For a fence, use auth recovery commands in the CLI reference. |
| Daemon unavailable/mismatched | Inspect `hivectl service status`; start the owned service or manual daemon using the printed command. |
| Native sandbox permission denied | Inspect AppArmor/kernel denials from the daemon's launch context. On Ubuntu 24.04, use the distribution's Bubblewrap profile as described in [Codex sandboxing](https://learn.chatgpt.com/docs/sandboxing). A successful caller-local probe does not verify the service context. |
| Saved config not loaded | `hivectl setup --restart-daemon` for an owned service; manually stop/start an unmanaged daemon. |
| Comb unreachable | Inspect the selected root's service and exact bridge credential. Do not overwrite a foreign credential. |
| Backfill incomplete | Text search stays available; inspect doctor progress and service logs. Setup does not block waiting for embeddings. |
| Missing HTTP secrets | Supply the declared environment names through the private service environment file. |
| Skill copies edited/stale | Inspect the owned copy; restore or remove it deliberately before setup updates it. User edits are never overwritten. |
| Setup conflict | Inspect the named key/path. Use compatible explicit choices or a fresh config/state; setup will not silently replace it. |

### Claude sandbox prerequisites

On Ubuntu, install both sandbox dependencies before starting sandboxed Claude
agents:

```sh
sudo apt-get install bubblewrap socat
```

Claude refuses to start when `socat` is missing. Hive keeps the sandbox required;
the auth-free native helper probe does not check every Claude CLI dependency.

Claude's native Linux sandbox needs nested capability-bearing user namespaces.
Ubuntu's `bwrap-userns-restrict` profile can allow Bubblewrap itself but deny
capabilities to its children through `unpriv_bwrap`. In that case the bundled
helper fails at `/proc/self/setgroups` before the requested command runs. The
Codex native check can pass on the same host: it does not establish Claude readiness.
Inspect kernel AppArmor denials and the separate `claude_sandbox:daemon` check.
Its auth-free probe exercises the native helper, not a complete Bash session.

Do not repair this by setting `allowAllUnixSockets`, weakening nested isolation,
or retrying outside the requested sandbox. Host policy must support the native
helper; Hive cannot override a kernel denial. Script-based or older Claude
distributions without the bundled `ARGV0=apply-seccomp` entry point are not
verified by this probe. The diagnostic also refuses executable paths containing
`=` with `helper_path_contains_equals`: the environment launcher could otherwise
interpret that path as an assignment and skip the helper. Choose an executable
path without `=` to run this check. Ordinary agent launches are unaffected.

An administrator may explicitly grant a per-user exception by copying the
distribution's Bubblewrap executable to a separate root-owned path, with
root-owned non-writable parent directories and execute/read ACL access only for
the intended account. A dedicated AppArmor profile for that exact path can use
`flags=(unconfined)` with `userns,`. Keep `/usr/bin/bwrap`, its distribution profile,
and the global user-namespace sysctl unchanged. This is an actual host-policy
concession: the permitted account can execute the copy for other commands and
create capability-bearing nested namespaces. It is not equivalent to retaining
Ubuntu's child-capability restriction. Native Claude still enforces its own
filesystem, network, PID and seccomp restrictions.

Select the administrator-provisioned directory with `service install --service-path`
and restart the owned service. The prefix affects **both** providers in that
service; it is not a Claude-only setting. Retain the other required tool directories.
Afterward verify real agent execution and negative controls for forbidden writes,
Unix sockets and further user namespaces. Verify that the original Bubblewrap
path retains its prior restriction. Refresh the protected copy deliberately when
the distribution ships Bubblewrap security updates. Hive does not install this
exception or modify host policy automatically.

## Offline and air-gapped install

Pass local release archives with `install.sh --from DIR`. Comb downloads
nothing itself; an embedding server on your machine or network works offline.

```sh
sh /absolute/offline/releases/install.sh --from /absolute/offline/releases --comb --yes -- \
  --provider codex --login skip --codex-model gpt-6-luna --codex-effort low --no-services
```

Native provider login and inference still require the provider's network service;
installing files offline does not make subscription agents offline.

## Manual setup without the wizard

Install with `--no-setup`, then create a subscription-only `manual.yaml`:

```yaml
auth_profiles:
  codex-main: {provider: codex, method: subscription}
default_provider: codex
providers:
  codex: {auth: codex-main, model: gpt-6-luna, effort: low}
```

The broader [example config](../examples/auth.yaml) also has API profiles. If using
it, remove unused API profiles or provide their declared environment sources through a private
`--environment-file` during service installation; `api_key_file` profiles instead
read their configured private files. Register API profiles with `auth login`.
The subscription-only operations
are:

```sh
hivectl --config manual.yaml auth login codex-main
hivectl --config manual.yaml models --auth codex-main
comb init "$HOME/Comb"
comb --root "$HOME/Comb" model use none
mkdir -p "$HOME/.config/hive"
comb --root "$HOME/Comb" credential create hivectl --bridge \
  --reviewer-role comb-reviewer --out "$HOME/.config/hive/comb-bridge.json"
comb --root "$HOME/Comb" service install
comb --root "$HOME/Comb" service start
# Add comb.credential_file and provider defaults to your Hive config.
hivectl --config manual.yaml service install
hivectl --config manual.yaml service start
hivectl --config manual.yaml doctor
```

A manually created bridge is not a setup-created grant: setup/purge will not adopt
or revoke it by name. For idempotent setup ownership use `comb setup --for-hive`
instead of the manual credential-create step. See the
[CLI reference](cli-reference.md) and [Comb guide](../comb/README.md) for the
complete daily command interface.

Configuration output names, resolved symlink targets, bridge credentials and custom
Comb socket names must not use `.hive-tmp-state-` followed by exactly six ASCII
letters/digits or sixteen lowercase hexadecimal digits. Hive reserves those
basenames for managed state preparations; setup rejects them before publication.
