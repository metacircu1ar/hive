"""One provider-trace state machine with memory and SQLite evidence backends."""

import json
from release.cancellation import check
from .io import CompleteLines
from collections import Counter
from itertools import zip_longest

from .checks import require
from .evidence import TraceRow


class MemoryTraceEvidence:
    def __init__(self, manifest, count):
        self.manifest, self.count = manifest, count
        self.frames, self.processes = [], {}
        self.bindings, self.seen_threads = {}, set()
        self.sessions = Counter(map(tuple, manifest["sessions"]))
        self.started, self.ended = set(), set()

    def frame(self, row: TraceRow, raw: str) -> None:
        self.frames.append(row)

    def process(self, row: TraceRow) -> None:
        require(row["process"] not in self.processes, "Duplicate process start")
        self.processes[row["process"]] = row

    def commit(self) -> None:
        pass

    def provider_rows(self):
        return sorted(
            ((name, row["pid"]) for name, row in self.manifest["providers"].items()),
            key=lambda item: item[1],
        )

    def process_rows(self):
        rows = sorted(self.processes.values(), key=lambda row: (row["pid"], row["ns"]))
        return ((row["process"], row["pid"]) for row in rows)

    def bind(self, process, provider) -> None:
        self.bindings[process] = provider

    def ordered_frames(self):
        return iter(sorted(self.frames, key=lambda row: row["ns"]))

    def provider(self, process):
        return self.bindings[process]

    def validate_thread(self, row: TraceRow) -> None:
        thread = row["thread"]
        resumed = row["kind"] == "thread_resume"
        require(
            self.manifest["threads"].get(thread) == row["agent"]
            and (thread in self.seen_threads) == resumed,
            "Wrong trace conversation",
        )

    def thread(self, row: TraceRow, provider) -> None:
        thread = row["thread"]
        resumed = row["kind"] == "thread_resume"
        self.seen_threads.add(thread)
        session = provider, thread, resumed
        require(self.sessions[session] > 0, "Wrong trace session binding")
        self.sessions[session] -= 1

    def turn_start(self, row: TraceRow, provider) -> None:
        expected = self.manifest["turns"].get(row["turn"])
        require(
            expected
            and expected["message"] == row["message"]
            and expected["thread"] == row["thread"]
            and expected["provider"] == provider
            and row["turn"] not in self.started,
            "Wrong trace turn identity",
        )
        self.started.add(row["turn"])

    def turn_end(self, row: TraceRow) -> None:
        require(
            row["turn"] in self.started
            and row["turn"] not in self.ended
            and self.manifest["turns"][row["turn"]]["status"] == row["status"],
            "Wrong trace turn status",
        )
        self.ended.add(row["turn"])

    def finish(self) -> None:
        require(
            len(self.seen_threads) == self.count
            and not any(self.sessions.values())
            and self.started == self.ended == set(self.manifest["turns"]),
            "Trace omitted runtime evidence",
        )


