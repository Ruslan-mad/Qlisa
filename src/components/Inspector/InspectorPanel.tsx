// Contextual inspector panel shown on the right side.
//
// Tab model: Basics (identity) | the cue type's main tab (Fade / Stop / Group /
// Camera / Messages / Light / Mic / Timecode / Text) | Time | Levels | Fade |
// Layer | Geometry | Triggers — only the tabs that apply to the selected cue
// type are shown.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ScriptCueData, AudioCueData, BrowserCueData, CameraCueData, CueSummary, CueType, DevampCueData, FadeCueData, ImageCueData, LightCueData, MemoCueData, MicCueData, MidiCueData, MidiFileCueData, NumberCueData, OscCueData, StopCueData, TextCueData, TimecodeCueData, TimerCueData, VideoCueData, WaitCueData } from "../../lib/types";
import { isCommandCueType } from "../../lib/types";
import { getCue, setPlaylistLoop, updateCue } from "../../lib/commands";
import { BasicsTab } from "./BasicsTab";
import { BasicQuickControls } from "./BasicQuickControls";
import { TimeTab } from "./TimeTab";
import { LevelsTab } from "./LevelsTab";
import { FadeTab } from "./FadeTab";
import { FadeCueTab } from "./FadeCueTab";
import { StopTab } from "./StopTab";
import { CommandTab } from "./CommandTab";
import { ScriptTab } from "./ScriptTab";
import { DevampTab } from "./DevampTab";
import { GroupTab } from "./GroupTab";
import { NumberTab } from "./NumberTab";
import { normalizeNumberCueData } from "./numberModel";
import { shouldRetainCueDataForTarget } from "../../lib/clipEditorLoadGuards";
import { LayerTab } from "./LayerTab";
import { GeometryTab } from "./GeometryTab";
import { MidiTab } from "./MidiTab";
import { MidiFileTab } from "./MidiFileTab";
import { MemoTab } from "./MemoTab";
import { OscTab } from "./OscTab";
import { LightTab } from "./LightTab";
import { MicTab } from "./MicTab";
import { TextTab } from "./TextTab";
import { TimerTab } from "./TimerTab";
import { TimecodeTab } from "./TimecodeTab";
import { TriggersTab } from "./TriggersTab";
import { CameraTab } from "./CameraTab";
import { BrowserTab } from "./BrowserTab";
import { MultiCueInspector } from "./MultiCueInspector";
import { displayedOutputIds, OutputSelector, type OutputSelectableCue } from "./OutputSelector";
import { formatInspectorSaveError, isCurrentCueRequest, isFadeEdit, persistCurrentThenCommit } from "./singleCueSave";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import { useLocale } from "../../i18n";
import {
  inspectorContentStyle,
  inspectorRootStyle,
  inspectorTabBarStyle,
  inspectorTabStyle,
  inspectorTitleStyle,
} from "./InspectorChrome";
import { CueTypeIcon } from "../common/CueTypeIcon";
import { PresentationControls } from "../Presentation/PresentationControls";
import { findPresentationOwner } from "../Presentation/presentationNavigation";

interface Props {
  selectedCue: CueSummary | null;
  selectedCueIds: string[];
  onRefresh: () => void;
  /** Open the clip editor dock (trim + slices) for a cue. */
  onOpenEditor?: (cueId: string) => void;
  /** Open the fade curve editor dock for a cue. */
  onOpenCurveEditor?: (cueId: string) => void;
  /** Bump to force a re-fetch of the inspected cue (dock edits). */
  reloadToken?: number;
  /** Called after every inspector save, so the clip editor dock can reload. */
  onCueSaved?: () => void;
  /** Select a nested Number action for direct configuration. */
  onSelectCue?: (cueId: string) => void;
  /** Current cue-list snapshot, used for Number flow target pickers. */
  allCues?: CueSummary[];
}

type Tab =
  | "basics" | "time" | "levels" | "fade" | "layer" | "geometry" | "messages"
  | "fade-cue" | "stop" | "devamp" | "group" | "light" | "mic" | "timecode"
  | "text" | "timer" | "camera" | "browser" | "triggers" | "command" | "script" | "midi-file" | "memo" | "number";

type CueData =
  | AudioCueData | VideoCueData | ImageCueData | WaitCueData | FadeCueData
  | MidiCueData | OscCueData | StopCueData | DevampCueData | LightCueData
  | MicCueData | TimecodeCueData | TextCueData | TimerCueData | CameraCueData | BrowserCueData | NumberCueData;

