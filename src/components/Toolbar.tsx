import {
  ChevronLeft,
  ChevronRight,
  FolderOpen,
  Minus,
  Plus,
  Search,
  ShieldCheck,
  X,
} from "lucide-react";
import { IconButton } from "@/components/IconButton";
import { cn, modLabel } from "@/lib/utils";

type Props = {
  fileName: string | null;
  page: number;
  pageCount: number;
  zoom: number;
  signatureCount: number;
  signaturesOpen: boolean;
  searchOpen: boolean;
  search: string;
  searchHits: number;
  loading: boolean;
  onOpen: () => void;
  onClose: () => void;
  onZoom: (next: number) => void;
  onFitWidth: () => void;
  onFitPage: () => void;
  onPage: (next: number) => void;
  onToggleSignatures: () => void;
  onToggleSearch: () => void;
  onSearchChange: (value: string) => void;
};

export function Toolbar({
  fileName,
  page,
  pageCount,
  zoom,
  signatureCount,
  signaturesOpen,
  searchOpen,
  search,
  searchHits,
  loading,
  onOpen,
  onClose,
  onZoom,
  onFitWidth,
  onFitPage,
  onPage,
  onToggleSignatures,
  onToggleSearch,
  onSearchChange,
}: Props) {
  return (
    <header className="titlebar flex h-10 shrink-0 items-center gap-2 px-2.5">
      <div className="flex w-[28%] min-w-0 items-center gap-1.5">
        <img src="/tsuro-mark.png" alt="" className="h-5 w-5 rounded-[5px]" />
        <IconButton onClick={onOpen} title={`Abrir (${modLabel()}+O)`}>
          <FolderOpen className="h-3.5 w-3.5" />
        </IconButton>
        {fileName && (
          <IconButton onClick={onClose} title="Fechar">
            <X className="h-3.5 w-3.5" />
          </IconButton>
        )}
      </div>

      <div className="min-w-0 flex-1 truncate text-center text-[13px] font-medium tracking-tight text-ink">
        {loading ? "A abrir…" : fileName ?? "Tsuro"}
      </div>

      <div className="flex w-[36%] min-w-0 items-center justify-end gap-1.5">
        {fileName ? (
          <>
            <div className="seg hidden sm:flex">
              <IconButton disabled={page <= 1} onClick={() => onPage(page - 1)} title="Página anterior">
                <ChevronLeft className="h-3.5 w-3.5" />
              </IconButton>
              <form
                className="flex h-6 items-center px-1 text-[11px] tabular-nums text-quiet"
                onSubmit={(e) => {
                  e.preventDefault();
                  const input = e.currentTarget.elements.namedItem("page") as HTMLInputElement;
                  onPage(Number(input.value));
                }}
              >
                <input
                  key={page}
                  name="page"
                  defaultValue={page}
                  className="h-5 w-7 rounded bg-transparent text-center text-ink outline-none"
                />
                <span>/ {pageCount}</span>
              </form>
              <IconButton
                disabled={page >= pageCount}
                onClick={() => onPage(page + 1)}
                title="Página seguinte"
              >
                <ChevronRight className="h-3.5 w-3.5" />
              </IconButton>
            </div>

            <div className="seg hidden md:flex">
              <IconButton onClick={() => onZoom(zoom - 0.1)} title="Reduzir">
                <Minus className="h-3.5 w-3.5" />
              </IconButton>
              <button
                type="button"
                onClick={onFitWidth}
                onDoubleClick={onFitPage}
                title="Ajustar à largura · duplo clique: página"
                className="h-6 min-w-10 px-1 text-[11px] tabular-nums text-quiet hover:text-ink"
              >
                {Math.round(zoom * 100)}%
              </button>
              <IconButton onClick={() => onZoom(zoom + 0.1)} title="Ampliar">
                <Plus className="h-3.5 w-3.5" />
              </IconButton>
            </div>

            {searchOpen ? (
              <div className="flex h-7 items-center gap-1 rounded-md bg-black/5 px-2">
                <Search className="h-3.5 w-3.5 text-quiet" />
                <input
                  autoFocus
                  value={search}
                  onChange={(e) => onSearchChange(e.target.value)}
                  placeholder="Buscar"
                  className="w-24 bg-transparent text-[12px] outline-none sm:w-36"
                />
                {search ? (
                  <span className="text-[10px] text-quiet">{searchHits}</span>
                ) : null}
                <button type="button" onClick={onToggleSearch} className="text-quiet">
                  <X className="h-3 w-3" />
                </button>
              </div>
            ) : (
              <IconButton onClick={onToggleSearch} title={`Buscar (${modLabel()}+F)`}>
                <Search className="h-3.5 w-3.5" />
              </IconButton>
            )}

            <IconButton
              active={signaturesOpen}
              onClick={onToggleSignatures}
              title={`Assinaturas (${modLabel()}+I)`}
              className={cn(signatureCount > 0 && "text-mark")}
            >
              <span className="relative">
                <ShieldCheck className="h-3.5 w-3.5" />
                {signatureCount > 0 && (
                  <span className="absolute -right-1.5 -top-1 h-1.5 w-1.5 rounded-full bg-mark" />
                )}
              </span>
            </IconButton>
          </>
        ) : (
          <button
            type="button"
            onClick={onOpen}
            className="h-7 rounded-md px-2.5 text-[12px] font-medium text-ink hover:bg-black/6"
          >
            Abrir
          </button>
        )}
      </div>
    </header>
  );
}
