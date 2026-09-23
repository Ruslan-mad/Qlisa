// Projector Tools shown in Preferences → Display: a visual corner-pin /
// alignment editor (re-frame or warp the whole picture inside the projector)
// and calibration test patterns (alignment grid, colour bars, colorimetry
// image, …).  Everything applies live to the output window.

import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { OutputDestination, OutputTransform, TestPatternKind } from "../../lib/types";
import { cloneOutputTransform, DEFAULT_OUTPUT_TRANSFORM } from "../../lib/types";
import {
  clearTestPattern,
  showTestPattern,
} from "../../lib/commands";
import { IMAGE_EXTENSIONS } from "../../lib/mediaTypes";
import { WarpEditor } from "./WarpEditor";
import { DragNumber } from "../common/DragNumber";
import { useLocale } from "../../i18n";

const sectionLabelStyle: React.CSSProperties = {
  fontSize: 10, fontWeight: 600, color: "var(--wc-text-muted)",
  textTransform: "uppercase", letterSpacing: "0.07em",
  marginBottom: 10, paddingBottom: 5,
  borderBottom: "1px solid var(--wc-border)",
};

const buttonStyle = (active: boolean): React.CSSProperties => ({
  padding: "5px 10px",
  fontSize: 12,
  borderRadius: 4,
  cursor: "pointer",
  whiteSpace: "nowrap",
  border: active ? "1px solid var(--wc-accent)" : "1px solid var(--wc-border-strong)",
  background: active ? "var(--wc-accent)" : "var(--wc-bg-surface)",
  color: active ? "var(--wc-accent-fg)" : "var(--wc-text)",
});

const PATTERNS: { kind: TestPatternKind; labelKey: string; hintKey: string }[] = [
  { kind: "grid", labelKey: "grid", hintKey: "gridHint" },
  { kind: "smpte_bars", labelKey: "smpteBars", hintKey: "smpteHint" },
  { kind: "rgb_test", labelKey: "rgb", hintKey: "rgbHint" },
  { kind: "test_card", labelKey: "testCard", hintKey: "testCardHint" },
  { kind: "white", labelKey: "white", hintKey: "whiteHint" },
  { kind: "gray", labelKey: "gray", hintKey: "grayHint" },
  { kind: "black", labelKey: "black", hintKey: "blackHint" },
];

function TransformField({
  label,
  value,
  step,
  min,
  max,
  suffix,
  onCommit,
}: {
  label: string;
  value: number;
  step: number;
  min: number;
  max: number;
  suffix: string;
  onCommit: (v: number) => void;
}) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
      <span style={{ width: 90, fontSize: 12, color: "var(--wc-text-secondary)", flexShrink: 0 }}>
        {label}
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onCommit(parseFloat(e.target.value))}
        style={{ flex: 1, cursor: "pointer" }}
      />
      <DragNumber
        min={min}
        max={max}
        step={step}
        key={`${label}-${value}`}
        defaultValue={value}
        onBlur={(e) => {
          const parsed = parseFloat(e.target.value);
          if (Number.isNaN(parsed)) return;
          onCommit(Math.min(max, Math.max(min, parsed)));
        }}
        style={{
          width: 64,
          background: "var(--wc-bg-app)",
          border: "1px solid var(--wc-border-strong)",
          borderRadius: 4,
          color: "var(--wc-text)",
          fontSize: 12,
          padding: "3px 6px",
        }}
      />
      <span style={{ width: 58, fontSize: 11, color: "var(--wc-text-muted)", flexShrink: 0 }}>
        {suffix}
      </span>
    </div>
  );
}

