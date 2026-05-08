import { useEffect, useState } from "react";
import { ipc } from "../ipc";
import { Icon } from "./Icon";

// Module-level cache so the same thumbnail isn't re-fetched when the
// list re-renders. Keyed by document id; value is the data-URL or
// `false` if the document has no thumbnail at all.
const cache = new Map<string, string | false>();
const inflight = new Map<string, Promise<string | false>>();

async function fetchThumbnail(documentId: string): Promise<string | false> {
  const cached = cache.get(documentId);
  if (cached !== undefined) return cached;
  const existing = inflight.get(documentId);
  if (existing) return existing;
  const p = (async () => {
    try {
      const url = await ipc.documentThumbnail(documentId);
      const v: string | false = url ?? false;
      cache.set(documentId, v);
      return v;
    } catch {
      cache.set(documentId, false);
      return false as const;
    } finally {
      inflight.delete(documentId);
    }
  })();
  inflight.set(documentId, p);
  return p;
}

export function invalidateThumbnail(documentId: string) {
  cache.delete(documentId);
}

interface Props {
  documentId: string;
  docType: string;
  size?: "tile" | "preview";
}

/* Renders a document's first-page thumbnail or a tasteful fallback
 * glyph that matches the document type. Loads asynchronously and
 * fades in when ready. */
export function Thumbnail({ documentId, docType, size = "tile" }: Props) {
  const [src, setSrc] = useState<string | false | null>(() => {
    const c = cache.get(documentId);
    return c === undefined ? null : c;
  });
  useEffect(() => {
    let live = true;
    if (cache.has(documentId)) {
      setSrc(cache.get(documentId)!);
      return;
    }
    setSrc(null);
    fetchThumbnail(documentId).then((v) => {
      if (live) setSrc(v);
    });
    return () => {
      live = false;
    };
  }, [documentId]);

  const dim = size === "preview" ? { width: 200, height: 268 } : { width: 138, height: 184 };

  if (typeof src === "string" && src) {
    return (
      <span className={`thumb thumb-${size}`} style={dim}>
        <img src={src} alt="" loading="lazy" />
      </span>
    );
  }
  return (
    <span className={`thumb thumb-${size} thumb-fallback`} style={dim}>
      <Icon name={fallbackIcon(docType)} size={size === "preview" ? 36 : 28} />
    </span>
  );
}

function fallbackIcon(docType: string) {
  if (docType === "Notebook") return "notebook" as const;
  if (docType === "DocumentType.Pdf") return "pdf" as const;
  if (docType === "DocumentType.Epub") return "epub" as const;
  return "library" as const;
}
