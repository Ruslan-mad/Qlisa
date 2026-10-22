// Theme-aware arrow marking the next cue in the cue list.
import { useLocale } from "../../i18n";

interface Props {
  visible: boolean;
  running?: boolean;
  stack?: boolean;
}

export function PlayheadIndicator({ visible, running = false, stack = false }: Props) {
  const { t } = useLocale();
  if (!visible && !running) return <span style={{ display: "inline-block", width: 12 }} />;
  const stacked = stack && visible && running;
  return (
    <span style={{
      display: "inline-flex",
      flexDirection: stacked ? "column" : "row",
      alignItems: "center",
      justifyContent: "center",
      width: stacked ? 14 : undefined,
      height: stacked ? 24 : 18,
      verticalAlign: "middle",
      flexShrink: 0,
    }}>
      {running && <span
        role="img"
        aria-label={t("activeCues.running")}
        title={t("activeCues.running")}
        style={{ display: "inline-block", width: 0, height: 0, borderTop: "6px solid transparent", borderBottom: "6px solid transparent", borderLeft: "10px solid #4ade80", flexShrink: 0 }}
      />}
      {visible && <span
        role="img"
        aria-label={t("transport.nextCue")}
        title={t("transport.nextCue")}
        style={{ display: "inline-flex", width: 12, height: stacked ? 12 : 18, flexShrink: 0, color: "var(--wc-text-bright, var(--wc-text))" }}
      >
        <svg aria-hidden="true" width="12" height={stacked ? 12 : 18} viewBox="0 0 14 18" fill="none">
          <path d="M3 1.5v8a4 4 0 0 0 4 4h5M8.5 10l3.5 3.5-3.5 3.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </span>}
    </span>
  );
}
