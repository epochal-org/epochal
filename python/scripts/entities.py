"""Writes the typed entity models of both SDKs from the entity schemas of the public API spec, so
neither can drift from the spec; ``tests`` fail when a model file is stale.

    python scripts/entities.py

``spec/entities.json`` holds the schemas the models are made from, and ``spec/examples/`` the
entity guide's example of each type, plus ``stock.json``, a type the public API no longer serves,
for the unknown-type tests. To copy them again from a checkout of the API docs (a directory
holding ``openapi.yaml`` and ``guides/``) after the spec changes, then regenerate:

    uv run --with pyyaml scripts/entities.py --refresh <docs>/content

The copy is refreshed by hand. The output is formatted by ruff (Python) and written already
in rustfmt's layout (Rust)."""

from __future__ import annotations

import json
import keyword
import re
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / "spec/entities.json"
EXAMPLES = ROOT / "spec/examples"
RUST = ROOT / "api/src/time_portal/entities.rs"
PYTHON = ROOT / "python/api/src/epochal/api/entities.py"
HEADER = (
    "Generated from spec/entities.json by python/scripts/entities.py; "
    "edit that script, then run it."
)

RUST_KEYWORDS = {
    "as",
    "async",
    "await",
    "box",
    "const",
    "crate",
    "dyn",
    "enum",
    "fn",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "type",
    "use",
    "where",
    "yield",
}


def _refs(node: Any) -> Iterator[str]:
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "$ref":
                yield value.rsplit("/", 1)[-1]
            else:
                yield from _refs(value)
    elif isinstance(node, list):
        for value in node:
            yield from _refs(value)


def refresh(content: Path) -> None:
    """Copies the public entity schemas and the guide's examples out of the API docs."""
    import yaml  # type: ignore[import-untyped]

    spec = yaml.safe_load((content / "openapi.yaml").read_text())
    schemas = spec["components"]["schemas"]
    union = schemas["Entity"]
    kept: dict[str, Any] = {"Entity": union}
    todo = list(_refs(union))
    while todo:
        name = todo.pop(0)
        if name not in kept:
            kept[name] = schemas[name]
            todo.extend(_refs(schemas[name]))
    SPEC.write_text(json.dumps({"schemas": kept}, indent=2, ensure_ascii=False) + "\n")

    guide = (content / "guides/time-portal/entities.mdx").read_text()
    EXAMPLES.mkdir(parents=True, exist_ok=True)
    for block in re.findall(r"```json \w+\n(.*?)\n```", guide, re.S):
        example = json.loads(block)
        path = EXAMPLES / f"{example['type']}.json"
        path.write_text(json.dumps(example, indent=2, ensure_ascii=False) + "\n")


class Field:
    """One property of an object schema, as both languages type and read it."""

    def __init__(self, name: str, schema: dict[str, Any], schemas: dict[str, Any]) -> None:
        self.name = name
        self.py_name = f"{name}_" if keyword.iskeyword(name) else name
        self.rust_name = f"r#{name}" if name in RUST_KEYWORDS else name
        self.doc = schema.get("description") or f"`{name}`."
        self.nullable = False
        if "anyOf" in schema:
            [inner] = [s for s in schema["anyOf"] if s != {"type": "null"}]
            self.nullable = len(schema["anyOf"]) == 2
            schema = inner
        kind = schema.get("type")
        if isinstance(kind, list):
            self.nullable = "null" in kind
            [kind] = [k for k in kind if k != "null"]
            schema = {**schema, "type": kind}
        self.list = kind == "array"
        self.item = _Item(schema["items"] if self.list else schema, schemas)

    @property
    def rust(self) -> str:
        rust = f"Vec<{self.item.rust}>" if self.list else self.item.rust
        return f"Option<{rust}>" if self.nullable else rust

    @property
    def python(self) -> str:
        python = f"list[{self.item.python}]" if self.list else self.item.python
        return f"{python} | None" if self.nullable else python

    def read(self) -> str:
        """The Python expression that reads this field out of ``j``."""
        parse, value = self.item.parse, f'j.get("{self.name}")'
        if self.list:
            if self.nullable:
                return f"_opt(lambda v: _list({parse}, v), {value})"
            return f'_list({parse}, j.get("{self.name}", []))'
        return f"_opt({parse}, {value})" if self.nullable else f'{parse}(j["{self.name}"])'


