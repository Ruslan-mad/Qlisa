import { memo, useEffect, useState } from "react";
import { useLocale } from "../../i18n";

function getDate(locale: string) {
  return new Intl.DateTimeFormat(locale === "ru" ? "ru-RU" : "en-US", {
    weekday: "short", day: "2-digit", month: "short", year: "numeric",
  }).format(new Date());
}

function getTime() {
  return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(new Date());
}

export const StageClock = memo(function StageClock() {
  const { locale } = useLocale();
  const [time, setTime] = useState(getTime);
  const [date, setDate] = useState(() => getDate(locale));

  useEffect(() => {
    const update = () => {
      setTime(getTime());
      setDate(getDate(locale));
    };
    const timer = window.setInterval(update, 1000);
    return () => window.clearInterval(timer);
  }, [locale]);

  return (
    <div className="stage-clock" aria-label={`${date} ${time}`} style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", lineHeight: 1.15, whiteSpace: "nowrap" }}>
      <span className="stage-clock-date" style={{ fontSize: 11, color: "var(--wc-text-muted)" }}>{date}</span>
      <span className="stage-clock-time" aria-live="off" style={{ fontSize: 28, fontVariantNumeric: "tabular-nums", lineHeight: 1 }}>{time}</span>
    </div>
  );
});
