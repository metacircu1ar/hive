# Opt-in end-to-end checks

Run from a Hive source checkout with Python 3.10 or newer. These checks are
separate from Cargo tests and GitHub CI. Mock agents respond deterministically;
they use no provider login, model or quota.

Build the release binaries first:

```sh
cargo build --release --locked -p hive
python3 e2e/run.py
```

`python3 e2e/run.py prepare --tier mock` reports missing binaries and exact build
commands. Add `--install` to let it build them. It never signs in to a provider.

The default is a 100-agent ping-pong pair run. Each ping starts one pong, exchanges
messages, then stops it and itself. All-to-one starts a coordinator whose children
each exchange one ping/pong with it.

```sh
python3 e2e/run.py mock pairs --agents 100 --preset high-residency
python3 e2e/run.py mock all-to-one --agents 100 --preset eviction --max-providers 1
python3 e2e/run.py mock pairs --help
python3 e2e/run.py unit
```

`unit --pattern test_worker.py` selects one test module; `unit --help` explains
that selector. Driver-specific options follow the command, with an optional `--`.

High residency is the mock default. Eviction limits residency; the one-provider
example forces unloading and resumption even in a small run.
With multiple providers, resumption is observed rather than required: the
coordinator can stay resident while other processes recycle.
Use a new `--output-dir` for each run; defaults stay under ignored `dist/e2e/`.
Results include an independently checked message trace, timings and sampled
process-tree RSS. RSS includes shared pages and can miss peaks.

`--binary` selects another release `hivectl`. Its release directory must also
contain the matching `hive-daemon`. `HIVE_ROOT` optionally selects a different
source checkout for product fixtures; it defaults to this checkout.

To compare two release builds in alternating order, with disk-speed observations:

```sh
python3 e2e/run.py compare --arm before=/path/before/release/hivectl \
  --arm after=/path/after/release/hivectl --pairs 2 --max-pairs 2 \
  --output-dir /tmp/hive-comparison -- --agents 100
```

These runs create private temporary state and stop only their own daemons and
mock providers. Failed cleanup retains evidence and fails the run. They never
reuse your provider login or running Hive service.

Graph contracts use deterministic embeddings and no model. A small check runs
against the current Comb alone:

```sh
cargo build --release --locked -p comb
python3 e2e/run.py graph --pages 11 --smoke
python3 e2e/run.py graph --pages 10000
```

The small result is `smoke_passed`, with scale qualification false. The full run
checks the graph latency gate. Add `--baseline /path/release/comb` to compare
ordinary search results and latency with a caller-selected build; without it,
the comparison fields are null. Both builds get new independent roots.

Local embeddings use pinned Qwen3-Embedding-8B Q8_0, 4096 dimensions and last-token
pooling. Preparation reports missing prerequisites. `--install` explicitly allows
building binaries, installing missing tools and downloading the roughly 8 GB model:

```sh
python3 e2e/run.py prepare --tier embeddings
python3 e2e/run.py prepare --tier embeddings --install
python3 e2e/run.py model serve
```

Keep `model serve` running in a separate terminal. It owns its server process and
stops it when interrupted. Each harness server run has a 24-hour bound and a
64 MiB captured-output limit. The server is Comb's bundled `comb-llama-server` when
it is installed on `PATH` and reports the pinned commit. A server that fails
`--version` or reports another commit, such as an older installation after a pin
change, is ignored. Otherwise `--install` builds the same pinned static server,
with the release part's exact options, in the private user cache: Metal on macOS,
CPU only on Linux. The build needs `git`, `cmake` and a C/C++ compiler; it never
installs a system server. Linux prerequisite packages need root for `--install`;
otherwise the command prints the required apt command. It never signs in to a
provider.

The default cache is `$XDG_CACHE_HOME/hive-e2e` or `~/.cache/hive-e2e`. Existing
model files are rehashed before use. `prepare --cache DIR` and `model serve
--directory DIR` select another private cache. Prepared configuration is
`DIR/comb-embedding.json`; use that path below. Linux serving requires at least
16 GiB available memory, including cgroup limits; allocate at least 20 GiB to a
VM/container. Insufficient memory is refused before reading model weights.

