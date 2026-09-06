import { FileSignature, FileText, FolderOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { modLabel } from "@/lib/utils";

type Props = {
  onOpen: () => void;
  onOpenSample: (which: "guide" | "signed") => void;
  dragging: boolean;
};

export function EmptyState({ onOpen, onOpenSample, dragging }: Props) {
  return (
    <div className="flex h-full items-center justify-center px-6 py-10">
      <div
        className={`w-full max-w-xl rounded-2xl border border-dashed px-8 py-12 text-center transition-colors ${
          dragging ? "border-teal bg-teal-soft/70" : "border-line bg-sheet/80"
        }`}
      >
        <div className="mx-auto mb-5 flex h-14 w-14 items-center justify-center rounded-2xl bg-teal text-white shadow-sm">
          <FileText className="h-7 w-7" />
        </div>
        <h1 className="font-serif text-3xl tracking-tight text-ink">Folio</h1>
        <p className="mt-2 text-[15px] leading-6 text-muted">
          Leitor de PDF leve para macOS e Windows. Tipografia fiel, rolagem
          imediata e reconhecimento de assinaturas digitais — sem conta e sem nuvem.
        </p>
        <div className="mt-7 flex flex-wrap items-center justify-center gap-3">
          <Button onClick={onOpen} size="lg">
            <FolderOpen className="h-4 w-4" />
            Abrir PDF
          </Button>
          <Button variant="outline" size="lg" onClick={() => onOpenSample("guide")}>
            Ver guia de exemplo
          </Button>
        </div>
        <p className="mt-4 text-xs text-muted">
          Arraste um arquivo para esta janela · {modLabel()}+O
        </p>
        <button
          type="button"
          onClick={() => onOpenSample("signed")}
          className="mt-8 inline-flex items-center gap-2 rounded-full border border-line bg-paper px-3 py-1.5 text-xs text-ink hover:border-teal/40"
        >
          <FileSignature className="h-3.5 w-3.5 text-seal" />
          Abrir contrato assinado digitalmente
        </button>
      </div>
    </div>
  );
}
