# Build and test policy

Always use release builds for validation, tests, scenarios, and benchmarks. Never
run them against debug builds.

- Build binaries with `cargo build --release --locked`.
- Run Rust tests with `cargo test --release --locked`.
- Run Clippy with `cargo clippy --release --locked --all-targets -- -D warnings`.
- Run CLI and Python scenarios against `target/release/hivectl` (and
  `target/release/hive-daemon` when needed). Build first; do not fall back to debug
  binaries. Explicit binary overrides must also point to release builds.
- `cargo fmt --check` is independent of the build profile.
- Also run `rustfmt --edition 2024 --check tests/cli/fixture.rs tests/setup_cli/uninstall.rs`
  for the two included files that Cargo's formatter does not follow.

For installer argument and bootstrap validation, first build the helper with
`cargo build --release --locked -p hive-install`, then run
`python3 -m unittest discover -s tests -p test_installer_arguments.py`.
The suite requires that release binary and uses isolated local transport fixtures.

Keep historical measurements labeled with the build profile actually used.

On a shared physical host, run the macOS validation campaign to completion before
starting native Linux validation in its local container or VM. Do not overlap
their builds, tests or scenarios. Keep `RUST_TEST_THREADS=4` and existing deadlines;
retain failures separately from any focused reruns. This reduces interference but
does not establish a cause or fix for intermittent failures. Independent hosted
runners may still run in parallel.

Compatibility corpora live under `compat/`. The synthetic-0 recipe uses current
formats and deterministic mock providers; it is not a development-format reader.
Its populated roots are generated and upgraded in place so native paths and
inode-bound ownership evidence remain valid. Never rewrite that evidence to load
a copied fixture. Rust corpus and transcript tests run in the normal release suites.
After plain package-scoped Hive and Comb release builds, run the binary matrix:

```sh
python3 scripts/check_compatibility.py --current-dir target/release --output-dir dist/compatibility
```

The output directory must be new. `compat/releases.json` pins each declared
release's native GitHub release archive and
each consumed executable and corpus recipe by SHA-256. Recipes pin every mock,
input and transcript; released copies live under `compat/<product-version>/`.
The driver upgrades every release's mock-produced corpus, checks newer-data
refusal and runs both control/service/MCP directions for the current and previous
release lines. Older lines get protocol-range checks and explicit refusal when
they have no shared protocol. Add the immutable recipe and fixtures at declaration.
No real provider account is used by this matrix. Live provider canaries require a
separate authorized release environment; ordinary CI only uses recorded fixtures.

After the release contract tests, run
`PYTHONPATH=scripts python3 -m release.compatibility_gate --baseline HEAD` for
local changes (CI compares with its parent commit). The inventory binds each
registry to production-tested fixtures; it is not a source-schema inference tool.
Incomplete corpus coverage is listed as warnings and never blocks an explicit
release declaration from freezing the formats as they stand. Keep those gaps
accurate; closing them is not required for a release. Supported released
fingerprints remain immutable.

Maintainer checks that need private tools are documented in
`../common_document_storage/hive/AGENTS.md`; follow it too when that checkout is
present. Product builds, tests, packaging and public CI never require it.

# Release status and format policy

Hive 0.1.0 is the first supported release. Its contracts are declared at
format/index/protocol/catalog version 1; their fingerprints and the frozen mock
corpus under `compat/0.1.0/` belong to it. An earlier preliminary 0.1.0
publication is not a supported predecessor; do not add adapters, aliases or
migration tests for it.

Preserve actual build provenance: a release commit may add generated binaries and
archive pins to the source they were built from, but its remaining tree must be
identical. Never rewrite build records to claim the binaries came from a later
commit. Keep the exact assembled assets and frozen parts until publication;
repackaging changes archive pins.

Preserve history with ordinary commits. Any change to a frozen contract requires
the corresponding version and reviewed upgrade work below. Linux x86_64 builds
made under emulation are not native hardware acceptance; retain that distinction
in release reports and product docs.

Keep exact structural checks (including schema SQL and strict parsing) in addition
to markers. Do not add legacy development readers, compatibility aliases,
markerless adoption or fallback bindings. Refuse unsupported development state
unchanged, including missing required markers in a nonempty root. Use a new
state directory or knowledge root for unsupported development data; never
automatically reset identities, seal keys or credentials. Preserve current
provider protocol handling and corruption/ownership checks.

