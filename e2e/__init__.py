"""Explicit opt-in scenarios; ordinary product tests never import this package."""

import sys
from pathlib import Path

# All suite modules use the existing product helpers under their canonical identity.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
