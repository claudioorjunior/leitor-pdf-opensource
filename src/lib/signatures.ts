import { invokeVerify } from "./tauri";
import type { PdfDocument } from "./pdf";

export type SignatureStatus =
  | "valid"
  | "intactButUntrusted"
  | "documentModified"
  | "invalid"
  | "unsupported"
  | "certificateExpired"
  | "certificateNotYetValid";

export type CertificateInfo = {
  subject: string;
  commonName: string | null;
  organization: string | null;
  issuer: string;
  serial: string;
  notBefore: string;
  notAfter: string;
  isSelfSigned: boolean;
};

export type SignatureInfo = {
  id: string;
  fieldName: string | null;
  signerName: string | null;
  reason: string | null;
  location: string | null;
  contactInfo: string | null;
  signingTime: string | null;
  filter: string | null;
  subFilter: string | null;
  byteRange: number[] | null;
  coversWholeDocument: boolean;
  status: SignatureStatus;
  statusDetail: string;
  certificate: CertificateInfo | null;
  digestAlgorithm: string | null;
  signatureAlgorithm: string | null;
};

type PdfJsSignature = {
  id: string;
  fieldName?: string;
  signerName?: string | null;
  reason?: string | null;
  location?: string | null;
  contactInfo?: string | null;
  signingTime?: string | null;
  filter?: string | null;
  subFilter?: string | null;
  byteRange?: number[];
  coversWholeDocument?: boolean;
};

export async function inspectSignatures(
  doc: PdfDocument,
  _fileBytes: Uint8Array,
): Promise<SignatureInfo[]> {
  const raw = (await doc.getSignatures()) as PdfJsSignature[] | null;
  if (!raw || raw.length === 0) return [];

  const out: SignatureInfo[] = [];
  for (const meta of raw) {
    const payload = await doc.getSignatureData(meta.id);
    const base: SignatureInfo = {
      id: meta.id,
      fieldName: meta.fieldName ?? null,
      signerName: meta.signerName ?? null,
      reason: meta.reason ?? null,
      location: meta.location ?? null,
      contactInfo: meta.contactInfo ?? null,
      signingTime: formatPdfDate(meta.signingTime ?? null),
      filter: meta.filter ?? null,
      subFilter: meta.subFilter ?? null,
      byteRange: meta.byteRange ?? null,
      coversWholeDocument: Boolean(meta.coversWholeDocument),
      status: meta.coversWholeDocument ? "unsupported" : "documentModified",
      statusDetail: meta.coversWholeDocument
        ? "Assinatura reconhecida. Verificando criptografia…"
        : "Há dados no arquivo fora do intervalo assinado.",
      certificate: null,
      digestAlgorithm: null,
      signatureAlgorithm: null,
    };

    if (!payload) {
      out.push(base);
      continue;
    }

    const chunks = payload.data as Uint8Array[];
    const pkcs7 = payload.pkcs7 instanceof Uint8Array
      ? payload.pkcs7
      : new Uint8Array(payload.pkcs7 as ArrayBuffer);
    const signed = concat(chunks);
    const sha256 = new Uint8Array(await crypto.subtle.digest("SHA-256", signed as BufferSource));
    const cms = parseCms(pkcs7);

    if (cms.certificate) {
      base.certificate = cms.certificate;
      base.signerName = base.signerName ?? cms.certificate.commonName;
    }
    base.digestAlgorithm = cms.digestAlgorithm;
    base.signatureAlgorithm = cms.signatureAlgorithm;

    const native = await invokeVerify(pkcs7, sha256);
    if (native) {
      out.push({
        ...base,
        ...native,
        id: meta.id,
        coversWholeDocument: base.coversWholeDocument && native.coversWholeDocument,
        signingTime: native.signingTime
          ? formatPdfDate(native.signingTime)
          : base.signingTime,
      });
      continue;
    }

    if (!base.coversWholeDocument) {
      base.status = "documentModified";
      base.statusDetail =
        "Há bytes no arquivo fora do intervalo assinado. O documento foi alterado depois da assinatura.";
    } else if (cms.messageDigest && equalBytes(cms.messageDigest, sha256)) {
      const rsaOk = await verifyRsaSha256(cms, signed, sha256).catch(() => false);
      if (rsaOk) {
        base.status = cms.certificate?.isSelfSigned ? "intactButUntrusted" : "valid";
        base.statusDetail = cms.certificate?.isSelfSigned
          ? "Assinatura criptograficamente válida, mas o certificado é autoassinado — não há cadeia de confiança pública."
          : "Assinatura íntegra: o conteúdo coberto não foi alterado.";
      } else if (cms.messageDigest) {
        base.status = "intactButUntrusted";
        base.statusDetail =
          "O resumo SHA-256 do intervalo assinado confere. A verificação RSA completa fica a cargo do motor Rust no app nativo.";
      }
    } else if (cms.messageDigest) {
      base.status = "documentModified";
      base.statusDetail =
        "O resumo do intervalo assinado não confere. O arquivo foi alterado após a assinatura.";
    } else {
      base.status = "unsupported";
      base.statusDetail = "Assinatura reconhecida, mas o perfil CMS não pôde ser verificado neste navegador.";
    }

    out.push(base);
  }
  return out;
}