/** Ordered tab list for a cue type: identity first, the type's main tab next,
 *  then timing, shared A/V tabs, and Triggers last. */
function tabsFor(type: CueType, t: (key: string, vars?: Record<string, string | number>) => string): { id: Tab; label: string }[] {
  const isAV = type === "audio" || type === "video" || type === "camera";
  const isVisual = type === "video" || type === "image" || type === "camera";
  const hasGeometry = isVisual || type === "text" || type === "timer";
  const hasAvFade = isAV || type === "image" || type === "number";

  const tabs: { id: Tab; label: string }[] = [{ id: "basics", label: t("inspector.basics") }];
  if (type === "fade") tabs.push({ id: "fade-cue", label: t("inspector.fade") });
  if (type === "stop") tabs.push({ id: "stop", label: t("inspector.stop") });
  if (isCommandCueType(type)) tabs.push({ id: "command", label: t("inspector.command") });
  if (type === "devamp") tabs.push({ id: "devamp", label: t("inspector.devamp") });
  if (type === "group" || type === "presentation") tabs.push({ id: "group", label: t(type === "presentation" ? "cueTypes.presentation" : "inspector.group") });
  if (type === "number") tabs.push({ id: "number", label: t("inspector.number") });
  if (type === "camera") tabs.push({ id: "camera", label: t("inspector.camera") });
  if (type === "browser") tabs.push({ id: "browser", label: t("inspector.browser") });
  if (type === "osc" || type === "midi") tabs.push({ id: "messages", label: t("sweepUi.cueTabsMessages") });
  if (type === "midi_file") tabs.push({ id: "midi-file", label: t("sweepUi.cueTabsMidiFile") });
  if (type === "memo") tabs.push({ id: "memo", label: t("inspector.memo") });
  if (type === "light") tabs.push({ id: "light", label: t("inspector.light") });
  if (type === "mic") tabs.push({ id: "mic", label: t("inspector.mic") });
  if (type === "timecode") tabs.push({ id: "timecode", label: t("inspector.timecode") });
  if (type === "text") tabs.push({ id: "text", label: t("inspector.text") });
  if (type === "timer") tabs.push({ id: "timer", label: t("inspector.timer") });
  if (type === "script") tabs.push({ id: "script", label: t("inspector.script") });
  tabs.push({ id: "time", label: t("inspector.time") });
  if (isAV) tabs.push({ id: "levels", label: t("inspector.levels") });
  if (hasAvFade) tabs.push({ id: "fade", label: t("inspector.fade") });
  if (isVisual) tabs.push({ id: "layer", label: t("inspector.layer") });
  if (hasGeometry) tabs.push({ id: "geometry", label: t("inspector.geometry") });
  tabs.push({ id: "triggers", label: t("inspector.triggers") });
  return tabs;
}

