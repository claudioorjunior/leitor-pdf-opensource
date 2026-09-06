import { ShieldAlert, ShieldCheck, ShieldQuestion, X } from "lucide-react";
import { IconButton } from "@/components/IconButton";
import { statusLabel, type SignatureInfo } from "@/lib/signatures";

type Props = {
  signatures: SignatureInfo[];
  loading: boolean;
  error: string | null;
  onClose: () => void;
};

export function SignaturePanel({ signatures, loading, error, onClose }: Props) {
  return (
    <aside className="sig-dock flex h-full w-[300px] shrink-0 flex-col border-l border-hairline bg-chrome">
      <div className="flex h-10 items-center justify-between px-3">
        <p className="text-[12px] font-semibold tracking-tight">Assinaturas</p>
        <IconButton onClick={onClose} title="Fechar painel">
          <X className="h-3.5 w-3.5" />
        </IconButton>
      </div>
      <div className="flex-1 overflow-auto px-3 pb-4 scrollbar-thin">
        {loading && <p className="py-8 text-center text-[12px] text-quiet">A verificar…</p>}
        {error && <p className="text-[12px] leading-5 text-bad">{error}</p>}
        {!loading && !error && signatures.length === 0 && (
          <div className="px-1 py-6 text-center">
            <ShieldQuestion className="mx-auto h-6 w-6 text-quiet" />
            <p className="mt-2 text-[13px] font-medium">Nenhuma assinatura</p>
            <p className="mt-1 text-[12px] leading-5 text-quiet">
              Este arquivo não traz um campo de assinatura digital.
            </p>
          </div>
        )}
        {signatures.map((sig) => (
          <article key={sig.id} className="mb-3 rounded-lg bg-white/70 p-3 shadow-[0_0_0_1px_rgb(0_0_0/0.06)]">
            <div className="flex items-start gap-2">
              <StatusIcon status={sig.status} />
              <div className="min-w-0">
                <p className="truncate text-[13px] font-semibold leading-5">
                  {sig.signerName || sig.certificate?.commonName || "Signatário"}
                </p>
                <p className={`text-[11px] font-medium ${statusClass(sig.status)}`}>
                  {statusLabel(sig.status)}
                </p>
              </div>
            </div>
            <p className="mt-2 text-[12px] leading-5 text-quiet">{sig.statusDetail}</p>
            <dl className="mt-3 space-y-2 border-t border-hairline pt-3">
              <Row label="Motivo" value={sig.reason} />
              <Row label="Local" value={sig.location} />
              <Row label="Quando" value={sig.signingTime} />
              <Row label="Campo" value={sig.fieldName} />
              <Row
                label="Cobertura"
                value={sig.coversWholeDocument ? "Documento inteiro" : "Parcial"}
              />
              <Row label="Algoritmo" value={sig.digestAlgorithm} />
              {sig.certificate && (
                <>
                  <Row label="Organização" value={sig.certificate.organization} />
                  <Row
                    label="Certificado"
                    value={
                      sig.certificate.isSelfSigned ? "Autoassinado" : "Com emissor"
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
    <div>
      <dt className="text-[10px] uppercase tracking-[0.08em] text-quiet">{label}</dt>
      <dd className="mt-0.5 break-words text-[12px] leading-4">{value}</dd>
    </div>
  );
}

function statusClass(status: SignatureInfo["status"]) {
  if (status === "valid") return "text-good";
  if (status === "intactButUntrusted") return "text-caution";
  if (status === "documentModified" || status === "invalid" || status === "certificateExpired") {
    return "text-bad";
  }
  return "text-quiet";
}

function StatusIcon({ status }: { status: SignatureInfo["status"] }) {
  const cls = "mt-0.5 h-4 w-4 shrink-0";
  if (status === "valid" || status === "intactButUntrusted") {
    return <ShieldCheck className={`${cls} text-good`} />;
  }
  if (status === "documentModified" || status === "invalid") {
    return <ShieldAlert className={`${cls} text-bad`} />;
  }
  return <ShieldQuestion className={`${cls} text-quiet`} />;
}