type CmsParse = {
  certificate: CertificateInfo | null;
  messageDigest: Uint8Array | null;
  digestAlgorithm: string | null;
  signatureAlgorithm: string | null;
  signature: Uint8Array | null;
  signedAttrs: Uint8Array | null;
  modulus: Uint8Array | null;
  exponent: Uint8Array | null;
};

function parseCms(pkcs7: Uint8Array): CmsParse {
  const empty: CmsParse = {
    certificate: null,
    messageDigest: null,
    digestAlgorithm: null,
    signatureAlgorithm: null,
    signature: null,
    signedAttrs: null,
    modulus: null,
    exponent: null,
  };
  try {
    const der = trimDer(pkcs7);
    const ci = readTlv(der, 0);
    const ciKids = children(ci);
    if (ciKids.length < 2) return empty;
    const wrapped = ciKids[1];
    const signedSeq = (wrapped.tag & 0xc0) === 0x80 ? readTlv(wrapped.value, 0) : wrapped;
    const sd = children(signedSeq);
    let idx = 3;
    let certs: Uint8Array | null = null;
    while (idx < sd.length && (sd[idx].tag & 0xc0) === 0x80) {
      if (sd[idx].tag === 0xa0) certs = sd[idx].value;
      idx += 1;
    }
    if (idx >= sd.length) return empty;
    const signerKids = children(children(sd[idx])[0]);
    let cursor = 2;
    const digestAlgorithm = oidName(children(signerKids[cursor])[0]?.value);
    cursor += 1;
    let signedAttrs: Uint8Array | null = null;
    let messageDigest: Uint8Array | null = null;
    if (signerKids[cursor]?.tag === 0xa0) {
      const attr = signerKids[cursor];
      const rewritten = new Uint8Array(attr.full.length);
      rewritten.set(attr.full);
      rewritten[0] = 0x31;
      signedAttrs = rewritten;
      messageDigest = findMessageDigest(attr.value);
      cursor += 1;
    }
    const signatureAlgorithm = oidName(children(signerKids[cursor])[0]?.value);
    cursor += 1;
    const signature = signerKids[cursor]?.tag === 0x04 ? signerKids[cursor].value : null;
    const cert = certs ? parseFirstCert(certs) : null;
    return {
      certificate: cert?.info ?? null,
      modulus: cert?.modulus ?? null,
      exponent: cert?.exponent ?? null,
      messageDigest,
      digestAlgorithm,
      signatureAlgorithm,
      signature,
      signedAttrs,
    };
  } catch {
    return empty;
  }
}