```sh
python3 e2e/run.py embeddings smoke --embedding-config ~/.cache/hive-e2e/comb-embedding.json
python3 e2e/run.py embeddings smoke --full --embedding-config ~/.cache/hive-e2e/comb-embedding.json
python3 e2e/run.py embeddings contract --embedding-config ~/.cache/hive-e2e/comb-embedding.json
python3 e2e/run.py embeddings append --count 100
python3 e2e/run.py embeddings paired-append --before /path/before/release/comb \
  --after /path/after/release/comb --output-dir /tmp/comb-append-comparison
```

The default smoke indexes three short facts one at a time, then checks semantic
retrieval. Linux searches offline with a 120-second HTTP deadline. macOS searches
through the service with its 60-second request deadline. Each CLI call has a
180-second bound and the smoke stops after 15 minutes. A failed query ends the
check immediately. `--full` adds dense splitting, token-limit rejection and
background indexing; the contract also checks long inputs and batch parity.
Those larger workloads are explicit options and can exceed CPU query deadlines.
The append commands use no model. Each command's `--help` lists its options.

Native and container validation uses only committed source. The container default
is Linux at the host's architecture, with plain release Hive and Comb builds,
100 mock agents and the eleven-page graph smoke check. It has a one-hour bound:

```sh
python3 e2e/run.py prepare --tier container
python3 e2e/run.py prepare --tier container --install
python3 e2e/run.py container
python3 e2e/run.py container --full --platform macos --run-id acceptance
python3 e2e/run.py container --full --platform linux-arm64 --run-id acceptance
python3 e2e/run.py container --platform linux-amd64 --variant hive
python3 e2e/run.py container --distribution-only --platform linux-arm64
```

Run all macOS checks to completion before starting Linux on the same machine.
Non-native architectures require an explicit `--platform`; availability of an
emulator is not checked or promised. `--full` runs the workflow's complete variant
matrix, `--variant` selects named variants, and `--distribution-only` builds,
packages and checks the five release parts. These modes have a 24-hour bound.
Each step retains up to 64 MiB of output, including partial failure logs. They
are broader checks than the essential default.

Container preparation reports the exact host installation command. Only
`prepare --install` installs Docker or builds its image; running `container`
explicitly may also build the image. Docker must be running, with a local Unix
endpoint and a rootful daemon. Remote SSH/TCP endpoints and rootless daemons are
refused. Run as an ordinary user. The container uses that user's numeric identity
and `--init`, with only its fresh output directory and owned build-cache volumes
mounted. No source checkout, host home or provider credentials are mounted.
`--fresh` removes only caches carrying the expected ownership label; it never
force-removes an in-use volume. Native macOS work uses a clean checkout under
the short `~/.hive-e2e.noindex/` path.

Results default to `dist/e2e/ci-runs/PLATFORM-RUN/`; `--output-dir` selects a new
runs root and `--run-id` shares an ID across separate invocations. Logs, steps,
provisional results and any exported release assets remain there. The host writes
`results.json` only after every selected platform has finished and cleanup is
confirmed. Cancelled or uncertain runs have no accepted result, even if their
provisional output says tests passed. A completed run with failed checks records
`passed: false` and exits unsuccessfully. The essential mode does not qualify
release assets for publication.

Campaigns share a private, user-level admission record independent of checkout
and cache settings. Interrupted Docker work is reconciled against its recorded
endpoint and daemon identity before another campaign starts. A failed lookup or
an empty list after an unfinished create never proves cleanup. Native work left
after a killed parent also blocks new campaigns. Inspect the record with:

```sh
python3 e2e/run.py campaign status
python3 e2e/run.py campaign clear --nonce NONCE --manual-cleanup-confirmed
```

Use `clear` only after checking that **all** work and pending requests belonging
to that recorded campaign have finished. It trusts this manual confirmation;
it does not independently prove cleanup. The exact active nonce is required.

