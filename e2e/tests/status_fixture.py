"""Synthetic complete daemon status input for current parser tests."""


def status(pid=123, **extra):
    return dict(
        binary_sha256="c" * 64,
        commit=None,
        protocols=dict(minimum=1, maximum=1),
        agent_catalog=1,
        state_format=1,
        provider_compatibility=[
            dict(
                provider="codex",
                minimum="0.154.0",
                last_tested="0.161.0",
                corpus_versions=["0.154.0", "0.161.0"],
            )
        ],
        deprecations=[],
        configuration=None,
        provider_selection={"codex": "a" * 64, "claude": "b" * 64},
        product_version="0.1.0",
        compiled_features=["comb"],
        pid=pid,
        status="running",
        resident_actors=0,
        profile_leases=0,
        storage=dict(
            sqlite_allocated_bytes=0, sqlite_cache_bytes=0, pending_wake_ids=0, ready=True
        ),
        capabilities=dict(
            providers=[
                dict(
                    provider="codex",
                    subscription=True,
                    next_turn=True,
                    redirect=True,
                    steering_policy="native_probe",
                    read_only_shell=True,
                ),
                dict(
                    provider="claude",
                    subscription=True,
                    next_turn=True,
                    redirect=True,
                    steering_policy="verified_canary",
                    read_only_shell=False,
                ),
            ],
            approvals="advisory asynchronous human questions; native provider approvals remain disabled",
        ),
        writer_observation=dict(
            source="committed", runtime_id="runtime", sequence=1, captured_unix_ms=1, age_ns=0
        ),
        reader_observation=dict(
            source="completed_reader_jobs", counts=dict(requests=0, snapshots=0, max_snapshot_ns=0)
        ),
        runtime_resources=dict(
            provider_versions=[],
            active_turns=0,
            reserved_shards=0,
            recovered_provider_reservations=0,
            unreaped_provider_reservations=0,
            recovered_unresolved_activity_profiles=0,
            resident_session_slots=0,
            attached_agents=0,
            starting_shards=0,
            warm_shards=0,
            lingering_shards=0,
            draining_shards=0,
            limits=dict(
                max_agents=128,
                max_loaded=128,
                max_providers=128,
                max_process_variants=8,
                sessions_per_codex_process=1,
                max_starting=8,
                max_active_turns=128,
                idle_seconds=60,
                cohort_min_seconds=5,
                warm_providers=0,
                warm_idle_seconds=60,
                linger_seconds=0,
                live_stream=True,
                allow_spawn_widening=False,
            ),
        ),
        **extra,
    )
