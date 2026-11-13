import { useEffect, useRef, useState } from "react";
import { CheckCircle, CircleNotch, WarningCircle, XCircle } from "@phosphor-icons/react";
import { getMediaThumbnail, type DocumentImportResult, type DocumentImportSlide } from "../../lib/commands";
import { useLocale } from "../../i18n";
import { dialogActionButtonStyle, dialogActionFooterStyle } from "../common/dialogActionStyles";

export type DocumentImportReviewResult = Omit<DocumentImportResult, "slides"> & {
  slides: Array<DocumentImportSlide & { document_title: string; document_source: string }>;
};

type Props = {
  pending: boolean;
  result: DocumentImportReviewResult | null;
  error: string | null;
  notice: string | null;
  onClose: () => void;
};

type ThumbnailState = { status: "loading" } | { status: "ready"; dataUrl: string } | { status: "error" };

const dialogStyle: React.CSSProperties = {
  width: "min(1040px, calc(100vw - 32px))", height: "min(90vh, 900px)", maxHeight: "90vh", overflow: "hidden", padding: 20,
  display: "flex", flexDirection: "column", minHeight: 0,
  borderRadius: 8, background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
  boxShadow: "0 12px 36px rgba(0,0,0,0.5)", color: "var(--wc-text)",
};

const failureStyle: React.CSSProperties = { color: "#f87171", display: "flex", gap: 6, alignItems: "flex-start", fontSize: 12, overflowWrap: "anywhere" };
const warningStyle: React.CSSProperties = { color: "#fbbf24", display: "flex", gap: 6, alignItems: "flex-start", fontSize: 12, overflowWrap: "anywhere" };

export function DocumentImportReview({ pending, result, error, notice, onClose }: Props) {
  const { t } = useLocale();
  const [thumbnails, setThumbnails] = useState<Record<string, ThumbnailState>>({});
  const dialogRef = useRef<HTMLElement>(null);
  const requestedThumbnailPaths = useRef(new Set<string>());
  const thumbnailQueue = useRef<string[]>([]);
  const activeThumbnailRequests = useRef(0);
  const pumpThumbnailQueue = useRef<() => void>(() => undefined);
  const modalOpen = !pending && (Boolean(result) || Boolean(error));
  const modalOpenRef = useRef(modalOpen);
  const mountedRef = useRef(false);
  modalOpenRef.current = modalOpen;
  pumpThumbnailQueue.current = () => {
    while (mountedRef.current && modalOpenRef.current && activeThumbnailRequests.current < 4 && thumbnailQueue.current.length > 0) {
      const path = thumbnailQueue.current.shift()!;
      activeThumbnailRequests.current++;
      void getMediaThumbnail(path, false)
        .then((dataUrl) => {
          if (mountedRef.current) setThumbnails((current) => ({ ...current, [path]: { status: "ready", dataUrl } }));
        })
        .catch(() => {
          if (mountedRef.current) setThumbnails((current) => ({ ...current, [path]: { status: "error" } }));
        })
        .finally(() => {
          activeThumbnailRequests.current--;
          if (mountedRef.current && modalOpenRef.current) pumpThumbnailQueue.current();
        });
    }
  };

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      for (const path of thumbnailQueue.current.splice(0)) requestedThumbnailPaths.current.delete(path);
    };
  }, []);

  useEffect(() => {
    if (!modalOpen) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialogRef.current?.focus();
    return () => previous?.focus();
  }, [modalOpen]);

  useEffect(() => {
    if (!modalOpen) {
      for (const path of thumbnailQueue.current.splice(0)) requestedThumbnailPaths.current.delete(path);
      return;
    }
    const paths = [...new Set(result?.slides.map((slide) => slide.image_path) ?? [])]
      .filter((path) => !requestedThumbnailPaths.current.has(path));
    if (paths.length === 0) {
      pumpThumbnailQueue.current();
      return;
    }
    paths.forEach((path) => requestedThumbnailPaths.current.add(path));
    setThumbnails((current) => ({ ...current, ...Object.fromEntries(paths.map((path) => [path, { status: "loading" as const }])) }));
    thumbnailQueue.current.push(...paths);
    pumpThumbnailQueue.current();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modalOpen, result?.slides]);

  if (pending) return (
    <div role="status" style={{ position: "fixed", right: 18, bottom: 58, zIndex: 11000, display: "flex", alignItems: "center", gap: 8, padding: "9px 13px", borderRadius: 6, background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", color: "var(--wc-text)", fontSize: 12 }}>
      <CircleNotch size={16} className="spin" />{t("presentationUi.preparing")}
    </div>
  );
  if (!modalOpen) return null;
  const onDialogKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      if (!pending) onClose();
      return;
    }
    if (event.key === "Tab") {
      const focusable = [...(dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])') ?? [])];
      if (focusable.length === 0) {
        event.preventDefault();
        dialogRef.current?.focus();
      } else if (event.shiftKey && (document.activeElement === focusable[0] || document.activeElement === dialogRef.current)) {
        event.preventDefault();
        focusable[focusable.length - 1].focus();
      } else if (!event.shiftKey && document.activeElement === focusable[focusable.length - 1]) {
        event.preventDefault();
        focusable[0].focus();
      }
    }
  };
  return (
    <div role="presentation" style={{ position: "fixed", inset: 0, zIndex: 12000, display: "grid", placeItems: "center", background: "rgba(0,0,0,0.62)" }}>
      <section ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="presentation-import-title" tabIndex={-1} onKeyDown={onDialogKeyDown} style={dialogStyle}>
        <header style={{ flex: "0 0 auto" }}>
          <h2 id="presentation-import-title" style={{ margin: "0 0 10px", fontSize: 16 }}>{t("presentationUi.importReview")}</h2>
          {result && <p style={{ margin: "0 0 12px", color: "var(--wc-text-secondary)", fontSize: 13 }}>
            {t("presentationUi.importSummary", { pages: result.page_count, videos: result.video_count })}
          </p>}
        </header>
        <div style={{ flex: "1 1 auto", minHeight: 0, overflowY: "auto", overflowX: "hidden", paddingRight: 4, overflowWrap: "anywhere" }}>
          {pending && !result && <p role="status" style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--wc-text-secondary)" }}><CircleNotch size={16} className="spin" />{t("presentationUi.preparing")}</p>}
          {notice && <p role="status" style={{ ...warningStyle, margin: "0 0 12px", overflowWrap: "anywhere" }}><WarningCircle size={16} style={{ flexShrink: 0 }} />{notice}</p>}
          {error && <p role="alert" style={{ ...failureStyle, margin: "0 0 12px", overflowWrap: "anywhere" }}><XCircle size={16} style={{ flexShrink: 0 }} />{t("presentationUi.importFailed", { error })}</p>}
          {result?.media_scan_error && <p role="alert" style={{ ...failureStyle, margin: "0 0 12px", padding: 10, border: "1px solid rgba(248,113,113,.45)", borderRadius: 6, overflowWrap: "anywhere" }}><XCircle size={16} style={{ flexShrink: 0 }} />{t("presentationUi.mediaScanFailed", { error: result.media_scan_error })}</p>}
          {result?.slides.length ? <div style={{ display: "grid", gridTemplateColumns: "repeat(3, minmax(0, 1fr))", gap: 12 }}>
            {result.slides.map((slide) => <SlideCard key={`${slide.image_cue_id}-${slide.image_path}`} slide={slide} thumbnail={thumbnails[slide.image_path]} />)}
          </div> : null}
          {result?.warnings.length ? <div style={{ marginTop: 16 }}>
            <h3 style={{ margin: "0 0 8px", fontSize: 13, color: "var(--wc-text-secondary)" }}>{t("presentationUi.importNotes")}</h3>
            <ul style={{ margin: 0, paddingLeft: 22, fontSize: 12, lineHeight: 1.5 }}>{result.warnings.map((warning, index) => <li key={`${index}-${warning}`} style={{ overflowWrap: "anywhere" }}>{warning}</li>)}</ul>
          </div> : null}
        </div>
        <footer style={{ ...dialogActionFooterStyle, flex: "0 0 auto", marginTop: 12 }}>
          <button type="button" onClick={onClose} style={dialogActionButtonStyle}>{t("common.close")}</button>
        </footer>
      </section>
    </div>
  );
}

