import {
  ChevronLeft,
  ChevronRight,
  FileSignature,
  FolderOpen,
  Minus,
  Plus,
  Search,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
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
    <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line bg-sheet/90 px-3 backdrop-blur">
      <div className="flex items-center gap-2">
        <img src="/folio.svg" alt="" className="h-7 w-7 rounded-md" />
        <span className="hidden text-sm font-semibold tracking-tight sm:inline">Folio</span>
      </div>

      <Button variant="ghost" size="sm" onClick={onOpen} title={`${modLabel()}+O`}>
        <FolderOpen className="h-4 w-4" />
        <span className="hidden sm:inline">Abrir</span>
      </Button>

      <div className="min-w-0 flex-1 truncate px-2 text-center text-[13px] text-muted">
        {loading ? "A abrir…" : fileName ?? "Nenhum documento"}
      </div>

      {fileName && (
        <>
          <div className="hidden items-center gap-1 md:flex">
            <Button
              variant="ghost"
              size="icon"
              onClick={() => onPage(page - 1)}
              disabled={page <= 1}
            >
              <ChevronLeft className="h-4 w-4" />
            </Button>
            <form
              className="flex items-center gap-1 text-[13px] text-muted"
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
                className="h-7 w-10 rounded border border-line bg-paper text-center text-ink"
              />
              <span>/ {pageCount}</span>
            </form>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => onPage(page + 1)}
              disabled={page >= pageCount}
            >
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>

          <div className="flex items-center gap-0.5">
            <Button variant="ghost" size="icon" onClick={() => onZoom(zoom - 0.1)}>
              <Minus className="h-4 w-4" />
            </Button>
            <button
              type="button"
              onClick={onFitWidth}
              className="h-8 min-w-12 rounded-md px-1 text-[12px] text-muted hover:bg-ink/6"
              title="Ajustar à largura"
            >
              {Math.round(zoom * 100)}%
            </button>
            <Button variant="ghost" size="icon" onClick={() => onZoom(zoom + 0.1)}>
              <Plus className="h-4 w-4" />
            </Button>
            <Button variant="ghost" size="sm" className="hidden lg:inline-flex" onClick={onFitPage}>
              Página
            </Button>
          </div>

          {searchOpen ? (
            <div className="flex h-8 items-center gap-1 rounded-md border border-line bg-paper px-2">
              <Search className="h-3.5 w-3.5 text-muted" />
              <input
                autoFocus
                value={search}
                onChange={(e) => onSearchChange(e.target.value)}
                placeholder="Buscar no texto"
                className="w-28 bg-transparent text-[13px] outline-none sm:w-40"
              />
              {search && (
                <span className="text-[11px] text-muted">
                  {searchHits} pág.
                </span>
              )}
              <button type="button" onClick={onToggleSearch}>
                <X className="h-3.5 w-3.5 text-muted" />
              </button>
            </div>
          ) : (
            <Button variant="ghost" size="icon" onClick={onToggleSearch} title={`${modLabel()}+F`}>
              <Search className="h-4 w-4" />
            </Button>
          )}

          <Button
            variant={signaturesOpen ? "outline" : "ghost"}
            size="sm"
            onClick={onToggleSignatures}
            className={cn(signatureCount > 0 && "text-teal")}
            title={`${modLabel()}+I`}
          >
            <FileSignature className="h-4 w-4" />
            <span className="hidden sm:inline">
              {signatureCount > 0 ? `${signatureCount} assinatura${signatureCount > 1 ? "s" : ""}` : "Assinaturas"}
            </span>
          </Button>

          <Button variant="ghost" size="icon" onClick={onClose} title="Fechar documento">
            <X className="h-4 w-4" />
          </Button>
        </>
      )}
    </header>
  );
}
