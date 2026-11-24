"""The generated sync and async twins are what ``scripts/unasync.py`` makes of their sources now."""

from __future__ import annotations

import runpy
from pathlib import Path


def test_each_generated_twin_is_current() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts/unasync.py"))

    stale = [path.name for path, text in script["twins"]().items() if path.read_text() != text]

    assert stale == [], "run scripts/unasync.py"
