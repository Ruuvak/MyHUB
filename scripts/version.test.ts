import { expect, test } from "bun:test";
import { checkVersions, cargoVersion, withCargoVersion } from "./version";

const toml = '[workspace]\nmembers = ["a"]\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n';

test("reads and rewrites the workspace version", () => {
  expect(cargoVersion(toml)).toBe("0.1.0");
  expect(cargoVersion(withCargoVersion(toml, "1.2.3"))).toBe("1.2.3");
});

test("check reports mismatches", () => {
  expect(checkVersions("0.1.0", "0.1.0", "v0.1.0")).toEqual([]);
  expect(checkVersions("0.1.0", "0.2.0")).toHaveLength(1);
  expect(checkVersions("0.1.0", "0.1.0", "v0.1.1")).toHaveLength(1);
  expect(checkVersions("x", "x")).toHaveLength(1);
});
