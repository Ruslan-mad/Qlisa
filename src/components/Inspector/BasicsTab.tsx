// Basics tab: cue identity only (number, name, color, notes, media file,
// flow). Type-specific behaviour lives in each cue type's own tab.

import { Field, Grid2, MiniField, Section, ToggleRow, inputStyle } from "./Field";
import { ColorPicker } from "./ColorPicker";
import { MediaPreview } from "./MediaThumbnail";
import { resolveMediaPreviewKind } from "./mediaPreviewModel";
import type { AudioCueData } from "../../lib/types";
import { Select } from "../common/Select";
import { useLocale } from "../../i18n";

export function BasicsTab({
  cue,
  isAudio,
  isVideo,
  isImage,
  isMidiFile,
  onSave,
  onBrowse,
  onBrowseVideo,
  onBrowseImage,
  onBrowseMidi,
}: {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  cue: any;
  isAudio: boolean;
  isVideo?: boolean;
  isImage?: boolean;
  isMidiFile?: boolean;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onSave: (p: Partial<any>) => void;
  onBrowse: () => void;
  onBrowseVideo?: () => void;
  onBrowseImage?: () => void;
  onBrowseMidi?: () => void;
}) {
  const { t } = useLocale();
  return (
    <>
      <Section title={t("inspector.identity")}>
        <Grid2>
          <MiniField label="Cue #">
            <input
              style={inputStyle}
              defaultValue={cue.number ?? ""}
              onBlur={(e) => onSave({ number: e.target.value || null })}
            />
          </MiniField>
          <MiniField label="Color">
            <div style={{ paddingTop: 3 }}>
              <ColorPicker value={cue.color} onChange={(c) => onSave({ color: c })} />
            </div>
          </MiniField>
        </Grid2>
        <Field label="Name">
          <input
            style={inputStyle}
            defaultValue={cue.name}
            onBlur={(e) => onSave({ name: e.target.value })}
          />
        </Field>
        <Field label="Notes">
          <textarea
            style={{ ...inputStyle, resize: "vertical", minHeight: 56 }}
            defaultValue={cue.notes ?? ""}
            onBlur={(e) => onSave({ notes: e.target.value })}
          />
        </Field>
      </Section>

      {(isAudio || isVideo || isImage || isMidiFile) && (
        <Section title={t("inspector.media")}>
          {(isVideo || isImage) && cue.file_path && (
            <MediaPreview
              cueId={cue.id}
              path={cue.file_path}
              kind={resolveMediaPreviewKind(!!isVideo)}
              startMs={cue.start_time_ms ?? 0}
              durationMs={cue.cached_duration_ms ?? cue.file_duration_ms ?? cue.duration_ms ?? 0}
            />
          )}
          <div style={{ display: "flex", gap: 6, marginBottom: 10 }}>
            <input
              style={{ ...inputStyle, flex: 1 }}
              readOnly
              value={cue.file_path ? cue.file_path.split(/[\\/]/).pop() ?? cue.file_path : t("common.none")}
              title={cue.file_path ?? ""}
            />
            <button
              style={{
                padding: "4px 12px",
                background: "var(--wc-bg-hover)",
                border: "1px solid var(--wc-border-strong)",
                borderRadius: 4,
                color: "var(--wc-text)",
                cursor: "pointer",
                fontSize: 12,
                flexShrink: 0,
              }}
              onClick={
                isVideo ? onBrowseVideo
                : isImage ? onBrowseImage
                : isMidiFile ? onBrowseMidi
                : onBrowse
              }
            >
              {t("uiFixes.browse")}
            </button>
          </div>
        </Section>
      )}

      <Section title={t("inspector.flow")}>
        <Field label="Continue">
          <Select
            style={inputStyle}
            value={cue.continue_mode}
            onChange={(e) =>
              onSave({ continue_mode: e.target.value as AudioCueData["continue_mode"] })
            }
          >
            <option value="do_not_continue">{t("inspector.doNotContinue")}</option>
            <option value="auto_continue">Auto-Continue</option>
            <option value="auto_follow">Auto-Follow</option>
          </Select>
        </Field>
        <ToggleRow
          label={t("sweepUi.disableCue")}
          checked={cue.is_disabled ?? false}
          onToggle={(v) => onSave({ is_disabled: v })}
        />
      </Section>
    </>
  );
}