The actual first supported release freezes that product version's exact formats
and protocols. Afterward, preserve published history with normal commits, and
every newer release must migrate data from any earlier released format
through reviewed steps, after a durable backup, and refuse newer formats safely.
A changed frozen format needs a new number, a frozen fixture and a migration;
derived indexes may instead be rebuilt. Numbers change only with the corresponding
format or protocol, never merely for a development iteration. Draft formats may
change freely between releases; development data still has no compatibility
promise. Synthetic migration/skew steps remain test-only. Support current and
previous released protocols with exact per-protocol shapes, without inventing a
predecessor before the first release. Product patch releases do not change format
or protocol numbers. Release support and the eventual release cut are separate.

# Documentation and incubation boundary

Style the product name as **Hive** in prose and display text. Keep executable,
package, path and tool identifiers lowercase (`hive`, `hivectl`, `hive-daemon`, `hive-install`,
`comb`, `hive_*`, `comb_*`). Branding changes must not rewrite stored markers or historical evidence.

Hive contains product code, user documentation, skills, examples and the tests and
drivers needed to validate it. Keep the README, CLI reference, installation and
operating guides complete without access to an internal repository.

Plans, specifications for future work, investigations, review notes and recorded
benchmark/validation results do not belong in this repository; keep them with the
maintainers' separate documentation. Do not recreate an internal docs archive here
or make builds, tests or packages require another checkout. Generated scratch
outputs may stay ignored locally.

# Fixture regeneration

Comb fixtures under `comb/tests/fixtures` pin exact serialization, protocol
output and the public error catalogue. Do not regenerate them to hide a regression.
For a deliberate reviewed contract change, run the filtered release test with
`COMB_RECORD_GOLDENS=1`, inspect its diff, then rerun without that variable.
Product tests read only the committed fixtures.

# Release assembly and acceptance

Use package-scoped commands to avoid workspace feature unification. Do not package
Cargo test-produced binaries. The four Cargo part builds must be plain release
builds. `comb-llama` is the fifth, non-Cargo part: a CMake build of the pinned static
`llama-server`, shipped as `comb-llama-server`. Its builder needs `git`, `cmake` and a
C/C++ compiler on the admitted PATH and network access to fetch the pinned commit. It
fetches that commit by ID, verifies the commit and tree IDs from
`comb/src/llama/pin.json` and builds in a new private tree removed afterwards. The
pin is the only copy of the llama.cpp commit/tree and the Qwen revision, SHA-256 and
size; the builder, Comb and `e2e/` all read it, and a placeholder refuses each.
Python 3.10+ is needed only for these maintainer tools.

The `barrier-delay` feature is a storage measurement build: `HIVE_BARRIER_DELAY_MS`
(0–1000, read at daemon start) delays each successful ledger barrier. It is a CI
and policy variant (`hive-barrier-delay`), never a part; its version names the
feature, so the build driver and the installer refuse it.

### Assemble a native set from source

Python is required here, on the build machine only. Each output directory must be
fresh. Run builds serially; the driver refuses a source change during a build.

```sh
for part in hive hive-core comb comb-llama hive-install; do
  python3 scripts/build_release.py --part "$part" --output-dir "dist/frozen/$part"
  python3 scripts/package_release.py --build-dir "dist/frozen/$part" --output-dir dist/assets
done
```

`--clean` on the build driver uses a new temporary Cargo target directory to
measure a clean build. Both modes freeze the plain package build before any tests,
record its hashes and dependency notices, and require that record when packaging.
Packaging refuses non-system libraries and, on Linux, any C++ runtime requirement.

The release driver owns a separate `target/.release-builder` cache (0700), locked
through compilation, artifact copying and dependency/source checks. `--clean`
uses an owned temporary cache. Do not point ordinary Cargo commands at that cache.
Packaged guides, skills, examples, logos and `install.sh` come from the build's
frozen `inputs/` directory. Packaging never rereads those payloads from the checkout.
Native assets share one `assets.json` ledger binding their source, version, frozen
inputs and exact part/build/member identities. Package into a fresh directory;
do not add files by hand or combine contributions from different freezes. Both
Hive alternatives are explicit parts. Packaging validates each binary's declared
profile, exact features and actual version before extending the locked set.
Acceptance requires all five native parts and snapshots the complete verified set
before running its bootstrap or installer helper. `SHA256SUMS` is an index of the
committed ledger; acceptance refuses a missing or stale index. Retrying the exact
same contribution can repair an index publication failure. Other partial outputs
or unknown/mutated members require a fresh directory and are never adopted.
Source identity includes tracked paths, modes, deletions and internal file-symlink
targets/content, plus untracked paths classified as product inputs, including
ignored source files. Git filenames are read as NUL-delimited bytes. Source
directory symlinks are refused. Every path dependency, including build-only
dependencies, must have its manifest and target sources in that snapshot;
third-party path packages also require all package inputs to be bound. Unclassified
packaged inputs, external source links and path dependencies outside the checkout
are refused. Private inode/timestamp evidence detects changes during the freeze;
only the deterministic relative source digest is published.

