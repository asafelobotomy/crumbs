// What the AUR packages say about the page. scripts/tests/uq_files.sh and scripts/tests/aur.sh
// check what they really install.
//   node --test 'tests/uq/*.test.mjs'
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

// ---- packaging -------------------------------------------------------------------------------

test("the packages install the whole page folder and name uQ once", () => {
  for (const pkg of ["crum-bs", "crum-bs-bin"]) {
    const pkgbuild = readFileSync(join(repo, "packaging", "aur", pkg, "PKGBUILD"), "utf8");
    assert.ok(pkgbuild.includes('install -Dm644 -t "$pkgdir/usr/share/uq/settings/pages/crumbs" uq/pages/crumbs/*'), `${pkg}: no page install line`);
    assert.equal((pkgbuild.match(/^optdepends=.*$/gm) ?? []).length, 1, `${pkg}: one optdepends line`);
    assert.match(pkgbuild, /^optdepends=\('uq: the Storage and cleanup page in Settings and an "Uninstall…" entry in the app menus'\)$/m);
  }
  for (const dir of ["deb", "rpm", "appimage"]) {
    for (const f of readdirSync(join(repo, "packaging", dir), { recursive: true })) {
      const p = join(repo, "packaging", dir, f);
      try {
        assert.ok(!readFileSync(p, "utf8").includes("uq/pages"), `packaging/${dir}/${f} ships the page`);
      } catch (e) {
        if (e.code !== "EISDIR" && e.code !== "ERR_INVALID_ARG_VALUE") throw e;
      }
    }
  }
});
