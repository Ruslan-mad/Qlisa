import { useEffect, useMemo, useState } from "react";
import type { CueSummary, MediaConversionImageFormat, MediaConversionJob, MediaConversionMode, MediaConversionQuality, MediaConversionResolution, MediaInfo } from "../../lib/types";
import { cancelMediaConversion, getMediaConversionStatus, getMediaInfo, openMediaOutputFolder, replaceCueMedia, restoreCueMedia, startMediaConversion } from "../../lib/commands";
import { useLocale } from "../../i18n";
import { useMediaConversionStore } from "../../stores/mediaConversionStore";
import { conversionProgress, formatMediaBytes, formatMediaDuration, mediaInfoFromCue } from "./mediaConversionModel";

export function MediaConverterDialog({
  cue,
  cues,
  infoOnly = false,
  onClose,
  onRefresh,
  initialJob = null,
  initialJobs = [],
}: {
  cue: CueSummary;
  /** Additional selected media cues. The first cue remains the primary cue. */
  cues?: CueSummary[];
  infoOnly?: boolean;
  onClose: () => void;
  onRefresh: () => void;
  initialJob?: MediaConversionJob | null;
  initialJobs?: MediaConversionJob[];
}) {
  const { t } = useLocale();
  const upsert = useMediaConversionStore((state) => state.upsert);
  const [info, setInfo] = useState<MediaInfo>(() => mediaInfoFromCue(cue));
  const [mode, setMode] = useState<MediaConversionMode>("optimize");
  const [quality, setQuality] = useState<MediaConversionQuality>("medium");
  const [resolution, setResolution] = useState<MediaConversionResolution>("original");
  const [imageFormat, setImageFormat] = useState<MediaConversionImageFormat>(() => {
    const extension = cue.file_path?.split(/[\\/.]/).pop()?.toLowerCase();
    return extension === "jpg" || extension === "jpeg" ? "jpg" : "png";
  });
  const selectedCues = useMemo(() => {
    const all = cues?.length ? cues : [cue];
    return all.filter((item, index) => all.findIndex((candidate) => candidate.id === item.id) === index);
  }, [cue, cues]);
  const isBatch = selectedCues.length > 1;
  const [job, setJob] = useState<MediaConversionJob | null>(initialJob);
  const [batchJobs, setBatchJobs] = useState<MediaConversionJob[]>(() => initialJobs);
  const [loading, setLoading] = useState(!infoOnly);
  const [error, setError] = useState<string | null>(null);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const jobs = useMemo(() => isBatch ? batchJobs : (job ? [job] : []), [batchJobs, isBatch, job]);
  const busy = jobs.some((item) => item.status === "queued" || item.status === "running");
  const completed = !isBatch && job?.status === "completed";
  const batchCompleted = isBatch && jobs.length === selectedCues.length && jobs.every((item) => item.status === "completed");
  const batchFinished = isBatch && jobs.length === selectedCues.length && !busy;
  const batchHasCompleted = isBatch && jobs.some((item) => item.status === "completed" && !!item.output_path);
  const title = infoOnly ? t("mediaConversion.infoTitle") : t("mediaConversion.title");
  const isImage = cue.cue_type === "image";
  const isAudio = cue.cue_type === "audio";
  const isVideo = cue.cue_type === "video";

  useEffect(() => {
    if (initialJob) {
      setJob(initialJob);
      void getMediaConversionStatus(initialJob.id).then((next) => { setJob(next); upsert(next); }).catch(() => undefined);
    }
  }, [initialJob?.id, upsert]);

  const initialJobSignature = initialJobs.map((item) => `${item.id}:${item.status}:${item.applied_to_cue ? 1 : 0}`).join(",");
  useEffect(() => {
    if (!initialJobs.length) return;
    setBatchJobs(initialJobs);
    void Promise.all(initialJobs.map((item) => getMediaConversionStatus(item.id).catch(() => item)))
      .then((next) => { setBatchJobs(next); next.forEach(upsert); });
  }, [initialJobSignature, upsert]);

  useEffect(() => {
    let active = true;
    if (!cue.file_path) {
      setLoading(false);
      return () => { active = false; };
    }
    getMediaInfo(cue.id)
      .then((next) => { if (active) setInfo({ ...next, source_size_bytes: cue.file_size_bytes ?? next.source_size_bytes }); })
      .catch(() => { /* cue summary remains useful when backend probing is unavailable */ })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [cue.id, infoOnly]);

  useEffect(() => {
    if (!jobs.length || !busy) return;
    const timer = window.setInterval(() => {
      void Promise.all(jobs.map((item) => getMediaConversionStatus(item.id)))
        .then((next) => {
          if (isBatch) setBatchJobs(next);
          else setJob(next[0] ?? null);
          next.forEach(upsert);
        })
        .catch((reason) => setError(String(reason)));
    }, 500);
    return () => window.clearInterval(timer);
  }, [jobs, busy, isBatch, upsert]);

  useEffect(() => {
    if (!busy) return;
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, [busy]);

  const resolutionOptions = useMemo(() => [
    ["original", t("mediaConversion.original")],
    ["1080p", "1080p"],
    ["720p", "720p"],
  ] as const, [t]);
  const compatibilityState = loading || !info.compatibility_known ? "unknown" : info.compatibility_status === "recommended" ? "recommended" : info.compatible ? "compatible" : "unsupported";
  const elapsedMs = startedAt == null ? 0 : Math.max(0, now - startedAt);
  const progress = isBatch
    ? (jobs.length ? jobs.reduce((sum, item) => sum + conversionProgress(item), 0) / selectedCues.length : 0)
    : conversionProgress(job ?? undefined);
  const displayElapsedMs = job?.processed != null ? job.processed * 1000 : elapsedMs;
  const displayTotalMs = job?.total != null ? job.total * 1000 : info.duration_ms;
  const sourceSize = job?.source_size ?? info.source_size_bytes;
  const speed = job?.speed != null ? `${job.speed.toFixed(1)}×` : elapsedMs > 0 && progress > 0 && info.duration_ms
    ? `${(progress * info.duration_ms / elapsedMs).toFixed(1)}×`
    : "—";

  const start = async () => {
    setError(null);
    try {
      const qualityLevel = quality === "high" ? 1 : quality === "low" ? 3 : 2;
      const maxResolution = resolution === "original" ? null : Number.parseInt(resolution, 10);
      const requests = selectedCues.filter((item) => item.file_path).map((item) => startMediaConversion({
        cue_id: item.id,
        input_path: item.file_path ?? "",
        mode,
        quality: mode === "size" ? qualityLevel : null,
        max_resolution: item.cue_type === "image" || mode === "size" ? maxResolution : null,
        image_format: item.cue_type === "image" ? imageFormat : null,
      }));
      const results = await Promise.allSettled(requests);
      const nextJobs = results.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
      const failures = results.filter((result): result is PromiseRejectedResult => result.status === "rejected");
      if (failures.length) setError(`${failures.length} ${failures.length === 1 ? "cue" : "cue"} не удалось поставить в очередь.`);
      nextJobs.forEach(upsert);
      if (isBatch) setBatchJobs(nextJobs);
      else setJob(nextJobs[0] ?? null);
      setStartedAt(Date.now());
      setNow(Date.now());
    } catch (reason) {
      setError(String(reason));
    }
  };

  const cancel = async () => {
    if (!jobs.length) return;
    try {
      await Promise.all(jobs.filter((item) => item.status === "queued" || item.status === "running").map((item) => cancelMediaConversion(item.id)));
      const next = jobs.map((item) => item.status === "queued" || item.status === "running" ? { ...item, status: "cancelled" as const } : item);
      next.forEach(upsert);
      if (isBatch) setBatchJobs(next); else setJob(next[0] ?? null);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const replace = async () => {
    const replaceable = jobs.filter((item) => item.output_path && item.status === "completed");
    if (!replaceable.length) return;
    setError(null);
    try {
      await Promise.all(replaceable.map((item) => replaceCueMedia(item.id)));
      replaceable.forEach((item) => upsert({ ...item, applied_to_cue: true }));
      onRefresh();
      onClose();
    } catch (reason) {
      setError(String(reason));
    }
  };

  const restore = async () => {
    const restorable = jobs.filter((item) => item.status === "completed" && item.applied_to_cue);
    if (!restorable.length) return;
    setError(null);
    try {
      await Promise.all(restorable.map((item) => restoreCueMedia(item.id)));
      restorable.forEach((item) => upsert({ ...item, applied_to_cue: false }));
      onRefresh(); onClose();
    }
    catch (reason) { setError(String(reason)); }
  };

  return (
    <div role="dialog" aria-modal="true" aria-label={title} style={overlay}>
      <div style={dialog}>
        <div style={header}>
          <div style={{ minWidth: 0 }}>
            <div style={titleStyle}>{title}</div>
            <div style={fileName} title={isBatch ? selectedCues.map((item) => item.name).join(", ") : info.path}>{isBatch ? `${selectedCues.length} cue` : info.path.split(/[\\/]/).pop() ?? info.path}</div>
            <div style={muted}>{isBatch ? selectedCues.map((item) => item.name || item.file_path?.split(/[\\/]/).pop()).filter(Boolean).join(", ") : cue.name}</div>
          </div>
          <button type="button" onClick={onClose} style={closeButton} aria-label={t("common.close")}>×</button>
        </div>

        <section style={section}>
          <div style={sectionTitle}>{t("mediaConversion.metadata")}</div>
          {isBatch ? <div style={muted}>{selectedCues.length} cue выбрано. Все подходящие файлы будут поставлены в очередь.</div> : loading ? <div style={muted}>{t("status.loading")}</div> : (
            <div style={metaGrid}>
              <Meta label={t("mediaConversion.format")} value={info.format ?? "—"} />
              {isVideo && <Meta label={t("mediaConversion.videoCodec")} value={info.video_codec ?? "—"} />}
              {(isAudio || isVideo) && <Meta label={t("mediaConversion.audioCodec")} value={info.audio_codec ?? "—"} />}
              {(isAudio || isVideo) && <Meta label={t("cueList.duration")} value={formatMediaDuration(info.duration_ms)} />}
              {(isImage || isVideo) && <Meta label={t("mediaConversion.resolution")} value={info.width && info.height ? `${info.width}×${info.height}` : "—"} />}
              {isVideo && <Meta label={t("mediaConversion.fps")} value={info.frame_rate ? `${info.frame_rate.toFixed(2)} fps` : "—"} />}
              {(isImage || isAudio || isVideo) && <Meta label={t("mediaConversion.sourceSize")} value={formatMediaBytes(sourceSize)} />}
              {(isAudio || isVideo) && <Meta label={t("mediaConversion.channels")} value={info.audio_channels?.toString() ?? "—"} />}
            </div>
          )}
          <div style={{ marginTop: 9, color: compatibilityState === "compatible" ? "#86efac" : compatibilityState === "unknown" ? "var(--wc-text-muted)" : compatibilityState === "unsupported" ? "#fca5a5" : "#fbbf24", fontSize: 12 }}>
            {compatibilityState === "unknown" ? `? ${t("mediaConversion.compatibilityUnknown")}`
              : compatibilityState === "compatible" ? `✓ ${t("mediaConversion.compatible")}`
              : compatibilityState === "recommended" ? `⚠ ${t("mediaConversion.compatibilityRecommended")}`
              : `⛔ ${t("mediaConversion.compatibilityUnsupported")}`}
          </div>
        </section>

        {!infoOnly && !jobs.length && (
          <section style={section}>
            <div style={sectionTitle}>{t("mediaConversion.mode")}</div>
            <div style={{ display: "flex", gap: 6 }}>
              <ModeButton active={mode === "optimize"} onClick={() => setMode("optimize")}>{t("mediaConversion.optimize")}</ModeButton>
              <ModeButton active={mode === "size"} onClick={() => setMode("size")}>{t("mediaConversion.size")}</ModeButton>
            </div>
            {isImage ? <div style={formGrid}>
              <label style={label}>{t("mediaConversion.imageFormat")}
                <select value={imageFormat} onChange={(event) => setImageFormat(event.target.value as MediaConversionImageFormat)} style={input}>
                  <option value="png">PNG</option>
                  <option value="jpg">JPEG</option>
                </select>
              </label>
              <label style={label}>{t("mediaConversion.resolution")}
                <select value={resolution} onChange={(event) => setResolution(event.target.value as MediaConversionResolution)} style={input}>
                  {resolutionOptions.map(([value, labelText]) => <option key={value} value={value}>{labelText}</option>)}
                </select>
              </label>
            </div> : mode === "size" && <div style={formGrid}>
              <label style={label}>{t("mediaConversion.quality")}
                <select value={quality} onChange={(event) => setQuality(event.target.value as MediaConversionQuality)} style={input}>
                  <option value="high">{t("mediaConversion.high")}</option>
                  <option value="medium">{t("mediaConversion.optimal")}</option>
                  <option value="low">{t("mediaConversion.compact")}</option>
                </select>
              </label>
              <label style={label}>{t("mediaConversion.resolution")}
                <select value={resolution} onChange={(event) => setResolution(event.target.value as MediaConversionResolution)} style={input}>
                  {resolutionOptions.map(([value, labelText]) => <option key={value} value={value}>{labelText}</option>)}
                </select>
              </label>
            </div>}
          </section>
        )}

        {jobs.length > 0 && (
          <section style={section}>
            <div style={{ display: "flex", justifyContent: "space-between", gap: 8, fontSize: 12 }}>
              <span>{isBatch ? `${jobs.filter((item) => item.status === "completed").length}/${selectedCues.length} ${t("mediaConversion.completed").toLowerCase()}` : job?.status === "completed" ? t("mediaConversion.completed") : job?.status === "failed" ? t("mediaConversion.failed") : job?.status === "cancelled" ? t("mediaConversion.cancelled") : t("mediaConversion.inProgress")}</span>
              <span>{Math.round(progress * 100)}%</span>
            </div>
            <div style={progressTrack}><div style={{ ...progressFill, width: `${progress * 100}%` }} /></div>
            {isBatch && <div style={{ ...batchList, maxHeight: 120, overflowY: "auto" }}>{selectedCues.map((item) => {
              const itemJob = jobs.find((candidate) => candidate.cue_id === item.id);
              return <div key={item.id} style={batchRow}><span>{item.name || item.file_path?.split(/[\\/]/).pop() || item.id}</span><span>{itemJob?.status ?? "—"}</span></div>;
            })}</div>}
            {busy && !isBatch && <div style={runStats}><span>{formatMediaDuration(displayElapsedMs)} / {formatMediaDuration(displayTotalMs)}</span><span>{t("mediaConversion.speed")}: {speed}</span></div>}
            {(completed || batchCompleted) && !isBatch && <div style={metaGrid}>
              <Meta label={t("mediaConversion.sourceSize")} value={formatMediaBytes(sourceSize)} />
              <Meta label={t("mediaConversion.newSize")} value={formatMediaBytes(job?.new_size)} />
              <Meta label={t("mediaConversion.saving")} value={sourceSize && job?.new_size ? `${Math.max(0, Math.round((1 - job.new_size / sourceSize) * 100))}%` : "—"} />
            </div>}
            {!isBatch && job?.error && <div style={errorText}>{job.error}</div>}
          </section>
        )}

        {error && <div role="alert" style={errorText}>{error}</div>}
        <div style={footer}>
          <button type="button" onClick={onClose} style={secondary}>{busy ? t("mediaConversion.background") : t("common.close")}</button>
          {!infoOnly && !jobs.length && <button type="button" onClick={() => void start()} style={primary}>{t("mediaConversion.start")}</button>}
          {busy && <button type="button" onClick={() => void cancel()} style={danger}>{t("mediaConversion.cancel")}</button>}
          {(completed || batchFinished) && jobs.some((item) => item.output_path) && <button type="button" onClick={() => void openMediaOutputFolder((jobs.find((item) => item.output_path) ?? jobs[0]).id)} style={secondary}>{t("mediaConversion.openFolder")}</button>}
          {jobs.some((item) => item.status === "completed" && item.applied_to_cue) && <button type="button" onClick={() => void restore()} style={secondary}>{t("mediaConversion.restoreOriginal")}{isBatch ? ` (${jobs.filter((item) => item.status === "completed" && item.applied_to_cue).length})` : ""}</button>}
          {(completed || (batchFinished && batchHasCompleted)) && <button type="button" onClick={() => void replace()} style={primary}>{isBatch ? `${t("mediaConversion.replace")} (${jobs.filter((item) => item.status === "completed" && item.output_path).length})` : t("mediaConversion.replace")}</button>}
        </div>
      </div>
    </div>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return <div><div style={{ color: "var(--wc-text-faint)", fontSize: 10 }}>{label}</div><div style={{ color: "var(--wc-text)", fontSize: 12 }}>{value}</div></div>;
}

function ModeButton({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return <button type="button" onClick={onClick} style={{ ...secondary, ...(active ? activeMode : {}) }}>{children}</button>;
}

const overlay: React.CSSProperties = { position: "fixed", inset: 0, zIndex: 10000, background: "rgba(0,0,0,.62)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 };
const dialog: React.CSSProperties = { width: 430, maxWidth: "100%", maxHeight: "calc(100vh - 32px)", overflowY: "auto", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 8, boxShadow: "0 18px 60px rgba(0,0,0,.65)", padding: 14 };
const header: React.CSSProperties = { display: "flex", justifyContent: "space-between", gap: 12, marginBottom: 12 };
const titleStyle: React.CSSProperties = { color: "var(--wc-text-bright)", fontSize: 14, fontWeight: 600 };
const fileName: React.CSSProperties = { color: "var(--wc-text)", fontSize: 12, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
const muted: React.CSSProperties = { color: "var(--wc-text-muted)", fontSize: 11, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
const closeButton: React.CSSProperties = { width: 26, height: 26, color: "var(--wc-text)", background: "transparent", border: "1px solid var(--wc-border-strong)", borderRadius: 4, cursor: "pointer", fontSize: 18 };
const section: React.CSSProperties = { borderTop: "1px solid var(--wc-border)", paddingTop: 10, marginTop: 10 };
const sectionTitle: React.CSSProperties = { color: "var(--wc-text)", fontSize: 11, fontWeight: 600, marginBottom: 8 };
const metaGrid: React.CSSProperties = { display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: "8px 12px" };
const formGrid: React.CSSProperties = { display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, marginTop: 10 };
const runStats: React.CSSProperties = { display: "flex", justifyContent: "space-between", gap: 8, marginTop: 7, color: "var(--wc-text-muted)", fontSize: 10 };
const batchList: React.CSSProperties = { marginTop: 8, borderTop: "1px solid var(--wc-border)", color: "var(--wc-text-muted)", fontSize: 10 };
const batchRow: React.CSSProperties = { display: "flex", justifyContent: "space-between", gap: 8, padding: "4px 0", borderBottom: "1px solid var(--wc-border)" };
const label: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 4, color: "var(--wc-text-muted)", fontSize: 11 };
const input: React.CSSProperties = { width: "100%", boxSizing: "border-box", color: "var(--wc-text)", background: "var(--wc-bg-deepest)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, padding: "5px 6px" };
const footer: React.CSSProperties = { display: "flex", justifyContent: "flex-end", gap: 6, marginTop: 14 };
const secondary: React.CSSProperties = { color: "var(--wc-text)", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, padding: "6px 10px", cursor: "pointer", fontSize: 12 };
const primary: React.CSSProperties = { ...secondary, background: "var(--wc-accent)", color: "var(--wc-accent-fg)", borderColor: "var(--wc-accent)" };
const danger: React.CSSProperties = { ...secondary, color: "#fecaca", borderColor: "#b91c1c" };
const activeMode: React.CSSProperties = { background: "var(--wc-accent)", color: "var(--wc-accent-fg)", borderColor: "var(--wc-accent)" };
const progressTrack: React.CSSProperties = { height: 7, marginTop: 7, borderRadius: 4, background: "var(--wc-bg-deepest)", overflow: "hidden" };
const progressFill: React.CSSProperties = { height: "100%", borderRadius: 4, background: "var(--wc-accent)", transition: "width .2s" };
const errorText: React.CSSProperties = { marginTop: 8, color: "#fca5a5", fontSize: 11, whiteSpace: "pre-wrap" };
