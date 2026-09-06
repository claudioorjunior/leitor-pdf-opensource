import { Badge } from "@/components/ui/badge";
import { statusLabel, type SignatureInfo } from "@/lib/signatures";
import { ShieldAlert, ShieldCheck, ShieldQuestion } from "lucide-react";

type Props = {
  signatures: SignatureInfo[];
  loading: boolean;
  error: string | null;
};

export function SignaturePanel({ signatures, loading, error }: Props) {
  return (
    <aside className="flex h-full w-full max-w-full flex-col border-l border-line bg-sheet sm:w-[340px]">
      <div className="border-b border-line px-4 py-3">
        <p className="text-[11px] font-semibold tracking-[0.14em] text-teal">
          ASSINATURAS DIGITAIS
        </p>
        <h2 className="mt-1 text-base font-semibold">Integridade do arquivo</h2>
        <p className="mt-1 text-xs leading-5 text-muted">
          O Folio lê o dicionário /Sig, o PKCS#7 e o intervalo de bytes. A
          criptografia é conferida no motor Rust (e também neste visor).
        </p>
      </div>
      <div className="flex-1 space-y-3 overflow-auto p-3 scrollbar-thin">
        {loading && (
          <p className="px-1 py-6 text-center text-sm text-muted">A verificar assinaturas…</p>
        )}
        {error && (
          <p className="rounded-lg bg-[#f4d6d4] px-3 py-2 text-sm text-danger">{error}</p>
        )}
        {!loading && !error && signatures.length === 0 && (
          <div className="rounded-xl border border-line bg-paper px-4 py-8 text-center">
            <ShieldQuestion className="mx-auto h-8 w-8 text-muted" />
            <p className="mt-3 text-sm font-medium">Nenhuma assinatura</p>
            <p className="mt-1 text-xs leading-5 text-muted">
              Este PDF não contém um campo de assinatura digital. O conteúdo
              ainda pode ser lido normalmente.
            </p>
          </div>
        )}
        {signatures.map((sig) => (
          <article key={sig.id} className="rounded-xl border border-line bg-paper p-3">
            <div className="flex items-start gap-2">
              <StatusIcon status={sig.status} />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-semibold">
                  {sig.signerName || sig.certificate?.commonName || "Signatário desconhecido"}
                </p>
                <Badge tone={toneFor(sig.status)} className="mt-1">
                  {statusLabel(sig.status)}
                </Badge>
              </div>
            </div>
            <p className="mt-3 text-[13px] leading-5 text-ink/80">{sig.statusDetail}</p>
            <dl className="mt-3 space-y-1.5 text-[12px]">
              <Row label="Campo" value={sig.fieldName} />
              <Row label="Motivo" value={sig.reason} />
              <Row label="Local" value={sig.location} />
              <Row label="Contacto" value={sig.contactInfo} />
              <Row label="Data" value={sig.signingTime} />
              <Row label="Filtro" value={sig.subFilter || sig.filter} />
              <Row label="Resumo" value={sig.digestAlgorithm} />
              <Row label="Cobre o ficheiro" value={sig.coversWholeDocument ? "Sim" : "Não"} />
              {sig.certificate && (
                <>
                  <Row label="Organização" value={sig.certificate.organization} />
                  <Row label="Emissor" value={sig.certificate.issuer} />
                  <Row
                    label="Certificado"
                    value={
                      sig.certificate.isSelfSigned
                        ? "Autoassinado (demonstração)"
                        : "Cadeia presente"
                    }
                  />
                </>
              )}
            </dl>
          </article>
        ))}
      </div>
    </aside>
  );
}

function Row({ label, value }: { label: string; value?: string | null }) {
  if (!value) return null;
  return (
    <div className="flex gap-2">
      <dt className="w-24 shrink-0 text-muted">{label}</dt>
      <dd className="min-w-0 break-words text-ink">{value}</dd>
    </div>
  );
}

function toneFor(status: SignatureInfo["status"]) {
  if (status === "valid") return "ok" as const;
  if (status === "intactButUntrusted") return "warn" as const;
  if (status === "documentModified" || status === "invalid" || status === "certificateExpired") {
    return "bad" as const;
  }
  return "muted" as const;
}

function StatusIcon({ status }: { status: SignatureInfo["status"] }) {
  if (status === "valid" || status === "intactButUntrusted") {
    return <ShieldCheck className="mt-0.5 h-5 w-5 text-teal" />;
  }
  if (status === "documentModified" || status === "invalid") {
    return <ShieldAlert className="mt-0.5 h-5 w-5 text-danger" />;
  }
  return <ShieldQuestion className="mt-0.5 h-5 w-5 text-muted" />;
}
