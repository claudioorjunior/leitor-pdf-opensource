import { useEffect, useMemo, useRef, useState } from "react";
import type { PdfDocument, PdfPage } from "@/lib/pdf";
import { PdfPageView } from "./PdfPage";

type Props = {
  doc: PdfDocument;
  scale: number;
  currentPage: number;
  search: string;
  onPageVisible: (page: number) => void;
  scrollNonce: number;
};

export function Viewer({ doc, scale, currentPage, search, onPageVisible, scrollNonce }: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const [pages, setPages] = useState<PdfPage[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPages([]);
    setError(null);
    (async () => {
      try {
        const loaded: PdfPage[] = [];
        for (let i = 1; i <= doc.numPages; i++) {
          const page = await doc.getPage(i);
          if (cancelled) return;
          loaded.push(page);
          if (i === 1 || i % 4 === 0 || i === doc.numPages) {
            setPages([...loaded]);
          }
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Não foi possível ler as páginas.");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [doc]);

  useEffect(() => {
    const root = scroller.current;
    if (!root) return;
    const target = root.querySelector(`[data-page="${currentPage}"]`);
    if (target) {
      target.scrollIntoView({ block: "start" });
    }
  }, [scrollNonce, currentPage]);

  const width = useMemo(() => {
    const first = pages[0];
    if (!first) return 720;
    return first.getViewport({ scale }).width;
  }, [pages, scale]);

  if (error) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-sm text-danger">
        {error}
      </div>
    );
  }

  return (
    <div ref={scroller} className="h-full overflow-auto scrollbar-thin">
      <div
        className="mx-auto flex flex-col items-center gap-7 py-8 pb-16"
        style={{ width: Math.max(width + 64, 280) }}
      >
        {pages.length === 0 && (
          <div className="mt-24 text-[13px] text-quiet">A montar as páginas…</div>
        )}
        {pages.map((page, index) => (
          <PdfPageView
            key={`${doc.fingerprints?.[0] ?? "doc"}-${index}`}
            page={page}
            pageNumber={index + 1}
            scale={scale}
            search={search}
            onVisible={onPageVisible}
          />
        ))}
      </div>
    </div>
  );
}
