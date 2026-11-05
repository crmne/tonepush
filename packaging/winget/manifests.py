#!/usr/bin/env python3
"""Write the winget manifests for a published TonePush release.

    python3 packaging/winget/manifests.py v1.2.3 OUTPUT_DIR

Fills the templates beside this script with the version, the release date and
the MSI hashes from the release's checksums.txt, and the first paragraph of
its notes (packaging/release-notes/<tag>.md) as ReleaseNotes, and writes them to
OUTPUT_DIR/manifests/c/crmne/TonePush/1.2.3, the layout of microsoft/winget-pkgs.
Nothing is submitted: see PACKAGING.md. --checksums and --release-date use local
values instead of asking GitHub, for a release that is not published yet.
"""

import argparse
import json
import re
import sys
import urllib.request
from pathlib import Path

REPOSITORY = "crmne/tonepush"
HERE = Path(__file__).resolve().parent
TEMPLATES = sorted(HERE.glob("*.yaml.in"))
INSTALLERS = {
    "X64_SHA256": "tonepush-{tag}-x86_64-pc-windows-msvc.msi",
    "ARM64_SHA256": "tonepush-{tag}-aarch64-pc-windows-msvc.msi",
}


def fetch(url):
    with urllib.request.urlopen(url, timeout=60) as response:
        return response.read().decode()


def checksums(text):
    result = {}
    for line in text.splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64})\s+\*?(\S+)\s*", line)
        if match:
            result[match.group(2)] = match.group(1).upper()
    return result


def summary(notes):
    """The notes' first paragraph as plain text: winget shows ReleaseNotes
    as it is, so Markdown links, emphasis and code marks go."""
    paragraph = notes.strip().split("\n\n")[0]
    text = " ".join(paragraph.split())
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"(\*\*|__|`)", "", text)
    if not text:
        sys.exit("the release notes have no first paragraph")
    return text


def render(template, values):
    text = template.read_text()
    for key, value in values.items():
        text = text.replace(f"@{key}@", value)
    leftover = re.findall(r"@[A-Z0-9_]+@", text)
    if leftover:
        sys.exit(f"{template.name}: no value for {', '.join(sorted(set(leftover)))}")
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("tag", help="a stable release tag, such as v1.2.3")
    parser.add_argument("output", type=Path)
    parser.add_argument("--checksums", type=Path, help="a local checksums.txt")
    parser.add_argument("--release-date", help="YYYY-MM-DD")
    args = parser.parse_args()

    if not re.fullmatch(r"v\d+\.\d+\.\d+", args.tag):
        sys.exit("winget takes stable releases only: give a tag such as v1.2.3")
    version = args.tag[1:]
    base = f"https://github.com/{REPOSITORY}/releases/download/{args.tag}"

    sums = checksums(
        args.checksums.read_text() if args.checksums else fetch(f"{base}/checksums.txt")
    )
    values = {"VERSION": version}
    for key, name in INSTALLERS.items():
        filename = name.format(tag=args.tag)
        if filename not in sums:
            sys.exit(f"checksums.txt has no {filename}")
        values[key] = sums[filename]

    date = args.release_date
    if not date:
        release = json.loads(
            fetch(f"https://api.github.com/repos/{REPOSITORY}/releases/tags/{args.tag}")
        )
        date = release["published_at"][:10]
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
        sys.exit(f"invalid release date: {date}")
    values["RELEASE_DATE"] = date

    notes = HERE.parent / "release-notes" / f"{args.tag}.md"
    if not notes.is_file():
        sys.exit(f"no release notes at {notes}")
    # A plain YAML scalar would misread a colon or a leading symbol: quote it.
    values["RELEASE_NOTES"] = json.dumps(summary(notes.read_text()), ensure_ascii=False)

    destination = args.output / "manifests/c/crmne/TonePush" / version
    destination.mkdir(parents=True, exist_ok=True)
    for template in TEMPLATES:
        path = destination / template.name.removesuffix(".in")
        path.write_text(render(template, values))
        print(path)


if __name__ == "__main__":
    main()
