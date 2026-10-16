// Keeps the app version in one place: package.json is the source, root Cargo.toml must match.
//   bun scripts/version.ts check [vX.Y.Z]   exit 1 when package.json, Cargo.toml or the tag differ
//   bun scripts/version.ts set X.Y.Z        writes package.json and root Cargo.toml
import { readFileSync, writeFileSync } from "node:fs";

const root = new URL("../", import.meta.url);
const packagePath = new URL("package.json", root);
const cargoPath = new URL("Cargo.toml", root);

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const WORKSPACE_VERSION = /(\[workspace\.package\][^[]*?\nversion\s*=\s*")([^"]+)(")/;

export function cargoVersion(toml: string): string | null {
  return WORKSPACE_VERSION.exec(toml)?.[2] ?? null;
}

export function withCargoVersion(toml: string, version: string): string {
  if (!WORKSPACE_VERSION.test(toml)) throw new Error("no version in [workspace.package] of Cargo.toml");
  return toml.replace(WORKSPACE_VERSION, `$1${version}$3`);
}

/** Problems found; empty when all versions agree. A tag is "vX.Y.Z" or "X.Y.Z". */
export function checkVersions(pkg: string, cargo: string | null, tag?: string): string[] {
  const problems: string[] = [];
  if (!SEMVER.test(pkg)) problems.push(`package.json version "${pkg}" is not SemVer`);
  if (cargo === null) problems.push("Cargo.toml has no workspace.package version");
  else if (cargo !== pkg) problems.push(`Cargo.toml version ${cargo} differs from package.json ${pkg}`);
  if (tag !== undefined) {
    const tagVersion = tag.replace(/^v/, "");
    if (tagVersion !== pkg) problems.push(`tag ${tag} differs from package.json ${pkg}`);
  }
  return problems;
}

function main(args: string[]): number {
  const [command, value] = args;
  const pkgText = readFileSync(packagePath, "utf8");
  const pkg = JSON.parse(pkgText) as { version: string };
  if (command === "check") {
    const problems = checkVersions(pkg.version, cargoVersion(readFileSync(cargoPath, "utf8")), value);
    for (const p of problems) console.error(p);
    if (problems.length === 0) console.log(`version ${pkg.version} ok`);
    return problems.length === 0 ? 0 : 1;
  }
  if (command === "set" && value !== undefined) {
    if (!SEMVER.test(value)) {
      console.error(`"${value}" is not SemVer (X.Y.Z)`);
      return 1;
    }
    const next = pkgText.replace(/("version"\s*:\s*")[^"]+(")/, `$1${value}$2`);
    writeFileSync(packagePath, next);
    writeFileSync(cargoPath, withCargoVersion(readFileSync(cargoPath, "utf8"), value));
    console.log(`version set to ${value}. Run cargo check to refresh Cargo.lock.`);
    return 0;
  }
  console.error("usage: bun scripts/version.ts check [vX.Y.Z] | set X.Y.Z");
  return 1;
}

if (import.meta.main) process.exit(main(process.argv.slice(2)));
