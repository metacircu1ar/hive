# Native Ubuntu runner for opt-in source validation.
# Match the invoking user's numeric identity so private output mounts are writable.
# Tests run as an unprivileged user; --init owns orphan reaping.
FROM ubuntu:22.04
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ARG RUST_VERSION=1.98.1
ARG RUNNER_UID=1000
ARG RUNNER_GID=1000
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config libssl-dev cmake git curl ca-certificates \
        python3 python3-venv tmux xz-utils unzip file procps \
        systemd \
    && rm -rf /var/lib/apt/lists/*
RUN (getent group "$RUNNER_GID" || groupadd --gid "$RUNNER_GID" runner) \
    && useradd --create-home --uid "$RUNNER_UID" --gid "$RUNNER_GID" runner \
    && mkdir -p /work/target /home/runner/.cargo/registry \
    && chown -R "$RUNNER_UID:$RUNNER_GID" /work /home/runner \
    && install -d -m 700 -o "$RUNNER_UID" -g "$RUNNER_GID" /run/user/${RUNNER_UID}
USER runner
# A user session's runtime directory, as hosted runners have; systemd-analyze
# --user needs it.
ENV CARGO_HOME=/home/runner/.cargo RUSTUP_HOME=/home/runner/.rustup \
    XDG_RUNTIME_DIR=/run/user/${RUNNER_UID} PATH=/home/runner/.cargo/bin:$PATH \
    HOME=/home/runner USER=runner LOGNAME=runner
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain "$RUST_VERSION" --component rustfmt,clippy \
    && git config --global user.name local-ci \
    && git config --global user.email local-ci@example.invalid
WORKDIR /work