class SqliteTraceEvidence:
    def __init__(self, audit):
        self.audit = audit

    def frame(self, row: TraceRow, raw: str) -> None:
        self.audit.execute("INSERT INTO frames VALUES(?,?)", (row["ns"], raw))

    def process(self, row: TraceRow) -> None:
        self.audit.execute(
            "INSERT INTO processes(process,pid,ns) VALUES(?,?,?)",
            (row["process"], row["pid"], row["ns"]),
        )

    def commit(self) -> None:
        self.audit.commit()

    def provider_rows(self):
        return self.audit.execute("SELECT id,pid FROM providers ORDER BY pid,seq")

    def process_rows(self):
        return self.audit.execute("SELECT process,pid FROM processes ORDER BY pid,ns")

    def bind(self, process, provider) -> None:
        self.audit.execute("UPDATE processes SET provider=? WHERE process=?", (provider, process))

    def ordered_frames(self):
        self.audit.execute("CREATE INDEX frame_time ON frames(ns)")
        self.commit()
        return (
            json.loads(raw) for (raw,) in self.audit.execute("SELECT data FROM frames ORDER BY ns")
        )

    def provider(self, process):
        return self.audit.execute(
            "SELECT provider FROM processes WHERE process=?", (process,)
        ).fetchone()[0]

    def validate_thread(self, row: TraceRow) -> None:
        thread = row["thread"]
        resumed = row["kind"] == "thread_resume"
        expected = self.audit.execute(
            "SELECT agent,seen FROM threads WHERE thread=?", (thread,)
        ).fetchone()
        require(
            expected and expected[0] == row["agent"] and bool(expected[1]) == resumed,
            "Wrong trace conversation",
        )

    def thread(self, row: TraceRow, provider) -> None:
        thread = row["thread"]
        resumed = row["kind"] == "thread_resume"
        self.audit.execute("UPDATE threads SET seen=1 WHERE thread=?", (thread,))
        require(
            self.audit.execute(
                "UPDATE sessions SET seen=1 WHERE provider=? AND thread=? AND resumed=? AND seen=0",
                (provider, thread, int(resumed)),
            ).rowcount
            == 1,
            "Wrong trace session binding",
        )

    def turn_start(self, row: TraceRow, provider) -> None:
        require(
            self.audit.execute(
                "UPDATE turns SET started=1,start_ns=? WHERE turn=? AND message=? AND thread=? AND provider=? AND started=0",
                (row["ns"], row["turn"], row["message"], row["thread"], provider),
            ).rowcount
            == 1,
            "Wrong trace turn identity",
        )

    def turn_end(self, row: TraceRow) -> None:
        require(
            self.audit.execute(
                "UPDATE turns SET ended=1,end_ns=? WHERE turn=? AND status=? AND started=1 AND ended=0",
                (row["ns"], row["turn"], row["status"]),
            ).rowcount
            == 1,
            "Wrong trace turn status",
        )

    def finish(self) -> None:
        for table, where in (
            ("threads", "seen=0"),
            ("sessions", "seen=0"),
            ("turns", "started=0 OR ended=0"),
        ):
            require(
                not self.audit.execute(f"SELECT 1 FROM {table} WHERE {where} LIMIT 1").fetchone(),
                "Trace omitted runtime evidence",
            )
        self.commit()


def ingest_trace(path, evidence) -> None:
    with CompleteLines(path) as stream:
        first = json.loads(next(stream))
        require(first["kind"] == "process_start", "Missing trace start")
        connection = json.loads(next(stream))
        require(connection["kind"] == "connection", "Missing trace connection")
        runtime = connection["runtime"]
    with CompleteLines(path) as stream:
        prior, saw_exit = -1, False
        for raw in stream:
            check()
            require(not saw_exit, "Incomplete trace lifecycle")
            row = json.loads(raw)
            require(
                row["process"] == first["process"]
                and row["pid"] == first["pid"]
                and row["ns"] > prior,
                "Invalid trace lifetime/clock",
            )
            prior = row["ns"]
            saw_exit = row["kind"] == "process_exit"
            if runtime:
                evidence.frame(row, raw)
            else:
                require(
                    row["kind"] in ("process_start", "connection", "process_exit"),
                    "Unattributed mock work",
                )
    require(saw_exit, "Incomplete trace lifecycle")
    if runtime:
        evidence.process(first)
    evidence.commit()


