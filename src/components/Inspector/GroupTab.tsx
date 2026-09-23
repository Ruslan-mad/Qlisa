// Group Cue main tab: playback mode and per-mode options.

import type { GroupMode } from "../../lib/types";
import { setGroupMode, setPlaylistLoop } from "../../lib/commands";
import { Section, ToggleRow, inputStyle } from "./Field";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";
import { useEffect, useState } from "react";

const MODE_HINTS: Record<GroupMode, string> = {
  simultaneous: "GO starts every child at once (use child pre-waits for a timeline).",
  sequential: "GO starts the first child; each child triggers the next.",
  playlist: "One child plays at a time; GO advances to the next.",
  start_random: "GO starts one random child (shuffle-bag, no repeats until all played).",
};

export function GroupTab({
  cue,
  onRefresh,
}: {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  cue: any;
  onRefresh: () => void;
}) {
  const { t, locale } = useLocale();
  const modeHints: Record<GroupMode, string> = locale === "ru" ? {
    simultaneous: "GO запускает всех дочерних cue одновременно (для таймлайна используйте Pre-Wait дочерних cue).",
    sequential: "GO запускает первое дочернее cue; каждое следующее запускается предыдущим.",
    playlist: "Одновременно воспроизводится одно дочернее cue; GO переходит к следующему.",
    start_random: "GO запускает одно случайное дочернее cue (перемешивание без повторов до полного цикла).",
  } : MODE_HINTS;
  const persistedMode: GroupMode = cue.group_mode ?? "simultaneous";
  const persistedLoop = cue.playlist_loop ?? false;
  const [mode, setMode] = useState<GroupMode>(persistedMode);
  const [playlistLoop, setPlaylistLoopState] = useState(persistedLoop);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setMode(persistedMode);
    setPlaylistLoopState(persistedLoop);
  }, [cue.id, persistedMode, persistedLoop]);

  return (
    <Section title="Mode">
      {error && <div role="alert" style={{ color: "#fecaca", fontSize: 11, marginBottom: 6 }}>{error}</div>}
      <Select
        style={inputStyle}
        value={mode}
        onChange={async (e) => {
          const previousMode = mode;
          try {
            const nextMode = e.target.value as GroupMode;
            setMode(nextMode);
            await setGroupMode(cue.id, nextMode);
            setError(null);
            onRefresh();
          } catch (caught) {
            setMode(previousMode);
            setError(String(caught));
          }
        }}
      >
        <option value="simultaneous">{t("components.simultaneous")}</option>
        <option value="sequential">{t("components.sequential")}</option>
        <option value="playlist">{t("components.playlist")}</option>
        <option value="start_random">{t("components.startRandom")}</option>
      </Select>
      <div style={{ fontSize: 11, color: "var(--wc-text-faint)", margin: "6px 0 10px" }}>
        {modeHints[mode]}
      </div>
      {mode === "playlist" && (
        <ToggleRow
          label={t("components.loopFirstChild")}
          checked={playlistLoop}
          onToggle={async (v) => {
            const previousLoop = playlistLoop;
            setPlaylistLoopState(v);
            try {
              await setPlaylistLoop(cue.id, v);
              setError(null);
              onRefresh();
            } catch (caught) {
              setPlaylistLoopState(previousLoop);
              setError(String(caught));
            }
          }}
        />
      )}
    </Section>
  );
}
