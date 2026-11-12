"""Read existing durable execution/provider evidence after the daemon is reaped."""

from dataclasses import dataclass
import json


@dataclass(frozen=True)
class Snapshot:
    paused: dict
    provider_after: int
    agent_after: int


class RetirementFailure(RuntimeError):
    def __init__(self, kind, identity, reason):
        if (
            not isinstance(identity, str)
            or not 1 <= len(identity) <= 128
            or not all(char.isascii() and (char.isalnum() or char in "-_") for char in identity)
        ):
            raise ValueError("Malformed retirement identity")
        self.report = dict(kind=kind, identity=identity, reason=reason)
        super().__init__("Scenario retirement proof failed")


def verify(database, snapshot):
    started, finished = set(), {}
    for provider, kind, data in database.execute(
        "SELECT provider,kind,data FROM provider_events WHERE seq>? "
        "AND kind IN ('started','shutdown') ORDER BY seq",
        (snapshot.provider_after,),
    ):
        if kind == "started":
            if provider in started:
                raise ValueError("Duplicate provider start identity in retirement evidence")
            started.add(provider)
            finished.pop(provider, None)
        else:
            value = json.loads(data)
            if not isinstance(value, dict) or not isinstance(value.get("shutdown"), dict):
                raise ValueError("Malformed provider retirement evidence")
            finished[provider] = value["shutdown"]
    for provider in started:
        report = finished.get(provider)
        if report is None or any(
            report.get(field) is not True for field in ("confirmed", "quiescent", "reaped")
        ):
            raise RetirementFailure("provider", provider, "retirement_unconfirmed")
    for identity, data, state, outcome, expired, thread, execution, provider in database.execute(
        "SELECT id,data,state,task_outcome,expired,thread_id,execution_id,provider_id "
        "FROM agents WHERE listing_seq>?",
        (snapshot.agent_after,),
    ):
        value = json.loads(data)
        if not isinstance(value, dict) or type(value.get("conversation_clean")) is not bool:
            raise ValueError("Malformed agent retirement evidence")
        attempted = value.get("native_input_attempted", False)
        if type(attempted) is not bool:
            raise ValueError("Malformed native input attempt evidence")
        profile = value.get("profile")
        claude = isinstance(profile, dict) and profile.get("provider") == "claude"
        never_launched = (
            execution is None and provider is None and not attempted and (thread is None or claude)
        )
        if state not in ("stopped", "failed"):
            raise RetirementFailure("agent", identity, "execution_open")
        if outcome not in ("completed", "cancelled") and expired != 1:
            raise RetirementFailure("agent", identity, "task_open")
        if value.get("recovery_fence") is not None:
            raise RetirementFailure("agent", identity, "recovery_fence")
        if not never_launched and value["conversation_clean"] is not True:
            raise RetirementFailure("agent", identity, "conversation_unconfirmed")
