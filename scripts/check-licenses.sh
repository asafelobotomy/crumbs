#!/bin/sh
# Lists every dependency whose licence is not satisfied by the allowed
# permissive licences (all compatible with GPL-3.0-or-later), and exits 1 if
# there is any. An SPDX expression passes when one OR alternative passes and
# every AND term in it is allowed.
set -eu
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --locked | python3 -c '
import json, re, sys
ALLOWED = {"MIT", "MIT-0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "ISC",
           "Unicode-3.0", "Zlib", "Unlicense", "GPL-3.0-or-later"}

def allowed(term):
    return term in ALLOWED or term.startswith("BSD-")

def parse(tokens):
    # or := and ("OR" and)* ; and := atom ("AND" atom)* ; atom := id | "(" or ")"
    def atom():
        t = tokens.pop(0)
        if t == "(":
            v = expr()
            tokens.pop(0)
            return v
        if tokens[:1] == ["WITH"]:
            tokens.pop(0)
            t = t + " WITH " + tokens.pop(0)
        return allowed(t)
    def conj():
        v = atom()
        while tokens[:1] == ["AND"]:
            tokens.pop(0)
            v = atom() and v
        return v
    def expr():
        v = conj()
        while tokens[:1] == ["OR"]:
            tokens.pop(0)
            v = conj() or v
        return v
    return expr()

def ok(lic):
    lic = lic.replace("/", " OR ")
    try:
        return parse(re.findall(r"[()]|[^\s()]+", lic))
    except IndexError:
        return False

bad = 0
for p in json.load(sys.stdin)["packages"]:
    lic = p.get("license")
    if not lic or not ok(lic):
        print("%s %s: %s" % (p["name"], p["version"], lic or "no licence field"))
        bad = 1
sys.exit(bad)
'
