import json
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
FIXTURES_ROOT = JUDGMENT_ROOT / "fixtures"


def materialize_fixture(name: str, destination: Path) -> Path:
    source = FIXTURES_ROOT / name
    root = destination / name
    root.mkdir()
    for path in source.rglob("*"):
        relative = path.relative_to(source)
        if relative.parts[0] == "ai-docs":
            relative = Path(".ai-docs", *relative.parts[1:])
        target = root / relative
        if path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        content = path.read_text(encoding="utf-8").replace("\r\n", "\n").replace("\r", "\n")
        if relative.name == "codegraph.json":
            artifact = json.loads(content)
            if artifact.get("root") != "<ROOT>":
                raise AssertionError("fixture codegraph root must use <ROOT>")
            artifact["root"] = str(root.resolve())
            content = json.dumps(artifact, indent=2)
        target.write_text(content, encoding="utf-8", newline="\n")
    return root
