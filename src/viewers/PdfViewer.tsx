import { useEffect, useRef, useState } from "react";
import type { PDFDocumentProxy, RenderTask } from "pdfjs-dist/types/src/display/api";
import pdfWorkerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import { ViewerLoading } from "./ViewerLoading";

type PdfJs = typeof import("pdfjs-dist/legacy/build/pdf.mjs");

let pdfJsPromise: Promise<PdfJs> | null = null;

function loadPdfJs(): Promise<PdfJs> {
  if (!pdfJsPromise) {
    pdfJsPromise = import("pdfjs-dist/legacy/build/pdf.mjs").then((pdfjs) => {
      pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;
      return pdfjs;
    });
  }
  return pdfJsPromise;
}

interface PdfPageProps {
  document: PDFDocumentProxy;
  pageNumber: number;
  availableWidth: number;
}

function PdfPage({ document, pageNumber, availableWidth }: PdfPageProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [visible, setVisible] = useState(false);
  const [aspectRatio, setAspectRatio] = useState(1 / Math.sqrt(2));
  const [error, setError] = useState(false);
  const [rendered, setRendered] = useState(false);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    }, { rootMargin: "800px 0px" });
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible || availableWidth <= 0) return;
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    setRendered(false);

    document.getPage(pageNumber).then((page) => {
      if (cancelled) return;
      const natural = page.getViewport({ scale: 1 });
      setAspectRatio(natural.width / natural.height);
      const cssWidth = Math.min(availableWidth, natural.width * 2);
      const viewport = page.getViewport({ scale: cssWidth / natural.width });
      const outputScale = Math.min(window.devicePixelRatio || 1, 2);
      const canvas = canvasRef.current;
      if (!canvas) return;

      canvas.width = Math.max(1, Math.floor(viewport.width * outputScale));
      canvas.height = Math.max(1, Math.floor(viewport.height * outputScale));
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;
      renderTask = page.render({
        canvas,
        viewport,
        transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0],
      });
      return renderTask.promise;
    }).then(() => {
      if (!cancelled) {
        setError(false);
        setRendered(true);
      }
    }).catch((cause: unknown) => {
      if (!cancelled && (cause as { name?: string })?.name !== "RenderingCancelledException") {
        setError(true);
      }
    });

    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
  }, [availableWidth, document, pageNumber, visible]);

  return (
    <div
      ref={hostRef}
      className="viewer__pdf-page"
      style={{ aspectRatio }}
      aria-label={`Page ${pageNumber}`}
      aria-busy={!rendered && !error}
      data-rendered={rendered || undefined}
    >
      {error
        ? <div className="viewer__pdf-page-error" role="alert">Unable to render page {pageNumber}</div>
        : <canvas ref={canvasRef} />}
    </div>
  );
}

interface PdfViewerProps {
  url: string;
  filename: string;
}

export function PdfViewer({ url, filename }: PdfViewerProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [availableWidth, setAvailableWidth] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => setAvailableWidth(Math.max(1, container.clientWidth - 32));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    let cancelled = false;
    let loadingTask: ReturnType<PdfJs["getDocument"]> | null = null;
    setDocument(null);
    setError(null);
    if (containerRef.current) containerRef.current.scrollTop = 0;

    loadPdfJs().then((pdfjs) => {
      if (cancelled) return;
      loadingTask = pdfjs.getDocument({ url });
      return loadingTask.promise;
    }).then((loaded) => {
      if (!cancelled && loaded) setDocument(loaded);
    }).catch((cause: unknown) => {
      if (!cancelled) {
        const detail = cause instanceof Error ? cause.message : String(cause);
        setError(`Unable to preview ${filename}: ${detail}`);
      }
    });

    return () => {
      cancelled = true;
      void loadingTask?.destroy();
    };
  }, [filename, url]);

  return (
    <div ref={containerRef} className="viewer__pdf" tabIndex={0} aria-label={filename} aria-busy={!document && !error}>
      {!document && !error && <ViewerLoading />}
      {error && <div className="viewer__pdf-error" role="alert">{error}</div>}
      {document && Array.from({ length: document.numPages }, (_, index) => (
        <PdfPage
          key={index + 1}
          document={document}
          pageNumber={index + 1}
          availableWidth={availableWidth}
        />
      ))}
    </div>
  );
}
