import { FileSignature, FolderOpen } from "lucide-react";
import { modLabel } from "@/lib/utils";

type Props = {
  onOpen: () => void;
  onOpenSample: (which: "guide" | "signed") => void;
  dragging: boolean;
};

export function EmptyState({ onOpen, onOpenSample, dragging }: Props) {
  return (
    <div className="relative flex h-full items-center justify-center px-5">
      <div
        className={`w-full max-w-[420px] rounded-xl px-1 py-2 transition-colors ${
          dragging ? "bg-white/35 ring-2 ring-mark/40" : ""
        }`}
      >
        <div className="mx-auto mb-5 flex justify-center">
          <MiniStack />
        </div>
        <h1 className="text-center text-[22px] font-semibold tracking-tight">
          Abra um PDF
        </h1>
        <p className="mt-1.5 text-center text-[13px] leading-5 text-quiet">
          Arraste o arquivo para esta janela, ou pressione {modLabel()}+O.
        </p>
        <div className="mt-6 overflow-hidden rounded-lg bg-white/50 shadow-[0_0_0_1px_rgb(0_0_0/0.06)]">
          <button
            type="button"
            onClick={onOpen}
            className="file-row flex w-full items-center gap-3 px-3 py-2.5 text-left"
          >
            <span className="flex h-9 w-9 items-center justify-center rounded-md bg-black/5 text-ink">
              <FolderOpen className="h-4 w-4" />
            </span>
            <span>
              <span className="block text-[13px] font-medium">Escolher arquivo…</span>
              <span className="block text-[11px] text-quiet">Do disco</span>
            </span>
          </button>
          <div className="h-px bg-hairline" />
          <button
            type="button"
            onClick={() => onOpenSample("guide")}
            className="file-row flex w-full items-center gap-3 px-3 py-2.5 text-left"
          >
            <span className="thumb-doc relative h-9 w-7 overflow-hidden rounded-[2px]">
              <span className="absolute inset-x-1 top-2 space-y-0.5">
                <span className="block h-px bg-black/25" />
                <span className="block h-px w-3/4 bg-black/15" />
                <span className="block h-px bg-black/15" />
              </span>
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[13px] font-medium">guia-folio.pdf</span>
              <span className="block text-[11px] text-quiet">Exemplo · leitura e tipografia</span>
            </span>
          </button>
          <div className="h-px bg-hairline" />
          <button
            type="button"
            onClick={() => onOpenSample("signed")}
            className="file-row flex w-full items-center gap-3 px-3 py-2.5 text-left"
          >
            <span className="thumb-doc relative h-9 w-7 overflow-hidden rounded-[2px]">
              <span className="absolute inset-x-1 top-2 space-y-0.5">
                <span className="block h-px bg-black/25" />
                <span className="block h-px w-2/3 bg-black/15" />
              </span>
              <FileSignature className="absolute bottom-0.5 right-0.5 h-3 w-3 text-mark" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[13px] font-medium">contrato-assinado.pdf</span>
              <span className="block text-[11px] text-quiet">Exemplo · assinatura digital</span>
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}

function MiniStack() {
  return (
    <div className="relative h-16 w-14">
      <span className="absolute left-2 top-2 h-[52px] w-[38px] rotate-6 rounded-[3px] bg-white/70 shadow-sm" />
      <span className="absolute left-1 top-1 h-[52px] w-[38px] -rotate-3 rounded-[3px] bg-white/85 shadow" />
      <span className="thumb-doc absolute left-0 top-0 h-[54px] w-[40px] overflow-hidden rounded-[3px]">
        <span className="absolute right-0 top-0 h-3 w-3 bg-desk" />
        <span className="absolute inset-x-1.5 top-4 space-y-[3px]">
          <span className="block h-px bg-black/20" />
          <span className="block h-px w-4/5 bg-black/12" />
          <span className="block h-px bg-black/12" />
          <span className="block h-px w-3/5 bg-black/12" />
        </span>
      </span>
    </div>
  );
}
