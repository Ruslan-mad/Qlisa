import { useState } from "react";
import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useLocale } from "../../i18n";
import { getMediaConversionStatus, listMediaConversions, replaceCueMedia } from "../../lib/commands";
import { useMediaConversionStore } from "../../stores/mediaConversionStore";
import { conversionProgress } from "../MediaConversion/mediaConversionModel";

export function MediaConversionStatus() {
  const { t } = useLocale();
  const jobs = useMediaConversionStore((state) => state.jobs);
  const clearFinished = useMediaConversionStore((state) => state.clearFinished);
  const upsert = useMediaConversionStore((state) => state.upsert);
  const [open, setOpen] = useState(false);
  const active = jobs.filter((job) => job.status === "queued" || job.status === "running");
  const finished = jobs.filter((job) => job.status === "completed" || job.status === "failed" || job.status === "cancelled");
  const activeIds = active.map((job) => job.id).join(",");
  const statusLabel = (status: string) => status === "completed" ? t("mediaConversion.completed") : status === "failed" ? t("mediaConversion.failed") : t("mediaConversion.cancelled");

  useEffect(() => {
    void listMediaConversions().then((items) => items.forEach(upsert)).catch(() => undefined);
    const subscription = listen("media-conversion-event", (event) => {
      const job = event.payload as Parameters<typeof upsert>[0];
      upsert(job);
      // The worker emits `completed` only after the temporary output has been
      // atomically renamed and validated. Apply it through the existing Cue
      // command so the original path remains tracked for Restore original.
      // Jobs without a Cue (or already applied) are intentionally untouched.
      if (job.status === "completed" && job.cue_id && !job.applied_to_cue) {
        void replaceCueMedia(job.id)
          .then(() => upsert({ ...job, applied_to_cue: true }))
          .catch((reason) => console.warn("automatic media replacement failed", reason));
      }
    });
    return () => { void subscription.then((unlisten) => unlisten()); };
  }, [upsert]);

  useEffect(() => {
    if (!activeIds) return;
    const timer = window.setInterval(() => {
      for (const jobId of activeIds.split(",")) {
        void getMediaConversionStatus(jobId).then(upsert).catch(() => undefined);
      }
    }, 700);
    return () => window.clearInterval(timer);
  }, [activeIds, upsert]);

  if (jobs.length === 0) return null;

  return <div style={{ position: "relative", flexShrink: 0 }}>
    <button type="button" onClick={() => setOpen((value) => !value)} style={statusButton} aria-expanded={open}>
      {active.length > 0 ? `⇄ ${t("mediaConversion.active")}: ${active.length}` : `✓ ${t("mediaConversion.completed")}: ${finished.length}`}
    </button>
    {open && <div style={popover}>
      <div style={popoverHeader}><span>{t("mediaConversion.jobs")}</span>{finished.length > 0 && <button type="button" onClick={clearFinished} style={clearButton}>{t("mediaConversion.clear")}</button>}</div>
      {jobs.map((job) => <div key={job.id} style={jobRow} onClick={() => { if ((job.status === "completed" || job.status === "failed" || job.status === "cancelled") && job.cue_id) window.dispatchEvent(new CustomEvent("media-conversion-open", { detail: job })); }}>
        <div style={{ display: "flex", justifyContent: "space-between", gap: 8 }}><span style={jobName}>{job.input_path.split(/[\\/]/).pop() ?? job.input_path}</span><span style={jobStatus}>{job.status === "running" || job.status === "queued" ? `${Math.round(conversionProgress(job) * 100)}%` : statusLabel(job.status)}</span></div>
        {(job.status === "running" || job.status === "queued") && <div style={progressTrack}><div style={{ ...progressFill, width: `${conversionProgress(job) * 100}%` }} /></div>}
        {job.status === "completed" && job.cue_id && <button type="button" style={openJobButton} onClick={(event) => { event.stopPropagation(); window.dispatchEvent(new CustomEvent("media-conversion-open", { detail: job })); }}>{job.applied_to_cue ? t("mediaConversion.restoreOriginal") : t("mediaConversion.openCompleted")}</button>}
        {(job.status === "failed" || job.status === "cancelled") && job.cue_id && <button type="button" style={openJobButton} onClick={(event) => { event.stopPropagation(); window.dispatchEvent(new CustomEvent("media-conversion-open", { detail: job })); }}>{t("mediaConversion.details")}</button>}
      </div>)}
    </div>}
  </div>;
}

const statusButton: React.CSSProperties = { color: "var(--wc-text-muted)", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 4, padding: "5px 8px", cursor: "pointer", fontSize: 11, whiteSpace: "nowrap" };
const popover: React.CSSProperties = { position: "absolute", right: 0, bottom: "calc(100% + 8px)", width: 300, maxWidth: "calc(100vw - 24px)", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 6, padding: 8, boxShadow: "0 8px 24px rgba(0,0,0,.55)", zIndex: 20 };
const popoverHeader: React.CSSProperties = { display: "flex", justifyContent: "space-between", alignItems: "center", color: "var(--wc-text)", fontSize: 11, fontWeight: 600, marginBottom: 5 };
const clearButton: React.CSSProperties = { color: "var(--wc-text-muted)", background: "transparent", border: 0, cursor: "pointer", fontSize: 10 };
const jobRow: React.CSSProperties = { borderTop: "1px solid var(--wc-border)", padding: "6px 0" };
const jobName: React.CSSProperties = { minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "var(--wc-text)", fontSize: 11 };
const jobStatus: React.CSSProperties = { color: "var(--wc-text-muted)", fontSize: 10, flexShrink: 0 };
const progressTrack: React.CSSProperties = { height: 4, marginTop: 4, borderRadius: 3, background: "var(--wc-bg-deepest)", overflow: "hidden" };
const progressFill: React.CSSProperties = { height: "100%", background: "var(--wc-accent)", borderRadius: 3 };
const openJobButton: React.CSSProperties = { color: "var(--wc-accent)", background: "transparent", border: 0, padding: "3px 0 0", cursor: "pointer", fontSize: 10 };