class _Item:
    def __init__(self, schema: dict[str, Any], schemas: dict[str, Any]) -> None:
        if "$ref" in schema:
            name = schema["$ref"].rsplit("/", 1)[-1]
            target = schemas[name]
            self.rust = self.python = name
            if "x-extensible-enum" in target:
                self.python, self.parse = f"{name} | str", f"{name}.read"
            elif target.get("type") == "object":
                self.parse = f"{name}.from_json"
            else:
                self.parse = "_str"
            return
        kind, fmt = schema["type"], schema.get("format")
        if kind == "string" and fmt == "date-time":
            self.rust, self.python, self.parse = (
                "DateTime<Utc>",
                "dt.datetime",
                "_wire.parse_instant",
            )
        elif kind == "string" and fmt == "date":
            self.rust, self.python, self.parse = "NaiveDate", "dt.date", "dt.date.fromisoformat"
        elif kind == "string":
            self.rust, self.python, self.parse = "String", "str", "_str"
        elif kind == "number":
            self.rust, self.python, self.parse = "f64", "float", "_float"
        elif kind == "integer":
            unsigned = schema.get("minimum", -1) >= 0
            self.rust = "u64" if unsigned else "i64"
            self.python, self.parse = "int", "_uint" if unsigned else "_int"
        elif kind == "boolean":
            self.rust, self.python, self.parse = "bool", "bool", "_bool"
        else:
            raise ValueError(f"no mapping for {schema}")


def _load() -> tuple[dict[str, Any], dict[str, tuple[str, int]]]:
    """The schemas, and each modeled ``type`` with its properties schema and the layout version
    (``schema_version``) that schema is, in the spec's order."""
    schemas: dict[str, Any] = json.loads(SPEC.read_text())["schemas"]
    types = {}
    for kind, ref in schemas["Entity"]["discriminator"]["mapping"].items():
        entity = schemas[ref.rsplit("/", 1)[-1]]["properties"]
        props = entity["properties"]["$ref"].rsplit("/", 1)[-1]
        types[kind] = props, _version(kind, entity["schema_version"]["description"])
    return schemas, types


def _version(kind: str, text: str) -> int:
    """The layout version ``schema_version``'s description names for ``kind``, such as "`1` for
    every type", or one named for ``kind`` alone ("`<n>` for `<kind>`")."""
    named = {k: int(v) for v, k in re.findall(r"`(\d+)` for `(\w+)`", text)}
    other = re.search(r"`(\d+)` for every (?:other )?type", text)
    if kind in named:
        return named[kind]
    if other:
        return int(other[1])
    raise ValueError(f"no layout version for {kind} in {text!r}")


def _models(schemas: dict[str, Any]) -> list[str]:
    """Every schema a model is made for, sorted by name: all but the entity envelopes."""
    return sorted(
        name
        for name in schemas
        if name != "Entity"
        and not (name.endswith("Entity") and "properties" in schemas[name].get("properties", {}))
    )


def _words(value: str) -> list[str]:
    return [w for w in re.split(r"[^0-9A-Za-z]+", value) if w]


def _rust_doc(text: str, indent: str) -> str:
    return "".join(
        f"{indent}///{' ' if line else ''}{line}\n" for line in text.strip().splitlines()
    )


def _variant(value: str) -> str:
    name = "".join(w[:1].upper() + w[1:].lower() for w in _words(value))
    assert name[:1].isalpha(), value
    return name


