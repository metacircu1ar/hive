"""Explicit checkout selection and release-binary admission."""

import json
import os
from pathlib import Path
from release.process import run

ROOT = Path(__file__).resolve().parents[1]


def product_root():
    root = Path(os.environ.get("HIVE_ROOT", ROOT)).expanduser().resolve()
    if not (root / "Cargo.toml").is_file() or not (root / "src/runtime").is_dir():
        raise ValueError("HIVE_ROOT must select a Hive checkout")
    return root


def product_default(relative):
    return product_root() / relative


def require_paths(args, parser, *fields, product="Hive"):
    for field in fields:
        path = getattr(args, field)
        if path is None:
            parser.error(f"Pass --{field.replace('_', '-')} with a release binary")
        path = Path(path).expanduser().resolve()
        if field == "embedding_config":
            if not path.is_file():
                parser.error("Embedding configuration must be an existing file")
            setattr(args, field, path)
            continue
        if path.parent.name != "release" or not path.is_file():
            package = "comb" if product == "Comb" else "hive"
            parser.error(
                f"Build the release binary first: cargo build --release --locked -p {package}"
            )
        result = run([str(path), "version", "--json"], timeout=10, text=True, check=True)
        value = json.loads(result.stdout)
        expected = product
        if not isinstance(value, dict) or value.get("product") != expected:
            parser.error("Scenario binary reported a different product identity")
        if expected == "Hive" and not path.with_name("hive-daemon").is_file():
            parser.error("The admitted Hive release needs its hive-daemon sibling")
        setattr(args, field, path)


def output_directory(label):
    import uuid

    return product_root() / "dist/e2e" / f"{label}-{uuid.uuid4().hex}"
