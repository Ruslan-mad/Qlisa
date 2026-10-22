// Theme-aware arrow marking the next cue in the cue list.
import { useLocale } from "../../i18n";

interface Props {
  visible: boolean;
}

export function PlayheadIndicator({ visible }: Props) {
  const { t } = useLocale();
  if (!visible) return <span style={{ display: "inline-block", width: 12 }} />;
  return (
    <span
      role="img"
      aria-label={t("transport.nextCue")}
      title={t("transport.nextCue")}
      style={{ display: "inline-flex", width: 14, height: 18, verticalAlign: "middle", flexShrink: 0, color: "var(--wc-text-bright, var(--wc-text))" }}
    >
      <svg aria-hidden="true" width="14" height="18" viewBox="0 0 14 18" fill="none">
        <path d="M3 1.5v8a4 4 0 0 0 4 4h5M8.5 10l3.5 3.5-3.5 3.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </span>
  );
}
