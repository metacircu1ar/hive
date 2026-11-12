"""Own one produced compatibility corpus through upgrade and resume."""

import json
from .acceptance import ready
from .acceptance import json_command, running, wait_for, require, mock_login
from .compatibility_corpus import StateCorpus, KnowledgeCorpus, completed
from . import compatibility_transcripts as transcripts
from .compatibility_mcp import McpClient
from .compatibility_readers import check_previous_reader, refuse_newer
from .files import atomic_write_json, atomic_write_text


class Pair:
    def __init__(self, matrix, label, work, embedding):
        self.matrix = matrix
        self.label = label
        self.work = work
        self.embedding = embedding

    def run(self, cli, daemon, comb_client, comb_server):
        self.cli = cli
        self.daemon = daemon
        self.comb_client = comb_client
        self.comb_server = comb_server
        self.initialize()
        with running(
            self.producer + ["serve", "--embedding-config", str(self.embedding)],
            self.matrix.output / (self.label + "-comb.log"),
            self.matrix.env,
        ) as server:
            self.prepare_state(server)
            with running(
                [str(self.daemon), *self.args, "daemon", "run"],
                self.matrix.output / (self.label + "-hive.log"),
                self.matrix.env,
            ) as process:
                self.seed_agents(process)
            self.observe_search(server)
        self.upgrade_data()
        with running(
            self.upgraded + ["serve", "--embedding-config", str(self.embedding)],
            self.matrix.output / (self.label + "-comb-upgraded.log"),
            self.matrix.env,
        ) as server:
            wait_for(
                lambda: self.socket.exists() and ready(self.invoke_current, "stats"),
                "upgraded Comb startup",
                server,
            )
            self.verify_knowledge()
        json_command(
            [str(self.matrix.current / "comb"), "--root", str(self.knowledge), "sync", "--rebuild"],
            env=self.matrix.env,
        )
        with running(
            self.upgraded + ["serve", "--embedding-config", str(self.embedding)],
            self.matrix.output / (self.label + "-comb-rebuilt.log"),
            self.matrix.env,
        ) as server:
            wait_for(
                lambda: self.socket.exists() and ready(self.invoke_current, "stats"),
                "rebuilt Comb startup",
                server,
            )
            self.verify_knowledge()
            self.latest = self.matrix.control(
                self.matrix.current / "hivectl", self.state, self.config
            )
            with running(
                [str(self.matrix.current / "hivectl"), *self.args, "daemon", "run"],
                self.matrix.output / (self.label + "-resumed.log"),
                self.matrix.env,
            ) as process:
                self.resume_agents(process)
        self.refusal_checks()
        return {
            "control": True,
            "hosted": True,
            "collector": True,
            "mcp": True,
            "connection_files": True,
            "upgrade_resume": True,
            "newer_data_refusal": True,
            "branch": True,
            "attachments": True,
            "artifacts": True,
            "trust": True,
            "receipts": True,
            "credentials": True,
            "text": True,
            "vector": True,
            "hybrid": True,
            "graph": True,
            "references": True,
            "graph_diagnostics": True,
        }

    def initialize(self):
        self.knowledge, self.socket = self.work / "knowledge", self.work / "comb.sock"
        self.producer = [
            str(self.comb_server),
            "--root",
            str(self.knowledge),
            "--socket",
            str(self.socket),
        ]
        self.consumer = [
            str(self.comb_client),
            "--root",
            str(self.knowledge),
            "--socket",
            str(self.socket),
        ]
        self.invoke = lambda *args: json_command(self.consumer + list(args), env=self.matrix.env)
        json_command(
            [str(self.comb_server), "--root", str(self.knowledge), "init"], env=self.matrix.env
        )
        self.brain = KnowledgeCorpus(self.work, self.matrix.inputs)
        self.corpus = StateCorpus(self.work, self.matrix.inputs)

    def prepare_state(self, server):
        wait_for(
            lambda: self.socket.exists() and ready(self.invoke, "stats"),
            "compat Comb startup",
            server,
        )
        self.bridge = self.work / "bridge.json"
        # The server and Hive daemon are opposite generations in both skew
        # directions. Its CLI writes the connection that Hive will consume.
        json_command(
            self.producer
            + ["credential", "create", "compat-bridge", "--bridge", "--out", str(self.bridge)],
            env=self.matrix.env,
        )
        self.reader = self.work / "reader.json"
        json_command(
            self.producer + ["credential", "create", "compat-reader", "--out", str(self.reader)],
            env=self.matrix.env,
        )
        self.state = self.work / "state"
        self.config = self.work / "config.json"
        self.instructions = self.work / "role.md"
        atomic_write_text(self.instructions, "Deterministic compatibility fixture.\n")
        atomic_write_json(
            self.config,
            {
                "auth_profiles": {"mock": {"provider": "codex", "method": "subscription"}},
                "roles": {
                    "implementor": {
                        "auth": "mock",
                        "model": self.matrix.inputs["model"],
                        "effort": self.matrix.inputs["effort"],
                        "instructions": str(self.instructions),
                        "lifetime": "independent",
                        "sandbox": "read-only",
                    }
                },
                "comb": {"credential_file": str(self.bridge)},
                "comb_collectors": {
                    "shared": {"credential_file": str(self.bridge), "sources": ["correction"]}
                },
            },
            mode=0o600,
        )
        self.args = [
            "--state-dir",
            str(self.state),
            "--config",
            str(self.config),
            "--codex-bin",
            str(self.matrix.provider),
        ]
        mock_login([str(self.daemon), *self.args], env=self.matrix.env)
        self.call = self.matrix.control(self.cli, self.state, self.config)

    def untrusted(self, page):
        actions = [
            {
                "name": "comb_put",
                "arguments": {"slug": page["slug"], "markdown": page["markdown"]},
            }
        ]
        agent = self.call(
            "spawn",
            "implementor",
            "--cwd",
            str(self.work),
            "--body",
            "comb:" + json.dumps(actions),
        )["agent_id"]
        self.wait(lambda: completed(self.call, agent), "compat hosted turn")

    def seed_corpora(self):
        self.brain.seed(self.invoke, self.untrusted)
        if self.comb_client == self.comb_server:
            transcripts.service(self.socket, self.reader, self.matrix.transcripts["service"])
        self.corpus.seed(self.call, self.wait)

    def control_call(self):
        reply = McpClient(
            [str(self.cli), "--state-dir", str(self.state), "mcp"], self.matrix.env
        ).call(
            "hive_call",
            {
                "target": self.corpus.source,
                "body": "reply:compat-mcp",
                "request_id": "compat-mcp",
                "timeout": 10,
            },
        )
        require(
            reply["outcome"] == "replied" and "compat-mcp" in json.dumps(reply["result"]),
            "Control MCP reply mismatch",
        )

    def seed_agents(self, process):
        self.wait = lambda probe, name: wait_for(probe, name, process)
        self.wait(
            lambda: (
                (self.state / "runtime/control.sock").exists()
                and ready(self.call, "daemon", "status")
            ),
            "compat Hive startup",
        )
        if self.cli == self.daemon:
            transcripts.control(
                self.state / "runtime/control.sock", self.matrix.transcripts["control"]
            )
        self.seed_corpora()
        self.control_call()
        self.queue_collector(process)

    def queue_collector(self, process):
        self.hidden = self.work / "bridge-hidden.json"
        self.bridge.rename(self.hidden)
        self.evidence_args = (
            "evidence",
            "correction",
            "--summary",
            self.matrix.inputs["collector"],
            "--request-id",
            "compat-evidence",
        )
        self.evidence = self.call(*self.evidence_args)
        require(
            self.call("collectors", "status")["collectors"][0]["pending"] == 1,
            "Outbox corpus was not pending",
        )
        self.call("daemon", "stop")
        require(process.wait(timeout=20) == 0, "Compatibility daemon failed to stop")
        self.hidden.rename(self.bridge)

    def observe_search(self, server):
        self.before_search = wait_for(
            lambda: self.brain.searches(self.invoke), "corpus embeddings", server
        )
        cases = (
            self.matrix.transcripts["mcp"]
            if self.comb_client == self.comb_server
            else [self.matrix.transcripts["mcp"][0]]
        )
        for case in cases:
            page = McpClient(
                [str(self.comb_client), "mcp", "--credential", str(self.reader)], self.matrix.env
            ).call(
                "comb_get",
                {"slug": self.matrix.inputs["pages"]["trusted"]["slug"]},
                offer=case["offer"],
                selected=case["selected"],
            )
            require(
                page == self.brain.documents[self.matrix.inputs["pages"]["trusted"]["slug"]],
                "Knowledge MCP read mismatch",
            )
            for operation in self.matrix.transcripts["tools"]:
                result = McpClient(
                    [str(self.comb_client), "mcp", "--credential", str(self.reader)],
                    self.matrix.env,
                ).call(
                    operation["tool"],
                    operation["arguments"],
                    offer=case["offer"],
                    selected=case["selected"],
                )
                transcripts.expected(result, operation["expect"])

    def upgrade_data(self):
        for command in (
            [
                str(self.matrix.current / "hivectl"),
                "--state-dir",
                str(self.state),
                "state",
                "upgrade",
            ],
            [str(self.matrix.current / "comb"), "--root", str(self.knowledge), "upgrade"],
        ):
            json_command(command + ["--dry-run"], env=self.matrix.env)
            json_command(command, env=self.matrix.env)
        self.upgraded = [
            str(self.matrix.current / "comb"),
            "--root",
            str(self.knowledge),
            "--socket",
            str(self.socket),
        ]
        self.invoke_current = lambda *args: json_command(
            self.upgraded + list(args), env=self.matrix.env
        )

    def verify_knowledge(self):
        self.brain.verify(self.invoke_current, self.before_search)
        page = McpClient(
            [str(self.matrix.current / "comb"), "mcp", "--credential", str(self.reader)],
            self.matrix.env,
        ).call("comb_get", {"slug": self.matrix.inputs["pages"]["trusted"]["slug"]})
        require(
            page == self.brain.documents[self.matrix.inputs["pages"]["trusted"]["slug"]],
            "Upgrade lost credential authorization",
        )
        transcripts.service(self.socket, self.reader, self.matrix.transcripts["service"])
        for operation in self.matrix.transcripts["tools"]:
            result = McpClient(
                [str(self.matrix.current / "comb"), "mcp", "--credential", str(self.reader)],
                self.matrix.env,
            ).call(operation["tool"], operation["arguments"])
            transcripts.expected(result, operation["expect"])

    def resume_agents(self, process):
        self.wait = lambda probe, name: wait_for(probe, name, process)
        self.wait(
            lambda: (
                (self.state / "runtime/control.sock").exists()
                and ready(self.latest, "status", self.corpus.child)
            ),
            "upgraded agent visibility",
        )
        self.corpus.verify(self.latest, self.wait, self.state)
        require(
            self.latest(*self.evidence_args) == self.evidence,
            "Upgrade lost collector request deduplication",
        )
        self.wait(
            lambda: self.latest("collectors", "status")["collectors"][0]["pending"] == 0,
            "upgraded outbox drain",
        )
        pages = json_command(
            self.upgraded + ["list", "--prefix", "inbox/"], env=self.matrix.env, allow_list=True
        )
        matching = [
            p
            for p in pages
            if self.matrix.inputs["collector"] in json.dumps(self.invoke_current("get", p["slug"]))
        ]
        require(len(matching) == 1, "Upgraded outbox lost or duplicated capture")
        self.latest("daemon", "stop")
        require(process.wait(timeout=20) == 0, "Upgraded daemon failed to stop")

    def refusal_checks(self):
        check_previous_reader(
            self.state,
            self.state / "format",
            "state",
            [str(self.daemon), "--state-dir", str(self.state), "state", "upgrade", "--dry-run"],
            self.matrix.env,
        )
        check_previous_reader(
            self.knowledge,
            self.knowledge / ".comb/format",
            "root",
            [str(self.comb_server), "--root", str(self.knowledge), "upgrade", "--dry-run"],
            self.matrix.env,
        )
        refuse_newer(
            self.state,
            self.state / "format",
            [str(self.daemon), "--state-dir", str(self.state), "state", "upgrade", "--dry-run"],
            self.matrix.env,
        )
        refuse_newer(
            self.knowledge,
            self.knowledge / ".comb/format",
            [str(self.comb_server), "--root", str(self.knowledge), "upgrade", "--dry-run"],
            self.matrix.env,
        )