Native Linux sandbox checks need a real kernel and a systemd user manager. On
macOS, the VM tier uses native-architecture VZ guests from pinned Ubuntu cloud
images. Preparation reports prerequisites; only `--install` installs Lima or
downloads an image, and preparation never boots a guest:

```sh
python3 e2e/run.py prepare --tier vm --ubuntu 24.04
python3 e2e/run.py prepare --tier vm --ubuntu 24.04 --install
python3 e2e/run.py vm
python3 e2e/run.py vm probe --distribution-check
python3 e2e/run.py vm run --full --ubuntu 22.04 --memory-gib 20 \
  --suite-comparison --embedding-smoke
```

The default fresh Ubuntu 24.04 guest has 8 GiB RAM and a 40 GiB disk. It runs
plain release Hive and Comb builds, 100 mock agents, the graph smoke and auth-free
Codex/Claude sandbox checks in shell and service contexts. The two-hour default
and 24-hour full-run work bounds are caps, not expected durations. Bounded cleanup
and deletion run afterward even when that work deadline expires. `--ubuntu` also
accepts 22.04 and 26.04; `--memory-gib` accepts 8–24. Ubuntu 24.04 has strict
expected denial results under stock policy. Other versions record their observed
stock behavior and correlate native acceptance with doctor; a valid denial is
distinct from an infrastructure failure. When stock denies a provider, the short
run reports an incomplete result because it has not checked a remedy; use `--full`
to check the scoped remedy. No provider login or model call is used for these
native checks.

`--full` additionally freezes, packages and installs all five release parts and
checks distribution and per-user sandbox exceptions where the stock restriction
requires them. It restores the guest's own initial policy afterward. Explicit
`--suite-comparison` runs the release workflow and opt-in unit tests inside the
guest. When fallback assets are used, its installed-release check uses those
verified assets instead of rebuilding archives on the guest. `--embedding-smoke`
prepares the pinned Qwen model **inside** a
guest with at least 20 GiB RAM and checks retrieval through its local CPU server.
It needs model and build disk headroom as well as the host's 10 GiB free floor.
These options are separate from the default and cannot run with essential mode.

Self-contained installed-set checks and the full CI comparison use a separate
unprivileged guest account with a private checkout, cache and runtime directory.
They use the exact staged release assets. This avoids protected SSH/PAM processes
owned by the Lima login user blocking image-purge tests. Their reports record the
account context; CI reports, logs and assets are exported through checked staging.
Native sandbox, daemon, doctor and service checks keep the ordinary Lima user and
systemd service context. No process-protection settings are weakened.

Use an explicit output directory to stop and resume a full campaign:

```sh
python3 e2e/run.py vm run --full --output-dir /tmp/hive-vm-run --through distribution
python3 e2e/run.py vm run --full --output-dir /tmp/hive-vm-run --resume
python3 e2e/run.py vm guard
```

`--through` also accepts provisioning, source, build, package, install and policy
unit names shown by `vm run --help`. Resume requires the same committed source,
Ubuntu, RAM, execution mode and extra-suite selections. Completed evidence remains
intact; operational policy and service prerequisites are restored before the next
check. `vm create --output-dir DIR` only creates a stopped guest; a later
`vm run --output-dir DIR --resume` chooses its execution mode once.

Guests use the short private `~/.hive-vm.noindex` home, no host mounts, no public
SSH-key import, no agent forwarding and no host credentials. Only a clean bundle
of committed source and explicitly selected release assets enter the guest.
Foreign global Lima base/default/override configuration in that home is refused.
An inherited `LIMA_HOME` cannot redirect operations. Long account-home paths that
cannot fit macOS's SSH socket limit are refused before creation.
This folder is reserved to Hive: **do not create your own Lima VMs there**.
Cleanup ownership comes from this folder and the Hive marker in each saved
configuration. It does not require a run output directory or registry.