function SlideCard({ slide, thumbnail }: { slide: DocumentImportSlide & { document_title: string; document_source: string }; thumbnail?: ThumbnailState }) {
  const { t } = useLocale();
  return (
    <article style={{ minWidth: 0, padding: 10, borderRadius: 6, background: "var(--wc-bg-base)", border: "1px solid var(--wc-border-strong)" }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 8, marginBottom: 8 }}>
        <strong style={{ fontSize: 12 }}>{t("presentationUi.slideTitle", { number: slide.number })}</strong>
        <span title={`${slide.document_title} — ${slide.document_source}`} style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", overflowWrap: "anywhere", color: "var(--wc-text-muted)", fontSize: 11 }}>{slide.document_source.split(/[\\/]/).pop() || slide.document_source}</span>
      </div>
      <div style={{ position: "relative", minHeight: 0, aspectRatio: "16 / 9", display: "grid", placeItems: "center", overflow: "hidden", borderRadius: 4, background: "#111", marginBottom: 8 }}>
        {thumbnail?.status === "ready" ? <img src={thumbnail.dataUrl} alt={t("presentationUi.slideTitle", { number: slide.number })} loading="lazy" style={{ position: "absolute", inset: 0, display: "block", width: "100%", height: "100%", minWidth: 0, minHeight: 0, objectFit: "contain" }} /> :
          thumbnail?.status === "error" ? <span role="alert" style={{ ...failureStyle, padding: 8, overflowWrap: "anywhere" }}><XCircle size={15} style={{ flexShrink: 0 }} />{t("presentationUi.thumbnailFailed")}</span> :
            <span role="status" style={{ color: "var(--wc-text-muted)", fontSize: 12 }}>{t("presentationUi.thumbnailLoading")}</span>}
      </div>
      {slide.videos.map((video, index) => <div key={`${video.name}-${index}`} style={{ marginTop: 6 }}>
        {video.status === "added" ? <div style={{ display: "flex", gap: 6, alignItems: "flex-start", color: "var(--wc-text-secondary)", fontSize: 12 }}>
          <CheckCircle size={15} color="#4ade80" style={{ flexShrink: 0 }} />
          <span>{t("presentationUi.videoAdded", { name: video.name })}{video.warning && <span style={{ ...warningStyle, marginTop: 3 }}>{video.warning}</span>}</span>
        </div> : <div style={failureStyle}>
          <XCircle size={15} style={{ flexShrink: 0 }} />
          <span>{t("presentationUi.videoNotAdded", { name: video.name, error: video.error ?? t("presentationUi.videoUnavailable") })}</span>
        </div>}
      </div>)}
    </article>
  );
}