class TraceVerifier:
    def __init__(self, evidence, limits):
        self.evidence, self.limits = evidence, limits
        self.resident, self.active, self.providers = {}, {}, {}
        self.active_per_process = Counter()
        self.peaks = dict(
            peak_mock_processes=0,
            peak_resident_threads=0,
            peak_active_turns=0,
            peak_same_process_turns=0,
        )
        self.barriers = 0
        self.first_start = self.last_turn = self.last_exit = None

    def verify(self, paths, check_disk=None):
        for path in paths:
            check()
            if check_disk is not None:
                check_disk(path.parent)
            ingest_trace(path, self.evidence)
        for provider, process in zip_longest(
            self.evidence.provider_rows(), self.evidence.process_rows()
        ):
            require(
                provider and process and provider[1] == process[1],
                "Provider process identity mismatch",
            )
            self.evidence.bind(process[0], provider[0])
        for count, row in enumerate(self.evidence.ordered_frames(), 1):
            check()
            self.observe(row)
            self.capacity()
            if check_disk is not None and count % 100000 == 0:
                check_disk(paths[0].parent)
                print(
                    json.dumps(dict(phase="trace_verification", verified_frames=count)), flush=True
                )
        require(not self.resident and not self.active, "Incomplete trace lifecycle")
        require(self.barriers > 0, "No verified overlap")
        self.evidence.finish()
        return dict(
            **self.peaks,
            overlap_barriers=self.barriers,
            trace_times_ns=dict(
                first_start=self.first_start, last_turn=self.last_turn, last_exit=self.last_exit
            ),
        )

    def observe(self, row: TraceRow) -> None:
        process, kind = row["process"], row["kind"]
        if kind == "process_start":
            require(process not in self.resident, "Duplicate process start")
            self.resident[process] = set()
            self.providers[process] = self.evidence.provider(process)
            if self.first_start is None:
                self.first_start = row["ns"]
        else:
            require(process in self.resident, "Event outside process lifetime")
        handlers = {
            "thread_start": self.thread,
            "thread_resume": self.thread,
            "turn_start": self.turn_start,
            "turn_end": self.turn_end,
            "barrier": self.barrier,
            "unsubscribe": self.unsubscribe,
            "process_exit": self.process_exit,
        }
        if kind in handlers:
            handlers[kind](row)
        else:
            require(kind in ("process_start", "connection"), "Unknown trace event")

    def thread(self, row: TraceRow) -> None:
        thread, process = row["thread"], row["process"]
        self.evidence.validate_thread(row)
        require(
            not any(thread in values for values in self.resident.values()),
            "Concurrent/duplicate conversation ownership",
        )
        self.evidence.thread(row, self.providers[process])
        self.resident[process].add(thread)

    def turn_start(self, row: TraceRow) -> None:
        process = row["process"]
        key = process, row["thread"]
        require(
            key not in self.active and row["thread"] in self.resident[process],
            "Overlapping/nonresident turn",
        )
        self.evidence.turn_start(row, self.providers[process])
        self.active[key] = row["turn"]
        self.active_per_process[process] += 1

    def turn_end(self, row: TraceRow) -> None:
        require(
            self.active.pop((row["process"], row["thread"]), None) == row["turn"],
            "Wrong trace turn completion",
        )
        self.evidence.turn_end(row)
        self.active_per_process[row["process"]] -= 1
        self.last_turn = row["ns"]

    def barrier(self, row: TraceRow) -> None:
        if row["overlapped"]:
            require(
                len(set(row["threads"])) >= 2
                and all((row["process"], thread) in self.active for thread in row["threads"]),
                "False overlap barrier",
            )
            self.barriers += 1

    def unsubscribe(self, row: TraceRow) -> None:
        require(
            row["thread"] in self.resident[row["process"]]
            and (row["process"], row["thread"]) not in self.active,
            "Invalid unsubscribe",
        )

    def process_exit(self, row: TraceRow) -> None:
        process = row["process"]
        require(
            not any(owner == process for owner, _ in self.active), "Process exited with active turn"
        )
        del self.resident[process], self.providers[process]
        self.active_per_process.pop(process, None)
        self.last_exit = row["ns"]

    def capacity(self) -> None:
        loaded = sum(map(len, self.resident.values()))
        for name, value in (
            ("peak_mock_processes", len(self.resident)),
            ("peak_resident_threads", loaded),
            ("peak_active_turns", len(self.active)),
            ("peak_same_process_turns", max(self.active_per_process.values(), default=0)),
        ):
            self.peaks[name] = max(self.peaks[name], value)
        require(
            len(self.resident) <= self.limits["max_providers"]
            and loaded <= self.limits["max_loaded"]
            and len(self.active) <= self.limits["max_active_turns"]
            and all(
                len(values) <= self.limits["sessions_per_codex_process"]
                for values in self.resident.values()
            ),
            "Resource limit exceeded",
        )