When packaging a newer Ubuntu build exceeds the supported glibc floor, full mode
can use `--fallback-assets DIR/assets` from a passing native Linux CI or distribution run.
Its parent `results.json`, exact file list, checksums, build records, platform and
source are checked. Provisional, failed and essential reports are refused. An
ancestor is allowed only when all changed files are release-contract files.
A complete passing VM run with `--suite-comparison` also exports its workflow
assets to `RUN/guest-suite-N/assets`. When the guest built those assets, the host
validates them after VM cleanup and publishes a CI `results.json` beside them for
use as fallback. Prebuilt fallback assets keep their original acceptance instead;
the VM result records their use without publishing a new CI acceptance.

The host writes final `results.json` after verified stop and optional deletion.
Only passing complete guests are deleted by default. `--keep`, failed checks and
partial campaigns retain a stopped guest plus its owner record and logs. Each
report gives the delete command. `vm guard` lists the actual guests in the private
folder with their native states and marker ownership:

```sh
python3 e2e/run.py vm guard
python3 e2e/run.py vm delete hive-INSTANCE
```

An exception or cancellation also leaves an **unaccepted** `retained.json` with
the cleanup outcome and delete command. Deletion checks the saved marker, stops
the guest, proves Stopped, deletes it and verifies absence. A marked Broken guest
reaches force-stop before refusal; if it remains Broken, cleanup fails and retains
it. Unmarked, malformed or symlinked entries are listed for manual attention and
never deleted. Removing a run's output does not hide its VM from guard or prevent
owned cleanup. Resume still requires its original strict execution evidence.

Interrupted VM work uses the same admission guard as containers. Durable pending
CLI proof lives beside that record; an empty lookup or remote state cannot replace
missing local completion. Unknown local cleanup blocks both new runs and deletion
until it is proven, or you explicitly confirm that all pending work has finished
with `campaign clear --nonce NONCE --manual-cleanup-confirmed`. That confirmation
changes only the shared admission record and does not depend on a missing, stale
or unwritable output. Deletion clears a matching run record only after cleanup is
proven; an unrelated record still blocks it. A completed create CLI can settle
an absent create; a start also needs checked remote stop. If a dispatched start
fails before publishing its hostagent PID, even a nonzero client exit and Stopped
listing are ambiguous: the hostagent may still appear. The guard waits for that
observation or explicit manual cleanup confirmation. Failures before dispatch
do not enter this pending-start state. Run macOS, containers and VMs serially
on the same physical host. Guests and retained reports are local only.

Downloads keep an interrupted `.part` file for a later resume. A server that does
not support byte ranges, or a corrupt/oversized partial, can require removing that
cache file before repeating preparation. Stop its downloader first. Source
archives and model files are always verified against their declared hashes.

## Live messaging

Live checks require `--live`, an existing subscription login and release builds.
They consume quota. They use your existing Hive state (`~/.local/state/hive`, or
`--state-dir`), with fresh workspaces and output directories. They never log in or
copy credentials. Stop your daemon and finish any other open agents first. Each
check starts its own daemon and ends its agents before shutting it down, including
after a failure or timeout. Pre-existing deliverable collector work causes a
pre-launch refusal; paused unrelated evidence is preserved.

```sh
# Two Codex Terra 6.1 agents at low effort; ten-minute work limit.
python3 e2e/run.py live pairs --live

# One coordinator and two children, all Codex Terra 6.1 at low effort.
python3 e2e/run.py live all-to-one --live

# The original ten-child exchange remains an explicit run.
python3 e2e/run.py live all-to-one --live --children 10

# Fifty Codex/Claude pairs, up to thirty minutes, with resident memory sampling.
python3 e2e/run.py live pairs --live --pairs 50 --providers mixed

# Select Claude explicitly for a small pair or all-to-one check.
python3 e2e/run.py live pairs --live --providers claude
python3 e2e/run.py live all-to-one --live --provider claude
```