Build, Cargo tree/metadata and toolchain queries share one admitted environment.
Compiler flags/wrappers/targets/profiles, native CC/CFLAGS/AR, SDK overrides and
dependency build overrides must be unset. Incoming Cargo jobs, incremental and
color settings are normalized to4/false/never. The driver passes only toolchain/
home/temp locations and HTTP/TLS transport settings, plus fixed locale/timezone
and macOS11.0 deployment settings for Cargo parts. The optional comb-llama part
uses macOS13.3 for its pinned Metal/Accelerate APIs. Public build records omit those private paths,
transport values and credentials. Cargo part builds remap the source root, Cargo home,
Rust library sources and target directory to `/hive`, `/cargo`, `/rustc/<commit>`
and `/target`, so compiled panic locations do not reveal the builder's paths;
the record lists only those public names.

Every ancestor `.cargo/config`/`config.toml` and the effective Cargo home are
checked. Allowed keys: `build.jobs`, `build.incremental`; `net.git-fetch-with-cli`,
`net.offline`, `net.retry`; `http.proxy`, `http.cainfo`, `http.timeout`,
`http.low-speed-limit`, `http.multiplexing`, `http.check-revoke`; `term.color`,
`term.verbose`, `term.quiet`; and `registries.crates-io.protocol`. The restricted
Python3.10-compatible grammar accepts bare names, one-line strings, booleans and
integers; it refuses duplicate tables/keys, dotted/quoted keys, arrays, inline
tables and multiline values. All other configuration, including env tables,
linkers, build-script overrides, paths/patches and source replacement/mirrors
(even checksum-pinned mirrors), is refused. For a build requiring isolation from
personal settings, use an explicitly selected empty Cargo home and a checkout
whose ancestors contain no Cargo config; retain the toolchain and registry caches
separately. Do not delete or rewrite the user's existing config or credentials.

### Check an installed set

The native acceptance driver installs into fresh prefixes, starts deterministic
mock agents, runs a separate installed Comb server, checks collector delivery,
and indexes through a fixture HTTP embedding endpoint before vector search.
It never uses real provider accounts or real models:

```sh
python3 scripts/check_release_install.py --assets dist/assets --output-dir dist/installed-checks
```

The output directory must be new. It retains installed prefixes, logs and a JSON
report with verified plain-release binary hashes. Temporary test state is private
and short-lived. CI runs this check on the native Ubuntu 22.04 and macOS 15 legs.

Mock acceptance does not establish that a host can run Codex's native sandbox.
On Linux, run the separate auth-free acceptance check with a real Codex executable:

```sh
HIVE_NATIVE_CODEX=/absolute/path/to/codex cargo test --release --locked -p hive --test runtime native_sandbox::native_sandbox_acceptance_without_authentication -- --ignored --exact
```

It creates an isolated daemon and temporary provider home, invokes no agent or
model and uses no login. Run the test in the intended service context too (for
example, via `systemd-run --user --wait --pipe --collect`); a terminal may have a
different AppArmor profile. `hivectl doctor` checks the already-running daemon's
context and reports caller-local fallback separately.

Claude has a separate auth-free Linux prerequisite check using its native bundled
helper and Bubblewrap:

```sh
HIVE_NATIVE_CLAUDE=/absolute/path/to/claude cargo test --release --locked -p hive --test runtime native_sandbox::claude_sandbox_acceptance_without_authentication -- --ignored --exact
```

Use a native Claude distribution with `ARGV0=apply-seccomp` support. The public
Linux Hive job pins Claude 2.1.318 and runs this check without credentials or model
calls. Its success does not establish the service's AppArmor context or replace
real-agent execution and negative confinement checks on the deployment host.

# Product text and file categories

`scripts/release/policy.py`, exercised by `tests/test_release_packaging.py`,
classifies every tracked file and checks retained guides/examples, package
summaries, CLI help, MCP descriptions/instructions and setup/installer output.
Classify a new product path explicitly before committing it. Keep plans,
experiments and recorded results outside this repository. The explicitly
classified opt-in end-to-end suite lives in `e2e/` with runnable inputs, tests
and one short guide. Run it through `python3 e2e/run.py`; GitHub CI never invokes
or discovers it. Other scale harnesses are not product files; do not expand a product
category just to admit them. Stage intended deletions before
running the tracked-file check. Local untracked task lists are not product files.

