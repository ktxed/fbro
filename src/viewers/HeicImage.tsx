import { useEffect, useState } from "react";
import { ZoomableImage } from "./ZoomableImage";
import { ViewerLoading } from "./ViewerLoading";

interface Props {
  filename: string;
  source?: string;
  buffer?: Uint8Array | null;
  maxBytes: number;
}

export function HeicImage({ filename, source, buffer, maxBytes }: Props) {
  const [result, setResult] = useState<{ source?: string; buffer?: Uint8Array | null; maxBytes: number; url?: string; error?: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | undefined;
    const abort = new AbortController();
    async function convert() {
      try {
        let blob: Blob;
        if (source) {
          const response = await fetch(source, { signal: abort.signal });
          if (!response.ok) throw new Error(`Could not read image (${response.status})`);
          if (Number(response.headers.get("content-length")) > maxBytes) {
            abort.abort();
            throw new Error("Image exceeds the preview size limit");
          }
          blob = await response.blob();
        } else if (buffer) {
          blob = new Blob([buffer.slice().buffer as ArrayBuffer]);
        } else return;
        if (cancelled) return;
        if (blob.size > maxBytes) throw new Error("Image exceeds the preview size limit");
        // Load the bundled decoder only when an opted-in preview needs it.
        const { default: heic2any } = await import("heic2any");
        if (cancelled) return;
        let converted: Blob | Blob[];
        try {
          converted = await heic2any({ blob, toType: "image/png" });
        } catch (cause) {
          if (cancelled) return;
          // HEIC2ANY's older libheif rejects newer iPhone containers. Keep
          // the same local preview flow with an up-to-date browser decoder.
          const message = typeof cause === "object" && cause !== null && "message" in cause
            ? String(cause.message) : "";
          if (!message.startsWith("ERR_LIBHEIF")) throw cause;
          const { heicTo } = await import("heic-to");
          if (cancelled) return;
          converted = await heicTo({ blob, type: "image/png" });
        }
        if (cancelled) return;
        const first = Array.isArray(converted) ? converted[0] : converted;
        if (!first) throw new Error("Decoder returned no image");
        objectUrl = URL.createObjectURL(first);
        setResult({ source, buffer, maxBytes, url: objectUrl });
      } catch (cause) {
        if (cancelled) return;
        const message = cause instanceof Error ? cause.message
          : typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message)
          : "HEIC conversion failed";
        setResult({ source, buffer, maxBytes, error: message });
      }
    }
    void convert();
    return () => {
      cancelled = true;
      abort.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [source, buffer, maxBytes]);

  const current = result?.source === source && result?.buffer === buffer && result?.maxBytes === maxBytes ? result : null;
  if (current?.url) return <ZoomableImage src={current.url} alt={filename} />;
  return <div className="viewer__image-container" data-testid="heic-preview">
    {current?.error
      ? <div className="viewer__image-notice" role="alert">Unable to preview {filename}: {current.error}</div>
      : <ViewerLoading />}
  </div>;
}