`pairs` accepts `--pairs 1..50`, `--max-starting 1..100`, `--codex-model` and
`--claude-model`. `all-to-one` accepts `--children 1..10`, `--expected-model`,
`--expected-effort` and an optional `--config` matching those roles/profiles.
Both accept `--binary`, `--state-dir`, `--output-dir` and `--timeout` in seconds.
The output directory must be new. The one-pair and all-to-one work limits are at
most ten minutes; larger pairs allow at most thirty minutes. Bounded teardown
can take another ninety seconds plus the daemon's shutdown grace.

Pairs record startup, resident hold, exchange, shutdown and verification timings,
and process-tree RSS on both platforms. macOS also records native physical
footprint; Linux reports that metric as unavailable. A process shared by several
conversations is counted once, with all its roles listed. RSS includes shared
pages and is not unique physical RAM or the remote model's weights.

Unknown cleanup retains the shared campaign record and evidence for inspection.
Use `campaign status` and its explicit manual confirmation flow only after proving
that all recorded work has stopped. No lost spawn reply causes a replacement
coordinator to be launched.


## Live knowledge

These checks require the same stopped Hive state and existing Codex login as live
messaging, plus a matching release Comb binary and the running local embedding
server described above. They use fresh knowledge roots and retain tool-exchange
evidence without bridge credentials. They never start an embedding model implicitly.

```sh
# One Terra 6.1 agent writes a fact, then retrieves it by semantic and literal search.
python3 e2e/run.py live comb --live --embedding-config ~/.cache/hive-e2e/comb-embedding.json

# One Terra 6.1 agent writes graph facts, reads them, retracts a relation and reads again.
python3 e2e/run.py live comb-graph --live --embedding-config ~/.cache/hive-e2e/comb-embedding.json

# Ten writers, a reader and a curator; includes correction and self-learning checks.
python3 e2e/run.py live comb --live --full --embedding-config ~/.cache/hive-e2e/comb-embedding.json

# The same graph checks with separate writer and reader conversations.
python3 e2e/run.py live comb-graph --live --full --embedding-config ~/.cache/hive-e2e/comb-embedding.json

# Curator upkeep: stale summaries, citations, broken links and accepted merges.
# Full mode repeats with a fresh curator to check idempotency.
python3 e2e/run.py live comb-upkeep --live --full --embedding-config ~/.cache/hive-e2e/comb-embedding.json
```

These commands accept `--binary`, `--root-binary` (Comb), `--state-dir`, `--output-dir`,
`--auth codex-main`, `--model`, `--effort` and `--timeout` in seconds. Essential checks use Terra 6.1
at low effort and a ten-minute work limit. The full knowledge check defaults to
Terra 6.1 at medium effort with a one-hour limit. Graph checks require Terra 6.1 at low
effort and retain the ten-minute limit in both modes. Upkeep defaults to Terra 6.1
at low effort, with a ten-minute limit in both modes. It checks three fixture pages,
preserved evidence, unchanged accepted proposals and out-of-scope content. Full mode
uses a second fresh curator to check that repaired pages stay unchanged.
Teardown has a separate bounded
cleanup budget. Guessed codes, retrieval fallback, missing source reads, provider
rerouting and incomplete tool evidence fail the check.

The full knowledge check reserves the collector name `hive-e2e` and reuses it on
later runs, so retained destinations do not accumulate. Do not use that name for
other work. Any pending, uncertain or gap evidence for it is refused before launch,
even when paused. Only this run's binding is discarded during cleanup; paused
unrelated collector evidence stays intact. The essential and graph checks do not
configure a collector. Custom login profile names are not inferred by these checks;
use the existing `codex-main` subscription profile. Collector refusals and cleanup
failures print exact inspection/discard commands for the selected state. Inspect
the evidence before discarding it, with that state's normal daemon running.


## Live controls and review teams

