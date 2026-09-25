"""Extract the pinned upstream union independently of the Rust method catalog."""

import argparse
import pathlib
import re
import subprocess

PIN = "2c8e8a826810337492cc5a38bb0bbd705b6fb632"
ROOT = pathlib.Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "crates/server-protocol/src/methods/fixtures/paseo-inbound.txt"


def extract(checkout):
    revision = subprocess.check_output(
        ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != PIN:
        raise ValueError(f"expected Paseo {PIN}, got {revision}")
    source = checkout / "packages/protocol/src"
    union = (source / "messages.ts").read_text().split(
        "export const SessionInboundMessageSchema", 1
    )[1].split("];", 1)[0].split("]);", 1)[0]
    refs = re.findall(r"^\s*(\w+Schema),?\s*$", union, re.M)
    definitions = {}
    for path in source.rglob("*.ts"):
        if ".test." in path.name:
            continue
        for declaration in re.split(r"\b(?:export\s+)?const\s+", path.read_text())[1:]:
            name = declaration.split("=", 1)[0].split(":", 1)[0].strip()
            literal = re.search(r'type:\s*z.literal\("([^"]+)"\)', declaration)
            helper = re.search(
                r'(?:pluginIdRequest|agentSkillsRequest)\(\s*"([^"]+)"', declaration
            )
            match = literal or helper
            if match:
                definitions[name] = match[1]
    names = [definitions[ref] for ref in refs]  # Fail on unrecognized schema constructors.
    if len(names) != 205 or len(set(names)) != len(names):
        raise ValueError("unexpected pinned union size or duplicate message types")
    return f"# Paseo {PIN}: SessionInboundMessageSchema\n" + "\n".join(sorted(names)) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=pathlib.Path)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    extracted = extract(args.checkout)
    if args.write:
        FIXTURE.parent.mkdir(parents=True, exist_ok=True)
        FIXTURE.write_text(extracted)
    elif FIXTURE.read_text() != extracted:
        raise SystemExit("upstream fixture differs; review before regenerating with --write")
    print("Verified 205 upstream inbound names from pinned Paseo source")


if __name__ == "__main__":
    main()
