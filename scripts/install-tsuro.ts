#!/usr/bin/env bun
/**
 * Instalador provisório do Tsuro (Bun). Não entra no .app.
 *
 * Flags (inglês): --check  --install  --version v0.2.0
 * Mensagens: português.
 *
 * Windows, quando o NSIS existir: `setup.exe /S` (CurrentUser).
 */
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  API_BASE,
  checksumMatches,
  hostFrom,
  isNewer,
  isTranslocated,
  MAC_APP_DEST,
  parseSha256File,
  planFromRelease,
  versionFromInfoPlist,
  type GhRelease,
  type InstallPlan,
} from "./install-tsuro/plan.ts";

type Args = {
  check: boolean;
  install: boolean;
  version?: string;
};

function parseArgs(argv: string[]): Args | { error: string } {
  const args: Args = { check: false, install: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!;
    if (a === "--check") args.check = true;
    else if (a === "--install") args.install = true;
    else if (a === "--version" || a.startsWith("--version=")) {
      const value = a.includes("=") ? a.slice("--version=".length) : argv[++i];
      if (!value) return { error: "Passe uma tag em --version, por exemplo --version v0.2.0." };
      args.version = value;
    } else if (a === "--help" || a === "-h") {
      return { error: help() };
    } else {
      return { error: `Flag desconhecida: ${a}\n${help()}` };
    }
  }
  if (args.check && args.install) {
    return { error: "Use --check ou --install, não os dois." };
  }
  return args;
}

function help(): string {
  return [
    "Uso: bun run install:tsuro -- [--check | --install] [--version vX.Y.Z]",
    "  (padrão)   baixa o artefato, confere SHA-256 se houver, não instala",
    "  --check    só diz se a release é mais nova que a instalação",
    "  --install  copia para /Applications (Mac) ou corre o NSIS /S (Windows)",
    "  --version  pin a uma tag em vez de /releases/latest",
  ].join("\n");
}

