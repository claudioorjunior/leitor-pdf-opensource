import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  checksumMatches,
  compareSemver,
  hostFrom,
  isNewer,
  isTranslocated,
  parseDigest,
  parseSha256File,
  planFromRelease,
  selectAsset,
  sha256Hex,
  versionFromInfoPlist,
  type GhRelease,
} from "./plan.ts";

const fixtures = join(dirname(fileURLToPath(import.meta.url)), "../fixtures/releases");

function load(name: string): GhRelease | null {
  const raw = JSON.parse(readFileSync(join(fixtures, name), "utf8")) as GhRelease & {
    message?: string;
  };
  if (!raw.tag_name || raw.message === "Not Found") return null;
  return raw;
}

const mac = hostFrom("darwin", "arm64");
const win = hostFrom("win32", "x64");

describe("host triple", () => {
  test("Mac arm64 e Windows x64", () => {
    expect(mac).toEqual({ ok: true, triple: "aarch64-apple-darwin", os: "macos" });
    expect(win).toEqual({ ok: true, triple: "x86_64-pc-windows-msvc", os: "windows" });
  });

  test("Mac Intel e Linux recusam", () => {
    const intel = planFromRelease(load("latest-macos.json"), hostFrom("darwin", "x64"));
    expect("error" in intel && intel.error.code).toBe("unsupported-host");
    const linux = planFromRelease(load("latest-macos.json"), hostFrom("linux", "x64"));
    expect("error" in linux && linux.error.code).toBe("unsupported-host");
  });
});

describe("release JSON → plano", () => {
  test("escolhe o DMG arm64 e os passos do hdiutil", () => {
    const result = planFromRelease(load("latest-macos.json"), mac);
    expect("plan" in result).toBe(true);
    if (!("plan" in result)) return;
    expect(result.plan.assetName).toBe("TsuroPDF-0.2.0-aarch64-apple-darwin.dmg");
    expect(result.plan.url).toContain("v0.2.0/TsuroPDF-0.2.0-aarch64-apple-darwin.dmg");
    expect(result.plan.digest).toBe("a".repeat(64));
    expect(result.plan.steps).toEqual([
      "hdiutil attach -nobrowse -readonly <dmg>",
      "ditto <TsuroPDF.app> /Applications/TsuroPDF.app",
      "hdiutil detach <mount>",
    ]);
  });

  test("Windows x64 escolhe o NSIS e o /S", () => {
    const result = planFromRelease(load("latest-macos.json"), win);
    expect("plan" in result).toBe(true);
    if (!("plan" in result)) return;
    expect(result.plan.assetName).toBe("TsuroPDF-0.2.0-x86_64-pc-windows-msvc-setup.exe");
    expect(result.plan.steps).toEqual(["<setup.exe> /S"]);
  });

  test("release vazia imprime o caminho de clone", () => {
    const result = planFromRelease(load("latest-empty.json"), mac);
    expect("error" in result && result.error.code).toBe("no-release");
    if (!("error" in result)) return;
    expect(result.error.message).toContain("git clone");
    expect(result.error.message).toContain("cargo build --release -p tsuro");
    expect(result.error.message).toContain("./scripts/bundle-macos.sh");
    expect(result.error.message).toContain("issues/5");
  });

  test("Windows sem exe diz que o NSIS não saiu", () => {
    const result = planFromRelease(load("latest-windows-missing.json"), win);
    expect("error" in result && result.error.code).toBe("windows-unpublished");
  });

  test("--version pin usa a tag e o sidecar .sha256", () => {
    const result = planFromRelease(load("tag-v0.1.0.json"), mac);
    expect("plan" in result).toBe(true);
    if (!("plan" in result)) return;
    expect(result.plan.tag).toBe("v0.1.0");
    expect(result.plan.digest).toBeUndefined();
    expect(result.plan.checksumUrl).toContain(".dmg.sha256");
  });

  test("selectAsset ignora nome sem o triple", () => {
    const release = load("latest-macos.json")!;
    expect(selectAsset(release, "aarch64-apple-darwin")?.name).toContain("aarch64-apple-darwin");
  });
});

describe("semver e checksum", () => {
  test("compara tags com ou sem v", () => {
    expect(compareSemver("v0.2.0", "0.1.0")).toBeGreaterThan(0);
    expect(compareSemver("0.1.0", "v0.1.0")).toBe(0);
    expect(isNewer("v0.2.0", "0.1.0")).toBe(true);
    expect(isNewer("v0.1.0", "0.2.0")).toBe(false);
    expect(isNewer("v0.2.0", null)).toBe(true);
  });

  test("digest GitHub e ficheiro SHA256", () => {
    expect(parseDigest("sha256:" + "ab".repeat(32))).toBe("ab".repeat(32));
    const name = "TsuroPDF-aarch64-apple-darwin.dmg";
    expect(parseSha256File(`${"cd".repeat(32)}  ${name}\n`, name)).toBe("cd".repeat(32));
    expect(parseSha256File(`SHA256 (${name}) = ${"ef".repeat(32)}\n`, name)).toBe("ef".repeat(32));
  });

  test("checksum bate e falha", () => {
    const bytes = new TextEncoder().encode("tsuro");
    const hex = sha256Hex(bytes);
    expect(checksumMatches(bytes, hex)).toBe(true);
    expect(checksumMatches(bytes, "0".repeat(64))).toBe(false);
  });
});

describe("Info.plist e translocação", () => {
  test("lê CFBundleShortVersionString", () => {
    const xml = `<?xml version="1.0"?>
    <dict><key>CFBundleShortVersionString</key><string>0.2.0</string></dict>`;
    expect(versionFromInfoPlist(xml)).toBe("0.2.0");
  });

  test("recusa caminho translocado", () => {
    expect(isTranslocated("/private/var/folders/xx/AppTranslocation/ABC/d/TsuroPDF.app")).toBe(true);
    expect(isTranslocated("/Volumes/TsuroPDF/TsuroPDF.app")).toBe(false);
  });
});