def rust() -> str:
    schemas, types = _load()
    out = [
        f"//! {HEADER}\n",
        "//!\n",
        "//! The typed layout of each public entity type's `properties`, from\n",
        "//! [`Entity::typed`](super::web::Entity::typed). Every enum is open: a value this\n",
        "//! version doesn't name reads as `Other`, keeping the value as sent.\n\n",
        "use chrono::{DateTime, NaiveDate, Utc};\n",
        "use serde::{Deserialize, Serialize};\n",
    ]
    for name in _models(schemas):
        schema = schemas[name]
        out.append("\n" + _rust_doc(schema.get("description") or f"`{name}`.", ""))
        if "x-extensible-enum" in schema:
            out.append("#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]\n")
            out.append("#[non_exhaustive]\n")
            out.append(f"pub enum {name} {{\n")
            variants = [_variant(v) for v in schema["x-extensible-enum"]]
            assert len(set(variants)) == len(variants), name
            for value, variant in zip(schema["x-extensible-enum"], variants, strict=True):
                out.append(
                    f'    /// `{value}`.\n    #[serde(rename = "{value}")]\n    {variant},\n'
                )
            out.append(
                "    /// A value this version doesn't name, as sent.\n    #[serde(untagged)]\n"
            )
            out.append("    Other(String),\n}\n")
        elif schema.get("type") == "object":
            out.append(
                "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\n#[non_exhaustive]\n"
            )
            out.append(f"pub struct {name} {{\n")
            for field, prop in schema["properties"].items():
                f = Field(field, prop, schemas)
                out.append(_rust_doc(f.doc, "    "))
                if f.list and not f.nullable:
                    out.append("    #[serde(default)]\n")
                out.append(f"    pub {f.rust_name}: {f.rust},\n")
            out.append("}\n")
        elif schema.get("type") == "string":
            out.append(f"pub type {name} = String;\n")
        else:
            raise ValueError(f"no mapping for {name}")

    out.append(
        "\n/// An entity's `properties`, read into the model its `type` names, from\n"
        "/// [`Entity::typed`](super::web::Entity::typed). It holds `properties` only: `id`,\n"
        "/// `kind`, `schema_version` and `updated_at` stay on the\n"
        "/// [`Entity`](super::web::Entity). Fields this version doesn't model are left out;\n"
        "/// `Entity::properties` keeps them.\n"
        "#[derive(Clone, Debug, PartialEq)]\n#[non_exhaustive]\npub enum TypedEntity {\n"
    )
    for kind, (props, version) in types.items():
        out.append(f"    /// `{kind}`, layout `{version}`: [`{props}`].\n")
        out.append(f"    {_variant(kind)}(Box<{props}>),\n")
    out.append(
        "    /// A `type`, or a `schema_version` of one, that this version doesn't model, with\n"
        "    /// its `properties` as sent.\n"
        "    Unknown {\n"
        "        /// The entity's `type`, as in [`Entity::kind`](super::web::Entity::kind).\n"
        "        kind: String,\n"
        "        /// The entity's `schema_version`. For a modeled `kind`, a layout this version\n"
        "        /// wasn't generated from, older or newer.\n"
        "        schema_version: u32,\n"
        "        /// Its `properties`, as sent.\n"
        "        properties: serde_json::Value,\n"
        "    },\n}\n\n"
        "impl TypedEntity {\n"
        "    pub(crate) fn read(\n"
        "        kind: &str,\n"
        "        schema_version: u32,\n"
        "        properties: &serde_json::Value,\n"
        "    ) -> Result<TypedEntity, serde_json::Error> {\n"
        "        Ok(match (kind, schema_version) {\n"
    )
    for kind, (_, version) in types.items():
        variant = _variant(kind)
        out.append(
            f'            ("{kind}", {version}) => TypedEntity::{variant}(boxed(properties)?),\n'
        )
    out.append(
        "            _ => TypedEntity::Unknown {\n"
        "                kind: kind.to_owned(),\n"
        "                schema_version,\n"
        "                properties: properties.clone(),\n"
        "            },\n"
        "        })\n    }\n}\n\n"
        "fn boxed<'de, T: Deserialize<'de>>(\n"
        "    properties: &'de serde_json::Value,\n"
        ") -> Result<Box<T>, serde_json::Error> {\n"
        "    T::deserialize(properties).map(Box::new)\n"
        "}\n"
    )
    return "".join(out)