export function InspectorPanel({ selectedCue, selectedCueIds, onRefresh, onOpenEditor, onOpenCurveEditor, reloadToken, onCueSaved, onSelectCue, allCues }: Props) {
  const { t } = useLocale();
  const displayPrefs = useWorkspaceStore((s) => s.displayPrefs);
  const [cueData, setCueData] = useState<CueData | null>(null);
  const [activeTab, setActiveTab] = useState<Tab>("basics");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [formRevision, setFormRevision] = useState(0);
  const [quickSavePending, setQuickSavePending] = useState(false);
  const quickSavePendingRef = useRef(false);
  const selectionKey = selectedCueIds.join("\u0000");
  const cueLoadGeneration = useRef(0);
  const cueDataTargetRef = useRef<string | null>(null);
  const singleSaveGeneration = useRef(0);
  const currentSaveSelectionKey = useRef(selectionKey);
  const currentSelectedCueId = useRef(selectedCueIds.length === 1 ? selectedCue?.id ?? null : null);

  useLayoutEffect(() => {
    currentSelectedCueId.current = selectedCueIds.length === 1 ? selectedCue?.id ?? null : null;
    const target = selectedCue && selectedCueIds.length === 1
      ? `${selectedCue.id}\u0000${selectedCue.cue_type}`
      : null;
    if (!shouldRetainCueDataForTarget(cueDataTargetRef.current, target)) setCueData(null);
    cueDataTargetRef.current = target;
    setSaveError(null);
  }, [selectedCue?.id, selectedCue?.cue_type, selectionKey, reloadToken]);

  useLayoutEffect(() => {
    currentSaveSelectionKey.current = selectionKey;
    ++singleSaveGeneration.current;
  }, [selectedCue?.id, selectionKey, reloadToken]);

  useEffect(() => {
    const generation = ++cueLoadGeneration.current;
    if (selectedCueIds.length > 1) {
      setCueData(null);
      return;
    }
    if (!selectedCue) {
      setCueData(null);
      return;
    }
    const available = tabsFor(selectedCue.cue_type, t).map((t) => t.id);
    setActiveTab((prev) => (available.includes(prev) ? prev : "basics"));
    getCue(selectedCue.id)
      .then((data) => {
        if (generation !== cueLoadGeneration.current) return;
        // Merge cue_type from the summary in case the serialised form uses
        // a different key ("type" vs "cue_type").
        const normalized = { ...data, cue_type: selectedCue.cue_type } as CueData;
        setCueData(selectedCue.cue_type === "number"
          ? normalizeNumberCueData(normalized as NumberCueData)
          : normalized);
      })
      .catch((error) => {
        if (generation === cueLoadGeneration.current) console.error(error);
      });
    // reloadToken: the clip editor dock saved this cue — re-fetch it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedCue?.id, selectedCue?.cue_type, selectedCue?.number_master_id, selectedCue?.state, selectedCueIds.length, reloadToken]);

  if (selectedCueIds.length > 1) {
    return <MultiCueInspector cueIds={selectedCueIds} />;
  }

  if (!selectedCue || !cueData) {
    return (
      <div
        style={{
          padding: 24,
          color: "var(--wc-text-faint)",
          textAlign: "center",
          fontSize: 13,
        }}
      >
        {t("common.select")} {t("cueList.cue").toLowerCase()}.
      </div>
    );
  }

  const type = selectedCue.cue_type;
  const presentationOwner = findPresentationOwner(allCues ?? [], selectedCue.id);
  const isAudio = type === "audio";
  const isVideo = type === "video";
  const isImage = type === "image";
  const isWait  = type === "wait";
  const isFade  = type === "fade";
  const isCamera = type === "camera";
  const isBrowser = type === "browser";
  const isMidiFile = type === "midi_file";

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const save = async (partial: Partial<any>) => {
    const targetCueId = cueData.id;
    const submittedSelectionKey = selectionKey;
    const submittedSelectionIds = [...selectedCueIds];
    const ticket = { cueId: targetCueId, generation: ++singleSaveGeneration.current };
    const isCurrent = () => isCurrentCueRequest(ticket, singleSaveGeneration.current, currentSelectedCueId.current)
      && currentSaveSelectionKey.current === submittedSelectionKey;
    const fadeEdit = isFadeEdit(partial);
    const persist = async () => {
      const { playlist_loop, ...cuePatch } = partial;
      if (typeof playlist_loop === "boolean") {
        await setPlaylistLoop(targetCueId, playlist_loop);
      }
      // Color changes fan out to every selected cue; everything else applies
      // to the primary (inspector) cue only.
      if ("color" in cuePatch && submittedSelectionIds.length > 1) {
        await Promise.all(submittedSelectionIds.map((id) => updateCue(id, { color: cuePatch.color })));
        // Apply any remaining non-color fields to the primary cue.
        // eslint-disable-next-line @typescript-eslint/no-unused-vars
        const { color: _c, ...rest } = cuePatch;
        if (Object.keys(rest).length > 0) await updateCue(targetCueId, rest);
      } else if (Object.keys(cuePatch).length > 0) {
        await updateCue(targetCueId, cuePatch);
      }
    };

    await persistCurrentThenCommit(
      persist,
      isCurrent,
      () => {
        if (!isCurrent()) return;
        setCueData((prev) => (isCurrent() && prev?.id === targetCueId ? { ...prev, ...partial } : prev));
        setSaveError(null);
        onRefresh();
        onCueSaved?.();
      },
      (error) => {
        if (!isCurrent()) return;
        console.error("Failed to save cue changes:", error);
        setSaveError(formatInspectorSaveError(error, fadeEdit, t));
        // Some Inspector fields are uncontrolled inputs. Remount them from the
        // unchanged cueData so a rejected attempted value is not left visible.
        setFormRevision((revision) => revision + 1);
      },
    );
  };

  const saveQuick = async (partial: Record<string, unknown>) => {
    if (quickSavePendingRef.current) return;
    quickSavePendingRef.current = true;
    setQuickSavePending(true);
    try {
      await save(partial);
    } finally {
      quickSavePendingRef.current = false;
      setQuickSavePending(false);
    }
  };

  return (
    <div style={inspectorRootStyle}>
      {/* Title */}
      <div style={inspectorTitleStyle}>
        <span style={{ display: "flex", alignItems: "center", gap: 7, minWidth: 0, fontWeight: 600 }}>
          <CueTypeIcon type={type} size={16} tone="neutral" />
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {cueData.name}
          </span>
        </span>
        {selectedCue.number && (
          <span style={{ fontSize: 11, color: "var(--wc-text-muted)", flexShrink: 0 }}>
            #{selectedCue.number}
          </span>
        )}
      </div>

      {presentationOwner && <PresentationControls owner={presentationOwner as CueSummary} mode="inspector" onChanged={onRefresh} />}

      {/* Tabs — flexWrap so every tab stays reachable however narrow the
          inspector gets (a fixed row cropped the trailing tabs). */}
      <div style={inspectorTabBarStyle}>
        {tabsFor(type, t).map((tab) => (
          <button key={tab.id} style={inspectorTabStyle(activeTab === tab.id)} onClick={() => setActiveTab(tab.id)}>
            {tab.label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div key={formRevision} style={inspectorContentStyle}>
        {saveError && (
          <div role="alert" style={{ padding: 9, marginBottom: 10, borderRadius: 4, background: "rgba(248,113,113,0.12)", color: "#fca5a5", fontSize: 12 }}>
            {saveError}
          </div>
        )}
        {isVideo && (cueData as VideoCueData).presentation_video_imported === true && (
          <div style={{ padding: 9, marginBottom: 10, border: "1px solid var(--wc-border)", borderRadius: 5 }}>
            <label style={{ display: "block", marginBottom: 5, fontSize: 11, color: "var(--wc-text-muted)" }}>{t("presentationUi.videoMode")}</label>
            <select
              value={(cueData as VideoCueData).presentation_video_mode ?? "fullscreen"}
              disabled={selectedCue.state !== "standby"}
              onChange={(event) => void save({ presentation_video_mode: event.target.value as "fullscreen" | "as_slide" })}
              style={{ width: "100%", padding: "6px 8px", color: "var(--wc-text)", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 4 }}
            >
              <option value="fullscreen">{t("presentationUi.fullscreen")}</option>
              <option value="as_slide" disabled={!(cueData as VideoCueData).presentation_video_layout_supported}>{t("presentationUi.asSlide")}</option>
            </select>
            {!(cueData as VideoCueData).presentation_video_layout_supported && <div style={{ marginTop: 6, color: "var(--wc-text-muted)", fontSize: 11 }}>{(cueData as VideoCueData).presentation_video_layout_error || t("presentationUi.asSlideUnsupported")}</div>}
            {selectedCue.state !== "standby" && <div style={{ marginTop: 6, color: "var(--wc-text-muted)", fontSize: 11 }}>{t("presentationUi.stopToChangeMode")}</div>}
          </div>
        )}
        {(type === "video" || type === "image" || type === "camera" || type === "text" || type === "timer" || isBrowser) && (
          <OutputSelector
            cues={[cueData as CueData]}
            outputs={(displayPrefs.output_destinations ?? []).filter((output) => !isBrowser || output.sink_kind === "display")}
            defaultOutputId={displayPrefs.default_output_id}
            onChange={(updates) => {
              const update = updates[0];
              if (update) void save({ output_id: update.output_id, output_ids: update.output_ids });
            }}
          />
        )}
        {activeTab === "basics" && (
          <BasicQuickControls
            cue={cueData}
            outputs={displayPrefs.output_destinations ?? []}
            outputIds={displayedOutputIds(
              cueData as OutputSelectableCue,
              new Set((displayPrefs.output_destinations ?? []).map((output) => output.id)),
              displayPrefs.default_output_id,
            )}
            disabled={quickSavePending}
            onSave={saveQuick}
          />
        )}
        {activeTab === "basics" && (
          <>
            <BasicsTab
              cue={cueData}
              onSave={save}
            />
          </>
        )}
        {activeTab === "fade-cue" && isFade && (
          <FadeCueTab
            cue={cueData as FadeCueData}
            onSave={save}
            onOpenCurveEditor={
              onOpenCurveEditor ? () => onOpenCurveEditor(cueData.id) : undefined
            }
          />
        )}
        {activeTab === "script" && type === "script" && (
          <ScriptTab cue={cueData as unknown as ScriptCueData} onSave={save} />
        )}
        {activeTab === "command" && isCommandCueType(type) && (
          <CommandTab cue={cueData as StopCueData} onSave={save} />
        )}
        {activeTab === "stop" && type === "stop" && (
          <StopTab cue={cueData as StopCueData} onSave={save} />
        )}
        {activeTab === "devamp" && type === "devamp" && (
          <DevampTab cue={cueData as DevampCueData} onSave={save} />
        )}
        {activeTab === "group" && (type === "group" || type === "presentation") && (
          <GroupTab cue={cueData} onRefresh={onRefresh} presentation={type === "presentation"} />
        )}
        {activeTab === "number" && type === "number" && (
          <>
            <NumberTab
              cue={cueData as NumberCueData}
              allCues={allCues}
              onRefresh={onRefresh}
              onSelectCue={onSelectCue}
              onSaved={() => {
                onCueSaved?.();
              }}
            />
          </>
        )}
        {activeTab === "time" && (
          <TimeTab
            cue={cueData}
            selectedCue={selectedCue}
            isAudio={isAudio}
            isVideo={isVideo}
            isImage={isImage}
            isWait={isWait}
            isFade={isFade}
            onSave={save}
            onOpenWaveform={() => onOpenEditor?.(cueData.id)}
          />
        )}
        {activeTab === "levels" && (isAudio || isVideo || isCamera) && (
          <LevelsTab cue={cueData as AudioCueData | VideoCueData | CameraCueData} isAudio={isAudio || isCamera} onSave={save} />
        )}
        {activeTab === "fade" && (isAudio || isVideo || isImage || isCamera || type === "number") && (
          <FadeTab cue={cueData as AudioCueData | VideoCueData | ImageCueData | CameraCueData | NumberCueData} onSave={save} />
        )}
        {activeTab === "layer" && (isVideo || isImage || isCamera) && (
          <LayerTab cue={cueData as VideoCueData | ImageCueData | CameraCueData} onSave={save} />
        )}
        {activeTab === "geometry" && (isVideo || isImage || isCamera || type === "text" || type === "timer") && (
          <GeometryTab
            key={cueData.id}
            cue={cueData as VideoCueData | ImageCueData | CameraCueData | TextCueData | TimerCueData}
            outputs={displayPrefs.output_destinations ?? []}
            sourceWidth={selectedCue.media_width}
            sourceHeight={selectedCue.media_height}
            cameraActive={selectedCue.state === "running" || selectedCue.state === "paused"}
            overlayMargin={displayPrefs.timer_position === "center" ? 0 : displayPrefs.timer_margin}
            outputIds={displayedOutputIds(
              cueData as OutputSelectableCue,
              new Set((displayPrefs.output_destinations ?? []).map((output) => output.id)),
              displayPrefs.default_output_id,
            )}
            onSave={save}
          />
        )}
        {activeTab === "camera" && isCamera && (
          <CameraTab cue={cueData as CameraCueData} onSave={save} />
        )}
        {activeTab === "browser" && isBrowser && (
          <BrowserTab cue={cueData as BrowserCueData} onSave={save} />
        )}
        {activeTab === "messages" && type === "osc" && (
          <OscTab cue={cueData as OscCueData} onSave={save} />
        )}
        {activeTab === "messages" && type === "midi" && (
          <MidiTab cue={cueData as MidiCueData} onSave={save} />
        )}
        {activeTab === "midi-file" && isMidiFile && (
          <MidiFileTab cue={cueData as unknown as MidiFileCueData} onSave={save} />
        )}
        {activeTab === "memo" && type === "memo" && (
          <MemoTab cue={cueData as unknown as MemoCueData} onSave={save} />
        )}
        {activeTab === "light" && type === "light" && (
          <LightTab cue={cueData as LightCueData} onSave={save} />
        )}
        {activeTab === "mic" && type === "mic" && (
          <MicTab cue={cueData as MicCueData} onSave={save} />
        )}
        {activeTab === "timecode" && type === "timecode" && (
          <TimecodeTab cue={cueData as TimecodeCueData} onSave={save} />
        )}
        {activeTab === "text" && type === "text" && (
          <TextTab cue={cueData as TextCueData} onSave={save} />
        )}
        {activeTab === "timer" && type === "timer" && (
          <TimerTab cue={cueData as TimerCueData} selectedCue={selectedCue} onSave={save} onRefresh={onRefresh} />
        )}
        {activeTab === "triggers" && (
          <TriggersTab cue={selectedCue} onSave={onRefresh} />
        )}
      </div>

    </div>
  );
}
