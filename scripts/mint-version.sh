#!/usr/bin/env bash
# Mint the release version across the files that carry it.
#
#   ./scripts/mint-version.sh --next   print the next version and exit
#   ./scripts/mint-version.sh 0.2.0    write 0.2.0 into the tree
#
# `--next` takes the greater of the workspace manifest and the latest `v*`
# tag, and bumps its patch when the manifest is not ahead: ordinary runs
# only ever move the patch, while a manually bumped minor or major in
# Cargo.toml ships as-is.
#
# The version lives in three places: the workspace manifest (every member
# inherits it), the lockfile's member entries (so `--locked` builds agree),
# and the AppStream releases block. The GUI reports it as its own version
# and the self-updater compares it against release tags, so a tag `vX.Y.Z`
# must always ship binaries built from a tree that says `X.Y.Z` --
# release.yml mints before building and tags only after the builds pass.
#
# Minting is a pure function of the version, never committed by the
# pipeline: branch protection takes the five checks on every `main` commit,
# and a bot-pushed bump would have none. Needs python3 for the edits and
# git for `--next`.
set -eu

die() { echo "mint-version: $*" >&2; exit 2; }

[ $# -eq 1 ] || die "usage: $0 --next | X.Y.Z"

if [ "$1" = "--next" ]; then
    python3 - "$PWD" <<'EOF'
import re, subprocess, sys, tomllib
root = sys.argv[1]
manifest = tomllib.load(open(f"{root}/Cargo.toml", "rb"))["workspace"]["package"]["version"]
tags = subprocess.run(
    ["git", "tag", "-l", "v*"], capture_output=True, text=True, cwd=root
).stdout.split()

def parse(tag):
    m = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", tag)
    return tuple(int(x) for x in m.groups()) if m else None

latest = max([p for p in (parse(t) for t in tags) if p is not None], default=None)
current = tuple(int(x) for x in manifest.split("."))
if latest is None or current > latest:
    print(manifest)
else:
    print(f"{latest[0]}.{latest[1]}.{latest[2] + 1}")
EOF
    exit 0
fi

VERSION="$1"
printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
    || die "not a version: $VERSION (need X.Y.Z)"
export VERSION

python3 - "$PWD" <<'EOF'
import datetime, os, re, sys, tomllib
root, version = sys.argv[1], os.environ["VERSION"]

# 1. The workspace manifest: the one literal version line. Every member
# inherits it (`version.workspace = true`), so this is the only manifest
# edit; a member with a literal version would fail the lock check below.
cargo = f"{root}/Cargo.toml"
text = open(cargo).read()
text, n = re.subn(r'(?m)^version = "\d+\.\d+\.\d+"$', f'version = "{version}"', text)
assert n == 1, f"expected one version line in Cargo.toml, found {n}"
open(cargo, "w").write(text)

# 2. The lockfile: the workspace members' entries only. Line-based, so the
# rest of the lockfile is byte-identical; a `name` resets at each package
# block, and only a block belonging to a member is rewritten.
members = [m.split("/")[-1] for m in tomllib.load(open(cargo, "rb"))["workspace"]["members"]]
lock = f"{root}/Cargo.lock"
out, name, changed = [], None, 0
for line in open(lock):
    if line.startswith("[[package]]"):
        name = None
    m = re.match(r'name = "(.*)"', line)
    if m:
        name = m.group(1)
    elif name in members and re.fullmatch(r'version = "\d+\.\d+\.\d+"\n?', line):
        line = f'version = "{version}"\n'
        changed += 1
    out.append(line)
assert changed == len(members), f"expected {len(members)} member entries, rewrote {changed}"
open(lock, "w").write("".join(out))

# 3. The AppStream releases block: newest first, history kept. Re-minting
# the same version (every build job mints) is a no-op.
appdata = f"{root}/data/org.sequ.timetrack.appdata.xml"
text = open(appdata).read()
if f'version="{version}"' not in text:
    anchor = "  <releases>\n"
    assert anchor in text, "no <releases> block in the appdata"
    today = datetime.date.today().isoformat()
    text = text.replace(anchor, f'{anchor}    <release version="{version}" date="{today}"/>\n', 1)
    open(appdata, "w").write(text)
EOF
