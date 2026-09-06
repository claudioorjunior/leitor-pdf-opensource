import { useEffect, useRef, useState } from "react";
import { TextLayer } from "pdfjs-dist";
import type { PdfPage } from "@/lib/pdf";
import { cn } from "@/lib/utils";

type Props = {
  page: PdfPage;
  pageNumber: number;
  scale: number;
  search: string;
  onVisible?: (pageNumber: number) => void;
};

export function PdfPageView({ page, pageNumber, scale, search, onVisible }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const textRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  const viewport = page.getViewport({ scale });

  useEffect(() => {
    const node = wrapRef.current;
    if (!node || !onVisible) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting && e.intersectionRatio > 0.35)) {
          onVisible(pageNumber);
        }
      },
      { threshold: [0.35] },
    );
    io.observe(node);
    return () => io.disconnect();
  }, [onVisible, pageNumber]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const textLayerDiv = textRef.current;
    if (!canvas || !textLayerDiv) return;

    let cancelled = false;
    const viewport = page.getViewport({ scale });
    const outputScale = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.floor(viewport.width * outputScale);
    canvas.height = Math.floor(viewport.height * outputScale);
    canvas.style.width = `${viewport.width}px`;
    canvas.style.height = `${viewport.height}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const transform = outputScale !== 1 ? [outputScale, 0, 0, outputScale, 0, 0] : undefined;
    const renderTask = page.render({
      canvasContext: ctx,
      viewport,
      canvas,
      transform,
    });

    setReady(false);
    setError(null);
    textLayerDiv.replaceChildren();
    textLayerDiv.style.width = `${viewport.width}px`;
    textLayerDiv.style.height = `${viewport.height}px`;

    let textLayer: TextLayer | null = null;

    Promise.all([
      renderTask.promise,
      page.getTextContent().then((textContent) => {
        if (cancelled) return;
        textLayer = new TextLayer({
          textContentSource: textContent,
          container: textLayerDiv,
          viewport,
        });
        return textLayer.render();
      }),
    ])
      .then(async () => {
        if (cancelled) return;
        setReady(true);
        const annots = await page.getAnnotations();
        if (cancelled || !wrapRef.current) return;
        wrapRef.current.querySelector(".annotationLayer")?.remove();
        const layer = document.createElement("div");
        layer.className = "annotationLayer";
        layer.style.width = `${viewport.width}px`;
        layer.style.height = `${viewport.height}px`;
        for (const annot of annots) {
          if (annot.subtype !== "Link" || !annot.rect) continue;
        const [x1, y1] = viewport.convertToViewportPoint(annot.rect[0], annot.rect[1]);
        const [x2, y2] = viewport.convertToViewportPoint(annot.rect[2], annot.rect[3]);
          const a = document.createElement("a");
          a.href = annot.url || "#";
          a.title = annot.url || "Ligação";
          if (annot.url) {
            a.target = "_blank";
            a.rel = "noreferrer";
          }
          a.style.left = `${Math.min(x1, x2)}px`;
          a.style.top = `${Math.min(y1, y2)}px`;
          a.style.width = `${Math.abs(x2 - x1)}px`;
          a.style.height = `${Math.abs(y2 - y1)}px`;
          layer.appendChild(a);
        }
        wrapRef.current.appendChild(layer);
        highlight(textLayerDiv, search);
      })
      .catch((err: unknown) => {
        const cancelledRender =
          err instanceof Error && /cancel/i.test(err.message);
        if (!cancelled && !cancelledRender) {
          setError(err instanceof Error ? err.message : "Falha ao desenhar a página.");
        }
      });

    return () => {
      cancelled = true;
      renderTask.cancel();
      textLayer?.cancel();
    };
  }, [page, scale, search]);

  return (
    <section
      ref={wrapRef}
      data-page={pageNumber}
      className="page-sheet relative mx-auto overflow-hidden bg-sheet"
      style={{ width: viewport.width, height: viewport.height }}
    >
      {!ready && !error && (
        <div className="absolute inset-0 animate-pulse bg-[linear-gradient(90deg,#efe8dc,#f7f3ec,#efe8dc)] bg-[length:200%_100%]" />
      )}
      {error && (
        <div className="absolute inset-0 flex items-center justify-center bg-sheet px-6 text-center text-sm text-danger">
          {error}
        </div>
      )}
      <canvas ref={canvasRef} className={cn("block", !ready && "opacity-0")} />
      <div ref={textRef} className="textLayer" />
    </section>
  );
}

function highlight(root: HTMLElement, query: string) {
  const needle = query.trim();
  for (const mark of root.querySelectorAll("span.highlight")) {
    mark.classList.remove("highlight");
  }
  if (!needle) return;
  const lower = needle.toLocaleLowerCase("pt-BR");
  for (const span of root.querySelectorAll("span")) {
    if (span.textContent?.toLocaleLowerCase("pt-BR").includes(lower)) {
      span.classList.add("highlight");
    }
  }
}
