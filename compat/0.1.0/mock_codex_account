"""Codex fixture account on its single protocol runtime."""

import os
import pathlib
import sys
import time
from mock_common import (
    wait_for_gate,
    publish_text,
    claim_counter,
)


class Account:
    def handle_account(self, request, conversation):
        if self.scenario.get("reject_account_read"):
            self.emit({"id": request["id"], "error": {"code": -32000, "message": "rejected"}})
            return
        kind = self.scenario.get("api_account", "apiKey" if self.api_key is not None else "chatgpt")
        self.reply(request, {"requiresOpenaiAuth": True, "account": {"type": kind}})

    def handle_requirements(self, request, conversation):
        if self.scenario.get("api_requirements_unsupported"):
            self.emit({"id": request["id"], "error": {"code": -32601, "message": "unsupported"}})
        else:
            self.reply(request, self.scenario.get("api_requirements", {"requirements": None}))

    def handle_config(self, request, conversation):
        self.reply(
            request,
            self.scenario.get(
                "api_config",
                {"config": {"cli_auth_credentials_store": "ephemeral", "model_provider": "openai"}},
            ),
        )

    def handle_api_login(self, request, conversation):
        assert self.scenario.get("api_key"), "subscription must not perform API login"
        stores = [value for value in sys.argv if value.startswith("cli_auth_credentials_store=")]
        assert stores[-1] == 'cli_auth_credentials_store="ephemeral"'
        assert "OPENAI_API_KEY" not in os.environ and "CODEX_API_KEY" not in os.environ
        assert "ANTHROPIC_API_KEY" not in os.environ
        self.api_key = request["params"]["apiKey"]
        assert self.api_key and self.api_key not in sys.argv
        self.emit({"method": "account/updated", "params": {"private": self.api_key}})
        if self.scenario.get("api_login_error"):
            self.emit(
                {
                    "id": request["id"],
                    "error": {"code": -32000, "message": self.api_key, "data": self.api_key},
                }
            )
        else:
            if self.scenario.get("api_auth_file"):
                publish_text(self.home / "auth.json", self.api_key)
            self.reply(request, {"type": self.scenario.get("api_login_kind", "apiKey")})

    def handle_rate_limits(self, request, conversation):
        self.reply(
            request,
            {"rateLimits": self.scenario.get("rate_limits_read", {}), "ordinaryUsageAllowed": None},
        )

    def handle_models(self, request, conversation):
        if self.scenario.get("model_list_gate"):
            release = pathlib.Path(self.scenario["model_list_gate"])
            publish_text(pathlib.Path(self.scenario["model_list_waiting"]), str(os.getpid()))
            wait_for_gate(release, timeout=30, timeout_error=RuntimeError("catalog gate timed out"))
        if self.scenario.get("model_list_delay"):
            time.sleep(self.scenario["model_list_delay"])
        models = self.scenario.get(
            "catalog_models", ["gpt-5.6-terra", "gpt-5.6-sol", "gpt-5.6-luna"]
        )
        if (
            self.scenario.get("catalog_once_model")
            and claim_counter(self.home / "catalog-once", 1) is not None
        ):
            models = models + [self.scenario["catalog_once_model"]]
        self.reply(
            request,
            {
                "data": [
                    {
                        "model": m,
                        "supportedReasoningEfforts": [
                            {"reasoningEffort": e} for e in ("high", "medium")
                        ],
                    }
                    for m in models
                ],
                "nextCursor": None,
            },
        )