async function githubJson(url: string): Promise<unknown> {
  const res = await fetch(url, {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "tsuro-install",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (res.status === 404) return null;
  if (!res.ok) {
    throw new Error(`GitHub API ${res.status} em ${url}`);
  }
  return res.json();
}

function asRelease(raw: unknown): GhRelease | null {
  if (!raw || typeof raw !== "object") return null;
  const obj = raw as { tag_name?: unknown; assets?: unknown; message?: unknown };
  if (typeof obj.message === "string" && obj.message.toLowerCase().includes("not found")) {
    return null;
  }
  if (typeof obj.tag_name !== "string") return null;
  if (!Array.isArray(obj.assets)) return { tag_name: obj.tag_name, assets: [] };
  return obj as GhRelease;
}

async function fetchRelease(version?: string): Promise<GhRelease | null> {
  const url = version
    ? `${API_BASE}/releases/tags/${encodeURIComponent(version)}`
    : `${API_BASE}/releases/latest`;
  return asRelease(await githubJson(url));
}

async function installedVersion(os: "macos" | "windows"): Promise<string | null> {
  if (os === "macos") {
    try {
      const xml = await readFile(join(MAC_APP_DEST, "Contents/Info.plist"), "utf8");
      return versionFromInfoPlist(xml);
    } catch {
      return null;
    }
  }
  return null;
}

async function download(url: string, dest: string): Promise<Uint8Array> {
  const res = await fetch(url, { headers: { "User-Agent": "tsuro-install" }, redirect: "follow" });
  if (!res.ok || !res.body) {
    throw new Error(`Falha ao baixar (${res.status}): ${url}`);
  }
  const total = Number(res.headers.get("content-length") ?? 0);
  const chunks: Uint8Array[] = [];
  let got = 0;
  const reader = res.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (value) {
      chunks.push(value);
      got += value.byteLength;
      const mb = (got / (1024 * 1024)).toFixed(1);
      const of = total ? ` / ${(total / (1024 * 1024)).toFixed(1)}` : "";
      process.stderr.write(`\rBaixando ${mb}${of} MB`);
    }
  }
  process.stderr.write("\n");
  const bytes = new Uint8Array(got);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  await writeFile(dest, bytes);
  return bytes;
}

async function resolveDigest(plan: InstallPlan, bytes: Uint8Array): Promise<void> {
  let expected = plan.digest;
  if (!expected && plan.checksumUrl) {
    const res = await fetch(plan.checksumUrl, { headers: { "User-Agent": "tsuro-install" } });
    if (!res.ok) {
      throw new Error(`Não foi possível ler o SHA-256 (${res.status}).`);
    }
    expected = parseSha256File(await res.text(), plan.assetName);
  }
  if (!expected) {
    console.log("Release sem digest; o arquivo não foi conferido.");
    return;
  }
  if (!checksumMatches(bytes, expected)) {
    throw new Error("SHA-256 não confere. O arquivo baixado não será instalado.");
  }
  console.log("SHA-256 ok.");
}

async function run(cmd: string[], cwd?: string): Promise<void> {
  const proc = Bun.spawn(cmd, { cwd, stdout: "inherit", stderr: "inherit" });
  const code = await proc.exited;
  if (code !== 0) {
    throw new Error(`${cmd.join(" ")} saiu com ${code}`);
  }
}

async function findApp(root: string): Promise<string> {
  const glob = new Bun.Glob("**/Tsuro.app");
  for await (const match of glob.scan({ cwd: root, onlyFiles: false })) {
    const path = join(root, match);
    if (isTranslocated(path)) {
      throw new Error(
        "Recusa instalar a partir de um caminho translocado (AppTranslocation). Monte o DMG com hdiutil, não a partir de Downloads isolado.",
      );
    }
    return path;
  }
  throw new Error("O DMG não contém Tsuro.app.");
}

async function installMac(dmg: string): Promise<void> {
  const mount = await mkdtemp(join(tmpdir(), "tsuro-dmg-"));
  try {
    await run(["hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", mount, dmg]);
    const app = await findApp(mount);
    await rm(MAC_APP_DEST, { recursive: true, force: true });
    await run(["ditto", app, MAC_APP_DEST]);
    console.log(`Instalado em ${MAC_APP_DEST}`);
  } finally {
    await run(["hdiutil", "detach", mount]).catch(() => undefined);
    await rm(mount, { recursive: true, force: true }).catch(() => undefined);
  }
}

async function installWindows(exe: string): Promise<void> {
  await run([exe, "/S"]);
  console.log("Instalador NSIS (/S) concluído.");
}

async function main() {
  const parsed = parseArgs(process.argv.slice(2));
  if ("error" in parsed) {
    console.error(parsed.error);
    process.exit(parsed.error.startsWith("Uso:") ? 0 : 1);
  }
  const host = hostFrom(process.platform, process.arch);
  let release: GhRelease | null;
  try {
    release = await fetchRelease(parsed.version);
  } catch (err) {
    console.error(err instanceof Error ? err.message : String(err));
    process.exit(1);
    return;
  }
  const planned = planFromRelease(release, host);
  if ("error" in planned) {
    console.error(planned.error.message);
    process.exit(1);
  }
  const { plan } = planned;

  if (parsed.check) {
    const local = host.ok ? await installedVersion(host.os) : null;
    const newer = isNewer(plan.tag, local);
    console.log(`Instalado: ${local ?? "nenhum"}`);
    console.log(`Release: ${plan.tag}`);
    console.log(`Há versão nova: ${newer ? "sim" : "não"}`);
    return;
  }

  console.log(`Release ${plan.tag} → ${plan.assetName}`);
  const dir = await mkdtemp(join(tmpdir(), "tsuro-install-"));
  const dest = join(dir, plan.assetName);
  const bytes = await download(plan.url, dest);
  await resolveDigest(plan, bytes);
  if (!parsed.install) {
    console.log(`Baixado em ${dest}`);
    console.log(
      "Dry-run. Passe --install para copiar para /Applications (Mac) ou correr o NSIS (Windows).",
    );
    return;
  }
  try {
    if (plan.os === "macos") {
      await installMac(dest);
    } else {
      await installWindows(dest);
    }
  } finally {
    await rm(dir, { recursive: true, force: true }).catch(() => undefined);
  }
}

await mkdir(tmpdir(), { recursive: true });
try {
  await main();
} catch (err) {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
}
