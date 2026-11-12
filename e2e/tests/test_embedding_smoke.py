"""Actual release Comb emits short single-input HTTP requests for the CPU smoke."""

from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from e2e.paths import product_default
from e2e.embeddings import smoke
from e2e.embeddings import qwen_contract
from e2e.embeddings.http import HTTPFailure, post
from release.files import atomic_write_json


class LocalEndpoint(HTTPServer):
    def get_request(self):
        client, address = super().get_request()
        client.settimeout(2)
        return client, address


class EmbeddingSmoke(unittest.TestCase):
    def test_standalone_vector_contract_refuses_boolean_and_nonfinite_values(self):
        valid = [1.0] + [0.0] * 4095
        config = dict(url="http://127.0.0.1/embeddings", model="fixture", dimensions=4096)
        for index, vector, accepted in (
            (0, valid, True),
            (False, valid, False),
            (0, [True] + valid[1:], False),
            (0, [float("nan")] + valid[1:], False),
            (0, valid[:-1], False),
        ):
            with (
                self.subTest(index=index, accepted=accepted),
                patch.object(
                    qwen_contract,
                    "post",
                    return_value=dict(data=[dict(index=index, embedding=vector)]),
                ),
            ):
                if accepted:
                    self.assertEqual(qwen_contract.vectors(config, ["input"]), [valid])
                else:
                    with self.assertRaises(RuntimeError):
                        qwen_contract.vectors(config, ["input"])

    def endpoint(self):
        received = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                received.append(payload["input"])
                if payload.get("reject"):
                    self.send_response(413)
                    body = json.dumps(dict(error="fixture limit")).encode()
                else:
                    self.send_response(200)
                    rows = []
                    for index, text in enumerate(payload["input"]):
                        vector = [0.0] * 4096
                        vector[0 if "France" in text else 1 if "China" in text else 2] = 1.0
                        rows.append(dict(index=index, embedding=vector))
                    body = json.dumps(dict(data=rows)).encode()
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = LocalEndpoint(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.02))
        thread.start()

        def close():
            server.shutdown()
            thread.join(3)
            server.server_close()
            self.assertFalse(thread.is_alive())

        self.addCleanup(close)
        return f"http://127.0.0.1:{server.server_port}/v1/embeddings", received

    def test_cpu_uses_offline_search_short_payloads_and_one_input_per_request(self):
        binary = product_default("target/release/comb")
        self.assertTrue(binary.is_file(), "Build release Comb first")
        url, received = self.endpoint()
        with tempfile.TemporaryDirectory() as temporary:
            configuration = json.loads(product_default("examples/qwen3-embedding.json").read_text())
            configuration["url"] = url
            config = Path(temporary) / "embedding.json"
            atomic_write_json(config, configuration)
            report = smoke.execute(binary, config, cpu=True)
        self.assertEqual(report["route"], "offline")
        self.assertEqual(report["query_deadline_seconds"], 120)
        self.assertEqual(report["coverage"]["embedded"], 3)
        self.assertEqual(len(received), 6)
        for inputs in received:
            self.assertEqual(len(inputs), 1)
            self.assertLessEqual(len(inputs[0].encode()), smoke.INPUT_BYTES)

    def test_owned_http_worker_retains_explicit_rejection_without_retry(self):
        url, received = self.endpoint()
        with (
            patch.dict(
                os.environ,
                dict(
                    http_proxy="http://127.0.0.1:1",
                    HTTP_PROXY="http://127.0.0.1:1",
                    no_proxy="",
                    NO_PROXY="",
                ),
            ),
            self.assertRaises(HTTPFailure) as raised,
        ):
            post(url, dict(input=["short"], reject=True))
        self.assertEqual(raised.exception.status, 413)
        self.assertEqual(raised.exception.body, dict(error="fixture limit"))
        self.assertEqual(received, [["short"]])