export function ProjectorToolsSection({ output, defaultOutputName, onTransformChange }: {
  output: OutputDestination | undefined;
  defaultOutputName?: string;
  onTransformChange: (transform: OutputTransform) => void;
}) {
  const { t } = useLocale();
  const [transform, setTransformState] = useState<OutputTransform>(() => cloneOutputTransform(output?.transform));
  const [activePattern, setActivePatternState] = useState<TestPatternKind | null>(null);
  const [customImagePath, setCustomImagePath] = useState<string | null>(null);
  // Mirror of activePattern readable from the unmount cleanup (setState is
  // unreliable there).
  const activePatternRef = useRef<TestPatternKind | null>(null);

  const setActivePattern = (kind: TestPatternKind | null) => {
    activePatternRef.current = kind;
    setActivePatternState(kind);
  };

  useEffect(() => {
    setTransformState(cloneOutputTransform(output?.transform));
    // Leaving Preferences (unmount) clears any test pattern still showing —
    // a calibration grid must never survive into the show.
    return () => {
      if (activePatternRef.current !== null) {
        activePatternRef.current = null;
        void clearTestPattern().catch(console.error);
      }
    };
  }, [output?.id]);

  const applyTransform = (partial: Partial<OutputTransform>) => {
    const next = { ...transform, ...partial };
    setTransformState(next);
    onTransformChange(cloneOutputTransform(next));
  };

  const cornersPinned = transform.corners.some(([x, y]) => x !== 0 || y !== 0);
  const isIdentity =
    transform.pan_x === 0 && transform.pan_y === 0 &&
    transform.scale === 1 && transform.rotation === 0 &&
    !cornersPinned;

  const togglePattern = async (kind: TestPatternKind, path?: string) => {
    if (activePattern === kind && kind !== "custom_image") {
      setActivePattern(null);
      await clearTestPattern().catch(console.error);
      return;
    }
    setActivePattern(kind);
    await showTestPattern(kind === "custom_image" ? { kind, path } : { kind }).catch(console.error);
  };

  const pickCustomImage = async () => {
    const result = await open({
      multiple: false,
      filters: [{ name: t("preferencesUi.imageFiles"), extensions: [...IMAGE_EXTENSIONS] }],
    });
    if (typeof result === "string") {
      setCustomImagePath(result);
      await togglePattern("custom_image", result);
    }
  };

  return (
    <>
      <div style={{ marginBottom: 24 }}>
        <div style={sectionLabelStyle}>{t("preferencesUi.projectorAlignment")}</div>
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginBottom: 10 }}>
          {t("preferencesUi.calibratesOutput", { name: output?.name ?? t("preferencesUi.noOutput") })} {t("preferencesUi.dragCorner")}
        </div>
        <div style={{ marginBottom: 10 }}>
          <WarpEditor transform={transform} onChange={(t) => applyTransform(t)} />
        </div>
        <TransformField
          label={t("preferencesUi.rotation")}
          value={transform.rotation}
          step={0.1}
          min={-180}
          max={180}
          suffix={t("preferencesUi.clockwise")}
          onCommit={(v) => applyTransform({ rotation: Math.round(v * 10) / 10 })}
        />
        <TransformField
          label={t("preferencesUi.scale")}
          value={transform.scale}
          step={0.01}
          min={0.1}
          max={2}
          suffix="×"
          onCommit={(v) => applyTransform({ scale: v })}
        />
        <div style={{ display: "flex", gap: 6 }}>
          <button
            disabled={!cornersPinned}
            onClick={() => applyTransform({ corners: [[0, 0], [0, 0], [0, 0], [0, 0]] })}
            style={{
              ...buttonStyle(false),
              color: !cornersPinned ? "var(--wc-text-faint)" : "var(--wc-text)",
              cursor: !cornersPinned ? "default" : "pointer",
            }}
          >
            {t("preferencesUi.resetCorners")}
          </button>
          <button
            disabled={isIdentity}
            onClick={() => applyTransform({ ...DEFAULT_OUTPUT_TRANSFORM })}
            style={{
              ...buttonStyle(false),
              color: isIdentity ? "var(--wc-text-faint)" : "var(--wc-text)",
              cursor: isIdentity ? "default" : "pointer",
            }}
          >
            {t("preferencesUi.resetAll")}
          </button>
        </div>
      </div>

      <div style={{ marginBottom: 24 }}>
        <div style={sectionLabelStyle}>{t("preferencesUi.testPatterns")}</div>
        <div style={{ fontSize: 11, color: "var(--wc-text-faint)", marginBottom: 10 }}>
          {t("preferencesUi.patternHint")}{defaultOutputName ? ` “${defaultOutputName}”` : ""}.
        </div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 8 }}>
          {PATTERNS.map((p) => (
            <button
              key={p.kind}
              title={t(`preferencesUi.${p.hintKey}`)}
              onClick={() => void togglePattern(p.kind)}
              style={buttonStyle(activePattern === p.kind)}
            >
              {t(`preferencesUi.${p.labelKey}`)}
            </button>
          ))}
          <button
            title={customImagePath ?? t("preferencesUi.showColorimetry")}
            onClick={() => void pickCustomImage()}
            style={buttonStyle(activePattern === "custom_image")}
          >
            {t("preferencesUi.customImage")}
          </button>
        </div>
        <button
          disabled={activePattern === null}
          onClick={() => {
            setActivePattern(null);
            void clearTestPattern().catch(console.error);
          }}
          style={{
            ...buttonStyle(false),
            color: activePattern === null ? "var(--wc-text-faint)" : "var(--wc-text)",
            cursor: activePattern === null ? "default" : "pointer",
          }}
        >
          {t("preferencesUi.hidePattern")}
        </button>
      </div>
    </>
  );
}
