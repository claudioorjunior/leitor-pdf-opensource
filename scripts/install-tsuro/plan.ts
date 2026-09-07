import { createHash } from "node:crypto";

export const REPO = "claudioorjunior/tsuro-pdf";
export const CLONE_URL = "https://github.com/claudioorjunior/tsuro-pdf.git";
export const TRACKING_ISSUE = "https://github.com/claudioorjunior/tsuro-pdf/issues/5";
export const API_BASE = `https://api.github.com/repos/${REPO}`;
export const MAC_APP_DEST = "/Applications/Tsuro.app";

/** Nomes: `Tsuro-{versão}-aarch64-apple-darwin.dmg` / `Tsuro-{versão}-x86_64-pc-windows-msvc-setup.exe`. */
export const TRIPLE_MAC = "aarch64-apple-darwin";
export const TRIPLE_WIN = "x86_64-pc-windows-msvc";

export type Triple = typeof TRIPLE_MAC | typeof TRIPLE_WIN;

export type GhAsset = {
  name: string;
  browser_download_url: string;
  digest?: string | null;
};

export type GhRelease = {
  tag_name: string;
  assets: GhAsset[];
};

export type Host =
  | { ok: true; triple: Triple; os: "macos" | "windows" }
  | { ok: false; reason: "intel-mac" | "linux" | "unknown"; platform: string; arch: string };

export type InstallPlan = {
  tag: string;
  triple: Triple;
  os: "macos" | "windows";
  assetName: string;
  url: string;
  digest?: string;
  checksumUrl?: string;
  steps: string[];
};

export type PlanError = {
  code:
    | "no-release"
    | "missing-asset"
    | "unsupported-host"
    | "windows-unpublished"
    | "translocated";
  message: string;
};

export function hostFrom(platform: string, arch: string): Host {
  if (platform === "darwin") {
    if (arch === "arm64") {
      return { ok: true, triple: TRIPLE_MAC, os: "macos" };
    }
    return { ok: false, reason: "intel-mac", platform, arch };
  }
  if (platform === "win32") {
    if (arch === "x64") {
      return { ok: true, triple: TRIPLE_WIN, os: "windows" };
    }
    return { ok: false, reason: "unknown", platform, arch };
  }
  if (platform === "linux") {
    return { ok: false, reason: "linux", platform, arch };
  }
  return { ok: false, reason: "unknown", platform, arch };
}

export function hostError(host: Extract<Host, { ok: false }>): PlanError {
  if (host.reason === "intel-mac") {
    return {
      code: "unsupported-host",
      message:
        "Não há build para Mac Intel (x86_64). Use o clone e o bundle local, ou um Mac arm64.",
    };
  }
  if (host.reason === "linux") {
    return {
      code: "unsupported-host",
      message:
        "Linux está fora do instalador provisório. Clone o repositório para desenvolver.",
    };
  }
  return {
    code: "unsupported-host",
    message: `Sistema não suportado (${host.platform}/${host.arch}).`,
  };
}

export function normalizeTag(tag: string): string {
  return tag.trim().replace(/^v/i, "");
}

export function compareSemver(a: string, b: string): number {
  const pa = normalizeTag(a)
    .split(".")
    .map((n) => Number.parseInt(n, 10) || 0);
  const pb = normalizeTag(b)
    .split(".")
    .map((n) => Number.parseInt(n, 10) || 0);
  const len = Math.max(pa.length, pb.length, 3);
  for (let i = 0; i < len; i++) {
    const da = pa[i] ?? 0;
    const db = pb[i] ?? 0;
    if (da > db) return 1;
    if (da < db) return -1;
  }
  return 0;
}

export function isNewer(remoteTag: string, installed: string | null): boolean {
  if (!installed) return true;
  return compareSemver(remoteTag, installed) > 0;
}