Keep installed-product examples and actionable operating limits in product docs.
Build/test/packaging instructions for maintainers belong here. Product builds,
packages and tests must work from this checkout alone.

# File publication checks

Use the crates.io `atomic-fs` primitives for file publication and
`scripts/release/files.py` for Python publication. Product helper tests are
`PYTHONPATH=scripts python3 -m unittest release.test_files release.test_publication`.
Never downgrade authoritative durability to satisfy a performance check. Select
`AtomicVisible` only for a justified regenerable output; preserve Comb's ordered
and coalesced barriers through the helper's durability interface.

# Diagnostic logging checks

Emit product diagnostics with qualified `tracing` macros and explicit static event
names. Use only fields in `logging/src/fields.json`: bounded identities, codes,
counts, flags and native `Error` references. Never log raw messages, prompts,
provider payloads, credentials or error `Debug`/`Display`. The `hive-log` backend
filters library fields and owns bounded queues, rotation and recovery. Spans carry
static names, not field payloads. Reserve target `hive.fatal` for fatal `error!`
events; ordinary errors use the normal queue.

Review field semantics: a field name cannot prove that a value labeled as an ID
is safe.

# Python maintainer diagnostics and error checks

Use `scripts/release/progress.py` for long-running maintainer commands: wrap the
entrypoint with `entry`, group work with `phase` (ten-second heartbeats), and use
`finalizer`/`cleanup_all` so secondary cleanup errors cannot replace the primary
failure or prevent the remaining cleanup. `scripts/release/process.py` checks
subprocess exit status before parsing JSON. A nonzero status is never an empty
successful response. Explicit negative tests still inspect their expected status.

Maintainer signal handlers record INT/TERM/HUP; only explicit safe-point checks
cancel work. Use the shared owned process runner, not subprocess waits directly.
It bounds IO/deadlines, pins the unreaped leader through group cleanup, and accepts
only a real native reap status. SIGCHLD auto-reap is disabled before child creation
and its prior native disposition restored afterward. Lost ownership forbids later
signals; uncertain child cleanup retains temporary roots instead of removing files
that may still have a writer. Do not turn an observation failure into exit zero.
Give native owners their own shutdown budget: setup/login commands need12seconds
to retire their separately grouped provider; running services keep20seconds. Drain
bounded captured IO throughout graceful cleanup before the final group signal/reap.

Build, package, compatibility and installed checks retain bounded raw native stderr
separately from redacted diagnostics. Files are owner-only, limited to64MiB per
command, and never included in release archives. Builds/installed checks use the
output's `diagnostics/`; package/compatibility checks use a sibling
`<output>.diagnostics/`; the compatibility gate uses `dist/diagnostics/`. A failed
compiler/tool invocation keeps its actionable stderr there without exposing it in
structured logs. An evidence-write failure cannot turn a native failure into success.

Use `release.locking.directory_lock` in maintainer drivers. It supplies cooperative
cancellation and the enclosing absolute deadline to the standalone publication
primitive. The copied mock publication module stays self-contained; its default
lock wait is bounded and uses no maintainer signal handler. All file locks use
nonblocking attempts. Network downloads run in an owned worker and their exact
pinned SHA-256 is checked before consumption. Mock HTTP clients and worker joins
also have bounds; no abandoned network writer thread is an accepted cleanup.

Call `release.cancellation.finish` only for the final, finite report publication.
At a CLI entry it defers that action until operation, temporary-root and phase
cleanup have passed, then makes one cancellation/success decision. A signal before
that decision returns128+signal with no passing report. Later signals cannot revise
the committed success decision; publication failures still fail and preserve their
committed/retained outcome. Diagnostics expose those flags without private paths.

The Python backend uses standard `logging` with a bounded 1,024-record queue,
8 KiB record limit and 250 ms shutdown wait. A daemon writer owns a duplicate of
stderr; a stuck stderr consumer does not block interpreter exit. Causes contain
bounded types, source locations, errno, status and output sizes, never exception
arguments, subprocess output or source text. Library messages are redacted.
Loss counters are available from `Session.finish()`; the final loss receipt is
best effort. If a diagnostic worker cannot start, entry fails before work begins.

Run the maintainer helper tests with:

```sh
PYTHONPATH=scripts python3 -m unittest discover -s scripts/release -t scripts
```

Bare assertions are allowed only in tests.
