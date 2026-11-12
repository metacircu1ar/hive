"""Strict current daemon status observations for opt-in scenario checks."""


def counts(names):
    return {name: "u64" for name in names.split()}


def validate(value, schema, path):
    """Reject missing/unknown fields, wrong JSON types, and out-of-range numbers."""
    if isinstance(schema, dict):
        if type(value) is not dict or value.keys() != schema.keys():
            raise RuntimeError(f"Invalid fields at {path}")
        for name, child in schema.items():
            validate(value[name], child, f"{path}.{name}")
    elif isinstance(schema, list):
        if type(value) is not list:
            raise RuntimeError(f"Invalid array at {path}")
        for index, child in enumerate(value):
            validate(child, schema[0], f"{path}[{index}]")
    elif isinstance(schema, tuple):
        kind, child = schema
        if kind == "nullable":
            if value is not None:
                validate(value, child, path)
        elif kind == "map":
            if type(value) is not dict or not all(type(key) is str for key in value):
                raise RuntimeError(f"Invalid map at {path}")
            for name, item in value.items():
                validate(item, child, f"{path}.{name}")
    elif isinstance(schema, set):
        if type(value) is not str or value not in schema:
            raise RuntimeError(f"Invalid enum at {path}")
    elif isinstance(schema, str):
        signed = schema.startswith("i")
        bits = int(schema[1:])
        minimum, maximum = (-(1 << (bits - 1)), 1 << (bits - 1)) if signed else (0, 1 << bits)
        if type(value) is not int or not minimum <= value < maximum:
            raise RuntimeError(f"Invalid integer at {path}")
    elif type(value) is not schema:
        raise RuntimeError(f"Invalid type at {path}")


RUNTIME_LIMITS = {
    **counts(
        "max_agents max_loaded max_providers max_process_variants sessions_per_codex_process max_starting "
        "max_active_turns idle_seconds cohort_min_seconds warm_providers "
        "warm_idle_seconds linger_seconds"
    ),
    "live_stream": bool,
    "allow_spawn_widening": bool,
}
PROVIDER_CAPABILITY = {
    "provider": {"codex", "claude"},
    "subscription": bool,
    "next_turn": bool,
    "redirect": bool,
    "steering_policy": {"native_probe", "verified_canary"},
    "read_only_shell": bool,
}
PROVIDER_REGISTRY = {
    "provider": {"codex", "claude"},
    "minimum": str,
    "last_tested": str,
    "corpus_versions": [str],
}
PING = {
    "binary_sha256": str,
    "commit": ("nullable", str),
    "protocols": counts("minimum maximum"),
    "agent_catalog": "u32",
    "state_format": "u32",
    "provider_compatibility": [PROVIDER_REGISTRY],
    "deprecations": [
        {
            "key": str,
            "replacement": ("nullable", str),
            "since": str,
            "remove_in_major": "u64",
            "occurrences": "u64",
            "fixable": bool,
        }
    ],
    "provider_selection": {"codex": str, "claude": str},
    "configuration": ("nullable", {"path": str, "sha256": str}),
    "product_version": str,
    # barrier-delay: the storage-measurement build (storage measurement).
    "compiled_features": [{"comb", "barrier-delay"}],
    "status": {"running"},
    "pid": "u32",
    **counts("resident_actors profile_leases"),
    "storage": {
        "sqlite_allocated_bytes": "i64",
        "sqlite_cache_bytes": "i32",
        "pending_wake_ids": "u64",
        "ready": bool,
    },
    "capabilities": {"providers": [PROVIDER_CAPABILITY], "approvals": str},
    "reader_observation": {
        "source": {"completed_reader_jobs"},
        "counts": counts("requests snapshots max_snapshot_ns"),
    },
    "runtime_resources": {
        "provider_versions": [
            {
                "version": str,
                "support": {"tested", "untested", "unsupported"},
                "registry": PROVIDER_REGISTRY,
            }
        ],
        **counts(
            "active_turns reserved_shards recovered_provider_reservations unreaped_provider_reservations "
            "recovered_unresolved_activity_profiles resident_session_slots attached_agents "
            "starting_shards warm_shards lingering_shards draining_shards"
        ),
        "limits": RUNTIME_LIMITS,
    },
}
WRITER_COMMITTED = {
    "source": {"committed"},
    "runtime_id": str,
    **counts("sequence captured_unix_ms age_ns"),
}
WRITER_CURRENT = {"source": {"current_transaction"}}