export function parseDigest(raw: string | null | undefined): string | undefined {
  if (!raw) return undefined;
  const trimmed = raw.trim();
  const prefixed = trimmed.match(/^sha-?256:([0-9a-f]{64})$/i);
  if (prefixed) return prefixed[1].toLowerCase();
  if (/^[0-9a-f]{64}$/i.test(trimmed)) return trimmed.toLowerCase();
  return undefined;
}

export function parseSha256File(body: string, assetName: string): string | undefined {
  for (const rawLine of body.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    const direct = parseDigest(line);
    if (direct) return direct;
    const gnu = line.match(/^([0-9a-f]{64})\s+\*?(\S+)$/i);
    if (gnu && gnu[2].endsWith(assetName)) return gnu[1].toLowerCase();
    const bsd = line.match(/^SHA256\s+\((.+)\)\s+=\s+([0-9a-f]{64})$/i);
    if (bsd && bsd[1].endsWith(assetName)) return bsd[2].toLowerCase();
  }
  return undefined;
}

export function sha256Hex(data: Uint8Array): string {
  return createHash("sha256").update(data).digest("hex");
}

export function checksumMatches(data: Uint8Array, expectedHex: string): boolean {
  return sha256Hex(data) === expectedHex.toLowerCase();
}

export function isTranslocated(path: string): boolean {
  return path.includes("/AppTranslocation/");
}

export function selectAsset(release: GhRelease, triple: Triple): GhAsset | undefined {
  const ext = triple === TRIPLE_MAC ? ".dmg" : ".exe";
  return release.assets.find(
    (asset) => asset.name.includes(triple) && asset.name.toLowerCase().endsWith(ext),
  );
}

export function checksumSidecar(release: GhRelease, assetName: string): GhAsset | undefined {
  const exact = `${assetName}.sha256`;
  return release.assets.find(
    (asset) =>
      asset.name === exact ||
      /^sha256sums(\.txt)?$/i.test(asset.name) ||
      asset.name === "SHA256SUMS",
  );
}

export function noReleaseError(): PlanError {
  return {
    code: "no-release",
    message: [
      "Ainda não há GitHub Release para este sistema.",
      `git clone ${CLONE_URL}`,
      "No Mac, depois do clone: cargo build --release -p tsuro && ./scripts/bundle-macos.sh",
      `Acompanhe: ${TRACKING_ISSUE}`,
    ].join("\n"),
  };
}

export function planFromRelease(
  release: GhRelease | null,
  host: Host,
): { plan: InstallPlan } | { error: PlanError } {
  if (!host.ok) {
    return { error: hostError(host) };
  }
  if (!release || !release.tag_name) {
    return { error: noReleaseError() };
  }
  const asset = selectAsset(release, host.triple);
  if (!asset) {
    if (host.os === "windows") {
      return {
        error: {
          code: "windows-unpublished",
          message: `Ainda não há instalador Windows nesta release (${release.tag_name}). O NSIS ainda não foi publicado.`,
        },
      };
    }
    return {
      error: {
        code: "missing-asset",
        message: `A release ${release.tag_name} não tem artefato para ${host.triple}.\n${noReleaseError().message}`,
      },
    };
  }
  const sidecar = checksumSidecar(release, asset.name);
  const digest = parseDigest(asset.digest);
  const steps =
    host.os === "macos"
      ? [
          "hdiutil attach -nobrowse -readonly <dmg>",
          `ditto <Tsuro.app> ${MAC_APP_DEST}`,
          "hdiutil detach <mount>",
        ]
      : [
          // NSIS CurrentUser: /S silencioso. /D= só quando o template fixar a pasta.
          "<setup.exe> /S",
        ];
  return {
    plan: {
      tag: release.tag_name,
      triple: host.triple,
      os: host.os,
      assetName: asset.name,
      url: asset.browser_download_url,
      digest,
      checksumUrl: digest ? undefined : sidecar?.browser_download_url,
      steps,
    },
  };
}

export function versionFromInfoPlist(xml: string): string | null {
  const match = xml.match(
    /<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/,
  );
  return match?.[1]?.trim() ?? null;
}