def _py_doc(text: str, indent: str) -> str:
    text = text.strip().replace("\\", "\\\\").replace('"""', '\\"\\"\\"')
    lines = text.splitlines()
    body = "\n".join([lines[0], *(f"{indent}{line}" if line else "" for line in lines[1:])])
    return f'{indent}"""{body}"""\n'


def python() -> str:
    schemas, types = _load()
    out = [
        f'"""The typed layout of each public entity type\'s ``properties``, from\n'
        f":attr:`epochal.api.time_portal.Entity.typed`. Every enum is open: a value this version\n"
        f'doesn\'t name reads as the ``str`` sent.\n\n{HEADER}\n"""\n\n',
        "from __future__ import annotations\n\n",
        "import datetime as dt\nimport enum\nimport math\nfrom collections.abc import Callable\n",
        "from dataclasses import dataclass\nfrom typing import Any, Self, TypeAlias, TypeVar\n\n",
        "from epochal.api import _wire\nfrom epochal.api._errors import DecodeError\n\n",
        '_T = TypeVar("_T")\n\n\n',
        "def _opt(parse: Callable[[Any], _T], value: Any) -> _T | None:\n",
        "    return None if value is None else parse(value)\n\n\n",
        "def _list(parse: Callable[[Any], _T], values: Any) -> list[_T]:\n",
        "    if not isinstance(values, list):\n",
        '        raise TypeError(f"expected a list, got {values!r}")\n',
        "    return [parse(v) for v in values]\n\n\n",
        "def _str(value: Any) -> str:\n",
        "    if not isinstance(value, str):\n",
        '        raise TypeError(f"expected a string, got {value!r}")\n',
        "    return value\n\n\n",
        "def _int(value: Any) -> int:\n",
        "    if isinstance(value, bool) or not isinstance(value, int):\n",
        '        raise TypeError(f"expected an integer, got {value!r}")\n',
        "    return value\n\n\n",
        "def _uint(value: Any) -> int:\n",
        "    if not 0 <= _int(value) < 2**64:\n",
        '        raise ValueError(f"expected an integer in u64 range, got {value!r}")\n',
        "    return int(value)\n\n\n",
        "def _float(value: Any) -> float:\n",
        "    if isinstance(value, bool) or not isinstance(value, int | float):\n",
        '        raise TypeError(f"expected a number, got {value!r}")\n',
        "    if not math.isfinite(value):\n",
        '        raise ValueError(f"expected a finite number, got {value!r}")\n',
        "    return float(value)\n\n\n",
        "def _bool(value: Any) -> bool:\n",
        "    if not isinstance(value, bool):\n",
        '        raise TypeError(f"expected a boolean, got {value!r}")\n',
        "    return value\n\n\n",
        "class _Open(enum.StrEnum):\n",
        '    """An enum whose values a newer server may add; those read as the ``str`` sent."""\n',
        "\n    @classmethod\n    def read(cls, value: Any) -> Self | str:\n",
        "        text = _str(value)\n",
        "        try:\n            return cls(text)\n",
        "        except ValueError:\n            return text\n",
    ]
    for name in _models(schemas):
        schema = schemas[name]
        doc = schema.get("description") or f"``{name}``."
        if "x-extensible-enum" in schema:
            out.append(f"\n\nclass {name}(_Open):\n{_py_doc(doc, '    ')}\n")
            for value in schema["x-extensible-enum"]:
                member = "_".join(w.upper() for w in _words(value))
                assert member[:1].isalpha(), value
                out.append(f'    {member} = "{value}"\n')
        elif schema.get("type") == "object":
            fields = [Field(f, p, schemas) for f, p in schema["properties"].items()]
            out.append(
                f"\n\n@dataclass(frozen=True, slots=True)\nclass {name}:\n{_py_doc(doc, '    ')}\n"
            )
            for f in fields:
                out.append(f"    {f.py_name}: {f.python}\n{_py_doc(f.doc, '    ')}")
            out.append("\n    @classmethod\n")
            out.append(
                f"    def from_json(cls, j: dict[str, Any]) -> {name}:\n        return cls(\n"
            )
            for f in fields:
                out.append(f"            {f.py_name}={f.read()},\n")
            out.append("        )\n")
        elif schema.get("type") == "string":
            out.append(f"\n\n{name}: TypeAlias = str\n{_py_doc(doc, '')}")
        else:
            raise ValueError(f"no mapping for {name}")

    out.append(
        "\n\n@dataclass(frozen=True, slots=True)\nclass UnknownProperties:\n"
        '    """The ``properties`` of a ``type``, or a ``schema_version`` of one, that this\n'
        "    version doesn't model, as sent. ``type`` and ``schema_version`` are the entity's; a\n"
        "    modeled ``type`` with another ``schema_version`` is a layout this version wasn't\n"
        '    generated from, older or newer."""\n\n'
        "    type: str\n    schema_version: int\n    properties: dict[str, Any]\n"
    )
    union = " | ".join([*(props for props, _ in types.values()), "UnknownProperties"])
    out.append(f"\n\nTypedEntity: TypeAlias = {union}\n")
    out.append(
        '"""An entity\'s ``properties``, read into the class its ``type`` names. It holds\n'
        "``properties`` only: ``id``, ``type``, ``schema_version`` and ``updated_at`` stay on the\n"
        ":class:`~epochal.api.time_portal.Entity`. Fields this version doesn't model are left\n"
        'out; ``Entity.properties`` keeps them."""\n'
    )
    out.append("\n_READERS: dict[tuple[str, int], Callable[[dict[str, Any]], TypedEntity]] = {\n")
    for kind, (props, version) in types.items():
        out.append(f'    ("{kind}", {version}): {props}.from_json,\n')
    out.append(
        "}\n\n\n"
        "def typed(kind: str, schema_version: int, properties: dict[str, Any]) -> TypedEntity:\n"
        '    """``properties`` read into the class of ``kind`` at layout ``schema_version``;\n'
        "    :class:`UnknownProperties` for a kind or layout this version doesn't model. Raises\n"
        '    :class:`~epochal.api.DecodeError` when they don\'t match the documented layout."""\n'
        "    read = _READERS.get((kind, schema_version))\n"
        "    if read is None:\n"
        "        return UnknownProperties(kind, schema_version, properties)\n"
        "    try:\n"
        "        return read(properties)\n"
        "    except (KeyError, TypeError, ValueError, OverflowError, AttributeError) as e:\n"
        '        raise DecodeError(f"unreadable {kind} properties: {e!r}") from e\n'
    )
    return "".join(out)


def _ruff(path: Path, source: str) -> str:
    for args in (["check", "--select", "I", "--fix", "--quiet"], ["format", "--quiet"]):
        source = subprocess.run(
            [sys.executable, "-m", "ruff", *args, "--stdin-filename", str(path), "-"],
            input=source,
            capture_output=True,
            text=True,
            check=True,
            cwd=ROOT / "python",
        ).stdout
    return source


def models() -> dict[Path, str]:
    """Each generated file and what it should hold."""
    return {RUST: rust(), PYTHON: _ruff(PYTHON, python())}


if __name__ == "__main__":
    if sys.argv[1:2] == ["--refresh"]:
        refresh(Path(sys.argv[2]))
    for path, text in models().items():
        path.write_text(text)
