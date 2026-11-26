"""Package the Decky plugin into out/AudioBridge-<version>.zip.

The archive holds one folder, AudioBridge/, laid out the way Decky Loader
installs plugins. Entries are sorted and carry a fixed timestamp so the same
sources always give the same bytes.
"""

import hashlib
import json
import os
import re
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FOLDER = "AudioBridge"
STAMP = (2026, 1, 1, 0, 0, 0)
FILES = ["main.py", "plugin.json", "package.json", "LICENSE", "README.md", "dist/index.js",
         "THIRD_PARTY_NOTICES.md", "licenses/LGPL-2.1.txt"]


def collect():
    paths = list(FILES)
    modules = os.path.join(ROOT, "py_modules")
    for base, dirs, files in os.walk(modules):
        dirs[:] = sorted(d for d in dirs if d != "__pycache__")
        for name in sorted(files):
            if name.endswith(".py"):
                full = os.path.join(base, name)
                paths.append(os.path.relpath(full, ROOT).replace(os.sep, "/"))
    for rel in paths:
        full = os.path.join(ROOT, rel)
        if not os.path.isfile(full) or os.path.islink(full):
            sys.exit("missing or unsafe file: " + rel)
    return sorted(paths)


def check_versions():
    with open(os.path.join(ROOT, "package.json"), encoding="utf-8") as handle:
        version = json.load(handle)["version"]
    with open(os.path.join(ROOT, "py_modules", "audiobridge", "__init__.py"), encoding="utf-8") as handle:
        match = re.search(r'VERSION = "([^"]+)"', handle.read())
    if not match or match.group(1) != version:
        sys.exit("package.json and audiobridge.VERSION disagree")
    return version


def main():
    version = check_versions()
    out_dir = os.path.join(ROOT, "out")
    os.makedirs(out_dir, exist_ok=True)
    target = os.path.join(out_dir, "%s-%s.zip" % (FOLDER, version))
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for rel in collect():
            info = zipfile.ZipInfo("%s/%s" % (FOLDER, rel), STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            with open(os.path.join(ROOT, rel), "rb") as handle:
                archive.writestr(info, handle.read())
    digest = hashlib.sha256(open(target, "rb").read()).hexdigest()
    with open(target + ".sha256", "w", encoding="ascii") as handle:
        handle.write("%s  %s\n" % (digest, os.path.basename(target)))
    print("%s\n%s" % (target, digest))


if __name__ == "__main__":
    main()
