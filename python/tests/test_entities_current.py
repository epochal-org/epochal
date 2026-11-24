"""The generated entity models are what ``scripts/entities.py`` makes of ``spec/entities.json``."""

from __future__ import annotations

import runpy
from pathlib import Path


def test_each_generated_model_file_is_current() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts/entities.py"))

    stale = [path.name for path, text in script["models"]().items() if path.read_text() != text]

    assert stale == [], "run scripts/entities.py"