async function verifyRsaSha256(
  cms: CmsParse,
  _signedContent: Uint8Array,
  _sha256: Uint8Array,
): Promise<boolean> {
  if (!cms.modulus || !cms.exponent || !cms.signature || !cms.signedAttrs) return false;
  try {
    const key = await crypto.subtle.importKey(
      "jwk",
      {
        kty: "RSA",
        n: b64url(cms.modulus),
        e: b64url(cms.exponent),
        alg: "RS256",
        ext: true,
      },
      { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
      false,
      ["verify"],
    );
    return crypto.subtle.verify(
      "RSASSA-PKCS1-v1_5",
      key,
      cms.signature as BufferSource,
      cms.signedAttrs as BufferSource,
    );
  } catch {
    return false;
  }
}

function parseFirstCert(data: Uint8Array) {
  let offset = 0;
  while (offset < data.length) {
    const tlv = readTlv(data, offset);
    offset = tlv.end;
    if (tlv.tag !== 0x30) continue;
    const cn = findOidString(tlv.full, [0x55, 0x04, 0x03]);
    const org = findOidString(tlv.full, [0x55, 0x04, 0x0a]);
    const rsa = extractRsaKey(tlv.full);
    const info: CertificateInfo = {
      subject: cn ? `CN=${cn}` : "Certificado do signatário",
      commonName: cn,
      organization: org,
      issuer: cn ? `CN=${cn}` : "Emissor desconhecido",
      serial: "",
      notBefore: "",
      notAfter: "",
      isSelfSigned: true,
    };
    return { info, modulus: rsa?.n ?? null, exponent: rsa?.e ?? null };
  }
  return null;
}

function extractRsaKey(cert: Uint8Array): { n: Uint8Array; e: Uint8Array } | null {
  const rsaOid = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
  const at = indexOf(cert, rsaOid);
  if (at < 0) return null;
  // After the algorithm identifier, BIT STRING wraps the RSAPublicKey SEQUENCE.
  let pos = at + rsaOid.length;
  while (pos < cert.length && cert[pos] !== 0x03) pos += 1;
  if (pos >= cert.length) return null;
  const bit = readTlv(cert, pos);
  const payload = bit.value[0] === 0x00 ? bit.value.subarray(1) : bit.value;
  const seq = readTlv(payload, 0);
  const kids = children(seq);
  if (kids.length < 2 || kids[0].tag !== 0x02 || kids[1].tag !== 0x02) return null;
  return { n: stripInt(kids[0].value), e: stripInt(kids[1].value) };
}

function findMessageDigest(attrs: Uint8Array): Uint8Array | null {
  const oid = encodeOid("1.2.840.113549.1.9.4");
  let offset = 0;
  while (offset < attrs.length) {
    const attr = readTlv(attrs, offset);
    offset = attr.end;
    const parts = children(attr);
    if (parts.length < 2 || parts[0].tag !== 0x06) continue;
    if (!equalBytes(parts[0].value, oid)) continue;
    const inner = readTlv(parts[1].value, 0);
    return inner.tag === 0x04 ? inner.value : inner.full;
  }
  return null;
}

function findOidString(blob: Uint8Array, oid: number[]): string | null {
  const at = indexOf(blob, oid);
  if (at < 0) return null;
  let pos = at + oid.length;
  while (pos < blob.length && blob[pos] === 0x00) pos += 1;
  if (pos >= blob.length) return null;
  const tlv = readTlv(blob, pos);
  if (![0x0c, 0x13, 0x16, 0x1e, 0x0c].includes(tlv.tag) && tlv.tag !== 0x13 && tlv.tag !== 0x0c) {
    if (tlv.tag === 0x31 || tlv.tag === 0x30) {
      const inner = readTlv(tlv.value, 0);
      return new TextDecoder("utf-8", { fatal: false }).decode(inner.value);
    }
  }
  try {
    return new TextDecoder("utf-8", { fatal: false }).decode(tlv.value);
  } catch {
    return null;
  }
}

type Tlv = {
  tag: number;
  value: Uint8Array;
  full: Uint8Array;
  end: number;
};

function readTlv(data: Uint8Array, offset: number): Tlv {
  const tag = data[offset];
  let hlen = 2;
  let vlen = data[offset + 1];
  if (vlen & 0x80) {
    const n = vlen & 0x7f;
    vlen = 0;
    for (let i = 0; i < n; i++) vlen = (vlen << 8) | data[offset + 2 + i];
    hlen = 2 + n;
  }
  const start = offset + hlen;
  const end = start + vlen;
  return {
    tag,
    value: data.subarray(start, end),
    full: data.subarray(offset, end),
    end,
  };
}

function children(seq: Tlv): Tlv[] {
  const out: Tlv[] = [];
  let offset = 0;
  const data = seq.value;
  while (offset < data.length) {
    const tlv = readTlv(data, offset);
    out.push(tlv);
    offset = tlv.end;
  }
  return out;
}

function trimDer(data: Uint8Array): Uint8Array {
  const start = data.indexOf(0x30);
  if (start < 0) return data;
  const sliced = data.subarray(start);
  const tlv = readTlv(sliced, 0);
  return tlv.full;
}

function oidName(bytes?: Uint8Array | null): string | null {
  if (!bytes) return null;
  const hex = toHex(bytes);
  if (hex === "608648016503040201") return "SHA-256";
  if (hex === "2b0e03021a") return "SHA-1";
  if (hex === "2a864886f70d010101") return "RSA";
  if (hex === "2a864886f70d01010b") return "RSA";
  return null;
}

function encodeOid(s: string): Uint8Array {
  const parts = s.split(".").map(Number);
  const out: number[] = [parts[0] * 40 + parts[1]];
  for (const component of parts.slice(2)) {
    const stack: number[] = [component & 0x7f];
    let n = component >> 7;
    while (n > 0) {
      stack.push((n & 0x7f) | 0x80);
      n >>= 7;
    }
    stack.reverse();
    out.push(...stack);
  }
  return new Uint8Array(out);
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}

function equalBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  return a.every((v, i) => v === b[i]);
}

function stripInt(v: Uint8Array): Uint8Array {
  let i = 0;
  while (i < v.length - 1 && v[i] === 0) i += 1;
  return v.subarray(i);
}

function indexOf(hay: Uint8Array, needle: number[]): number {
  outer: for (let i = 0; i <= hay.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (hay[i + j] !== needle[j]) continue outer;
    }
    return i;
  }
  return -1;
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function b64url(bytes: Uint8Array): string {
  let binary = "";
  bytes.forEach((b) => {
    binary += String.fromCharCode(b);
  });
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

export function formatPdfDate(raw: string | null): string | null {
  if (!raw) return null;
  const s = raw.startsWith("D:") ? raw.slice(2) : raw;
  if (s.length >= 14 && /^\d/.test(s)) {
    return `${s.slice(0, 4)}-${s.slice(4, 6)}-${s.slice(6, 8)} ${s.slice(8, 10)}:${s.slice(10, 12)}:${s.slice(12, 14)}`;
  }
  return raw;
}

export function statusLabel(status: SignatureStatus): string {
  switch (status) {
    case "valid":
      return "Válida";
    case "intactButUntrusted":
      return "Íntegra (sem confiança pública)";
    case "documentModified":
      return "Documento alterado";
    case "invalid":
      return "Inválida";
    case "certificateExpired":
      return "Certificado expirado";
    case "certificateNotYetValid":
      return "Certificado ainda não válido";
    default:
      return "Não verificada";
  }
}
