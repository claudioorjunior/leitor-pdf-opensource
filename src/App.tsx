import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import { SignaturePanel } from "@/components/SignaturePanel";
import { Toolbar } from "@/components/Toolbar";
import { Viewer } from "@/components/Viewer";
import { loadPdf, searchDocument, type PdfDocument } from "@/lib/pdf";
import { inspectSignatures, type SignatureInfo } from "@/lib/signatures";

type OpenDoc = {
  name: string;
  size: number;
  bytes: Uint8Array;
  pdf: PdfDocument;
};

export default function App() {
  const fileRef = useRef<HTMLInputElement>(null);
  const [doc, setDoc] = useState<OpenDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [page, setPage] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [fit, setFit] = useState<"width" | "page" | "manual">("width");
  const [signaturesOpen, setSignaturesOpen] = useState(false);
  const [signatures, setSignatures] = useState<SignatureInfo[]>([]);
  const [sigLoading, setSigLoading] = useState(false);
  const [sigError, setSigError] = useState<string | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [searchHits, setSearchHits] = useState(0);
  const [dragging, setDragging] = useState(false);
  const [scrollNonce, setScrollNonce] = useState(0);
  const [paneWidth, setPaneWidth] = useState(() =>
    typeof window === "undefined" ? 960 : window.innerWidth,
  );

  const openBytes = useCallback(async (bytes: Uint8Array, name: string) => {
    setLoading(true);
    setError(null);
    setSignatures([]);
    setSigError(null);
    try {
      const pdf = await loadPdf(bytes);
      setDoc({ name, size: bytes.byteLength, bytes, pdf });
      document.title = `${name} — Tsuro`;
      setPage(1);
      setFit("width");
      setSearch("");
      setSearchOpen(false);
      setSigLoading(true);
      try {
        const found = await inspectSignatures(pdf, bytes);
        setSignatures(found);
        if (found.length > 0) setSignaturesOpen(true);
      } catch (err) {
        setSigError(err instanceof Error ? err.message : "Falha ao ler assinaturas.");
      } finally {
        setSigLoading(false);
      }
    } catch (err) {
      setDoc(null);
      setError(
        err instanceof Error
          ? `Não foi possível abrir este PDF. ${err.message}`
          : "Não foi possível abrir este PDF.",
      );
    } finally {
      setLoading(false);
    }
  }, []);

  const openFile = useCallback(
    async (file: File) => {
      const buf = new Uint8Array(await file.arrayBuffer());
      await openBytes(buf, file.name);
    },
    [openBytes],
  );

  const openSample = useCallback(
    async (which: "guide" | "signed") => {
      const path =
        which === "guide" ? "/samples/guia-folio.pdf" : "/samples/contrato-assinado.pdf";
      const name = which === "guide" ? "guia-folio.pdf" : "contrato-assinado.pdf";
      const res = await fetch(path);
      if (!res.ok) {
        setError("O exemplo não está disponível neste build.");
        return;
      }
      const buf = new Uint8Array(await res.arrayBuffer());
      await openBytes(buf, name);
    },
    [openBytes],
  );

  const pick = () => fileRef.current?.click();

  useEffect(() => {
    const onResize = () => setPaneWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const fittedZoom = useMemo(() => {
    if (!doc) return 1;
    const usable = Math.max(320, paneWidth - (signaturesOpen ? 360 : 48));
    // A4-ish fallback until first page reports size; Viewer uses real viewport.
    const pageWidth = 595;
    const widthScale = (usable - 48) / pageWidth;
    if (fit === "width") return Math.min(2.4, Math.max(0.45, widthScale));
    if (fit === "page") return Math.min(2.4, Math.max(0.45, widthScale * 0.72));
    return zoom;
  }, [doc, fit, paneWidth, signaturesOpen, zoom]);

  const activeZoom = fit === "manual" ? zoom : fittedZoom;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key.toLowerCase() === "o") {
        e.preventDefault();
        pick();
      }
      if (mod && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setSearchOpen(true);
      }
      if (mod && e.key.toLowerCase() === "i") {
        e.preventDefault();
        setSignaturesOpen((v) => !v);
      }
      if (mod && (e.key === "=" || e.key === "+")) {
        e.preventDefault();
        setFit("manual");
        setZoom((z) => Math.min(3, z + 0.1));
      }
      if (mod && e.key === "-") {
        e.preventDefault();
        setFit("manual");
        setZoom((z) => Math.max(0.4, z - 0.1));
      }
      if (mod && e.key === "0") {
        e.preventDefault();
        setFit("width");
      }
      if (e.key === "Escape") {
        setSearchOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!doc || !search.trim()) {
      setSearchHits(0);
      return;
    }
    let cancelled = false;
    searchDocument(doc.pdf, search).then((hits) => {
      if (!cancelled) {
        setSearchHits(hits.length);
        if (hits[0]) setPage(hits[0].pageIndex + 1);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [doc, search]);

  return (
    <div
      className="app-shell flex h-full flex-col"
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        const file = e.dataTransfer.files[0];
        if (file && (file.type === "application/pdf" || file.name.toLowerCase().endsWith(".pdf"))) {
          void openFile(file);
        } else {
          setError("Solte um arquivo PDF.");
        }
      }}
    >
      <input
        ref={fileRef}
        type="file"
        accept="application/pdf,.pdf"
        className="hidden"
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) void openFile(file);
          e.target.value = "";
        }}
      />
      <Toolbar
        fileName={doc?.name ?? null}
        page={page}
        pageCount={doc?.pdf.numPages ?? 0}
        zoom={activeZoom}
        signatureCount={signatures.length}
        signaturesOpen={signaturesOpen}
        searchOpen={searchOpen}
        search={search}
        searchHits={searchHits}
        loading={loading}
        onOpen={pick}
        onClose={() => {
          void doc?.pdf.cleanup();
          setDoc(null);
          document.title = "Tsuro";
          setSignatures([]);
          setError(null);
          setSignaturesOpen(false);
        }}
        onZoom={(next) => {
          setFit("manual");
          setZoom(Math.min(3, Math.max(0.4, next)));
        }}
        onFitWidth={() => setFit("width")}
        onFitPage={() => setFit("page")}
        onPage={(next) => {
          if (!doc) return;
          setPage(Math.min(doc.pdf.numPages, Math.max(1, next)));
          setScrollNonce((n) => n + 1);
        }}
        onToggleSignatures={() => setSignaturesOpen((v) => !v)}
        onToggleSearch={() => setSearchOpen((v) => !v)}
        onSearchChange={setSearch}
      />

      <div className="flex min-h-0 flex-1">
        <main className="relative min-w-0 flex-1">
          {error && (
            <div className="absolute left-1/2 top-3 z-10 -translate-x-1/2 rounded-md bg-[#1c1c1c] px-3 py-1.5 text-[12px] text-white shadow-lg">
              {error}
            </div>
          )}
          {!doc && <EmptyState onOpen={pick} onOpenSample={openSample} dragging={dragging} />}
          {doc && (
            <Viewer
              doc={doc.pdf}
              scale={activeZoom}
              currentPage={page}
              search={search}
              scrollNonce={scrollNonce}
              onPageVisible={setPage}
            />
          )}
        </main>
        {doc && signaturesOpen && (
          <SignaturePanel
            signatures={signatures}
            loading={sigLoading}
            error={sigError}
            onClose={() => setSignaturesOpen(false)}
          />
        )}
      </div>
    </div>
  );
}
