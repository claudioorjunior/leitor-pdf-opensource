import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import type { PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

export type PdfDocument = PDFDocumentProxy;
export type PdfPage = PDFPageProxy;

export type PdfMeta = {
  title: string | null;
  author: string | null;
  subject: string | null;
  creator: string | null;
  producer: string | null;
  creationDate: string | null;
  modDate: string | null;
};

export type OutlineItem = {
  title: string;
  pageIndex: number | null;
  items: OutlineItem[];
};

export async function loadPdf(data: Uint8Array): Promise<PdfDocument> {
  const loading = pdfjs.getDocument({
    data: data.slice(),
    useSystemFonts: true,
  });
  return loading.promise;
}

export async function readMetadata(doc: PdfDocument): Promise<PdfMeta> {
  const info = (await doc.getMetadata()).info as Record<string, unknown> | undefined;
  const str = (key: string) => {
    const v = info?.[key];
    return typeof v === "string" && v.trim() ? v : null;
  };
  return {
    title: str("Title"),
    author: str("Author"),
    subject: str("Subject"),
    creator: str("Creator"),
    producer: str("Producer"),
    creationDate: str("CreationDate"),
    modDate: str("ModDate"),
  };
}

export async function readOutline(doc: PdfDocument): Promise<OutlineItem[]> {
  const outline = await doc.getOutline();
  if (!outline) return [];

  const destToPage = async (dest: unknown): Promise<number | null> => {
    try {
      let resolved = dest;
      if (typeof dest === "string") {
        resolved = await doc.getDestination(dest);
      }
      if (!Array.isArray(resolved) || resolved.length === 0) return null;
      const ref = resolved[0] as { num?: number };
      if (!ref || typeof ref !== "object") return null;
      return (await doc.getPageIndex(ref as never)) as number;
    } catch {
      return null;
    }
  };

  const map = async (items: typeof outline): Promise<OutlineItem[]> => {
    const out: OutlineItem[] = [];
    for (const item of items) {
      out.push({
        title: item.title || "Sem título",
        pageIndex: await destToPage(item.dest),
        items: item.items?.length ? await map(item.items) : [],
      });
    }
    return out;
  };

  return map(outline);
}

export async function searchDocument(
  doc: PdfDocument,
  query: string,
): Promise<{ pageIndex: number; count: number }[]> {
  const needle = query.trim().toLocaleLowerCase("pt-BR");
  if (!needle) return [];
  const hits: { pageIndex: number; count: number }[] = [];
  for (let i = 1; i <= doc.numPages; i++) {
    const page = await doc.getPage(i);
    const text = await page.getTextContent();
    const blob = text.items
      .map((item) => ("str" in item ? item.str : ""))
      .join(" ")
      .toLocaleLowerCase("pt-BR");
    let count = 0;
    let from = 0;
    while (from <= blob.length) {
      const at = blob.indexOf(needle, from);
      if (at === -1) break;
      count += 1;
      from = at + needle.length;
    }
    if (count > 0) hits.push({ pageIndex: i - 1, count });
  }
  return hits;
}

export { pdfjs };