Preparation reports release Hive/Comb binaries, installed Codex/Claude executables
and the running local embedding server, without signing in or requesting embeddings.
Missing tools print exact build/install/serve commands. `--install` allows missing
release builds, provider npm packages and embedding prerequisites to be installed;
it never signs in or starts a model server. Provider installation uses the official
[npm Codex package](https://github.com/openai/codex) and
[Claude npm installation](https://code.claude.com/docs/en/setup#install-with-npm).
Node/npm must already be installed, with a user-writable global prefix.

```sh
python3 e2e/run.py prepare --tier live
python3 e2e/run.py prepare --tier live --install

# Six small checks, serially, with a one-hour overall work limit.
python3 e2e/run.py live --live --embedding-config ~/.cache/hive-e2e/comb-embedding.json

# Same-session history, model change, stop/resume, interrupt and redirect.
python3 e2e/run.py live controls --live

# Claude controls and same-turn steering acceptance require Claude support.
python3 e2e/run.py live controls --live --provider claude

# Explicit two-agent factorial review with meaningful independent Python checks.
python3 e2e/run.py live factorial --live

# Lead, two Codex workers and a read-only Claude reviewer with skills.
python3 e2e/run.py live shop-team --live
python3 e2e/run.py live shop-team --live --answer decline

# Explicit101-agent historical-commit review and resource measurement.
python3 e2e/run.py live commit-review --live

# Create or independently recheck a fixture without provider calls.
python3 e2e/run.py live commit-review --prepare-only --output-dir /tmp/hive-review-fixture
python3 e2e/run.py live commit-review --verify-only --output-dir /path/to/completed-run
```

All live leaves accept `--binary`, `--state-dir`, a new `--output-dir`, and `--timeout`
in seconds. Defaults use existing `codex-main`/`claude-main` subscription profiles.
They own their daemon and terminalize the entire task lineage on every exit.
Detached start/watch/verify modes are retired. Ordinary `hivectl` inspection can
still observe the owned daemon while a check is running.

The default set runs shop-team first, then pairs, all-to-one, controls, Comb and
Comb graph. The enclosing one-hour work cap shortens later checks' remaining time;
an unfinished tail fails and is listed. Each child retains its own accepted report.
The set publishes its result only after every child exits successfully with its
passing report. Cancellation allows a separately bounded cleanup tail of up to
three minutes for hosted tasks, services, samplers and process-absence checks.
Unknown cleanup retains the common admission guard and evidence.

Controls use one Codex Terra 6.1 low conversation, change it to Sol medium for
one recall, then restore Terra 6.1 low.
The work cap is ten minutes. Explicit Claude checks retain Sonnet low → Haiku low →
Sonnet low. `--provider both` runs Codex then Claude; model and
changed-model/effort flags are listed by `--help`. Claude steering is checked only
when explicitly selected; an unsupported canary fails, never redirects silently.
Factorial uses Terra 6.1 medium for both roles and a twenty-minute work cap. It
checks the original faulty snapshot, findings followed by clean feedback, unchanged
conversations,201correct factorials,3negative inputs and9invalid types. Its
`--require-resumed-tool` and checkpoint hash options add the named assertions.

Shop-team uses Terra 6.1 medium for the lead/workers and Sonnet low for its reviewer,
with a thirty-minute work cap. The driver answers exactly one Hive approval using
`--answer accept|decline`, then verifies named roles, joins, the reviewer's sandbox
and skills, one completion, independent behavior, and the matching changelog choice.
The model/effort selectors are available through `--help`.

Commit-review defaults to Terra 6.1 medium for its lead and 100 Terra 6.1 low reviewers,
with a four-hour absolute work cap, thirty-minute no-progress guard and32GiB RSS
abort threshold. A longer absolute run requires an explicit `--timeout`; the
watchdog and smaller RSS cap can be selected explicitly. Each reviewer gets a
different faulty historical commit in its own detached checkout, then rechecks the
same assigned current file until clean. Exactly100matching approvals, final bytes,
input-to-turn provenance, untouched contracts/history and native shutdown are checked.
This explicit stress measurement deliberately uses one conversation per Codex
process and retains all101sessions, separately from the shipping16-session default.
It samples RSS and CPU with process identities; shared pages and missed peaks remain
measurement limitations. The offline verifier checks saved protocol/file/memory
evidence; it neither clears admission nor proves cleanup of a currently running task.
