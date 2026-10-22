import { SpeakerHigh } from "@phosphor-icons/react";
import { useRef, type CSSProperties, type KeyboardEvent } from "react";
import { MASTER_VOLUME_MAX_DB, MASTER_VOLUME_MIN_DB, MASTER_VOLUME_STEP_DB, masterVolumeDbToRatio, normalizeMasterVolumeInput, stepMasterVolume } from "./masterVolumeModel";

export const METER_MIN_DB = -60;
export const METER_MAX_DB = 0;
const METER_DB_RANGE = METER_MAX_DB - METER_MIN_DB;
const DB_TICKS = [0, -6, -12, -18, -24, -36];

export function dbToRatio(db: number): number {
  return Math.max(0, Math.min(1, (db - METER_MIN_DB) / METER_DB_RANGE));
}

export function peakToFillPct(linear: number): number {
  const db = linear > 0 ? 20 * Math.log10(linear) : -Infinity;
  return dbToRatio(db) * 100;
}

interface MeterRowProps {
  label: string;
  fillPct: number;
  holdPct: number;
}

function MeterRow({ label, fillPct, holdPct }: MeterRowProps) {
  return (
    <div className="transport-meter-row">
      <span className="transport-meter-row__channel">{label}</span>
      <div className="transport-meter-rail" role="meter" aria-label={`Master ${label} level`} aria-valuemin={METER_MIN_DB} aria-valuemax={METER_MAX_DB} aria-valuenow={fillPct > 0 ? METER_MIN_DB + fillPct / 100 * METER_DB_RANGE : METER_MIN_DB}>
        <div className="transport-meter-rail__fill" style={{ clipPath: `inset(0 ${100 - fillPct}% 0 0)` }} />
        <div className="transport-meter-rail__segments" />
        {holdPct > 0 && (
          <div
            className="transport-meter-rail__peak"
            data-clipping={holdPct > 90}
            style={{ left: `calc(${Math.min(holdPct, 100)}% - 2px)` }}
          />
        )}
      </div>
    </div>
  );
}

function TickRow() {
  return (
    <div className="transport-meter-scale">
      <span className="transport-meter-scale__unit">dB</span>
      <div className="transport-meter-scale__rail">
        {DB_TICKS.map((db) => (
          <div key={db} className="transport-meter-scale__tick" style={{ left: `${dbToRatio(db) * 100}%` }}>
            <span>{db}</span>
            <i />
          </div>
        ))}
      </div>
    </div>
  );
}

function VolumeRow({ label, valueDb, onChange }: { label: string; valueDb: number; onChange: (db: number) => void }) {
  const pointerActive = useRef(false);
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    pointerActive.current = false;
    if (event.key !== "ArrowLeft" && event.key !== "ArrowDown" && event.key !== "ArrowRight" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const direction = event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 1;
    onChange(stepMasterVolume(valueDb, direction));
  };
  return (
    <div className="transport-master-row__control">
      <div className="transport-master-slider-wrap">
        <input
          type="range"
          aria-label={label}
          min={MASTER_VOLUME_MIN_DB}
          max={MASTER_VOLUME_MAX_DB}
          step={MASTER_VOLUME_STEP_DB}
          value={valueDb}
          onChange={(event) => onChange(normalizeMasterVolumeInput(Number(event.target.value), pointerActive.current ? "pointer" : "keyboard"))}
          onPointerDown={(event) => { pointerActive.current = true; event.currentTarget.setPointerCapture(event.pointerId); }}
          onPointerUp={() => { pointerActive.current = false; }}
          onPointerCancel={() => { pointerActive.current = false; }}
          onLostPointerCapture={() => { pointerActive.current = false; }}
          onKeyDown={onKeyDown}
          className="transport-master-slider"
          style={{ "--volume-progress": `${masterVolumeDbToRatio(valueDb) * 100}%` } as CSSProperties}
        />
        <div className="transport-master-slider__unity" aria-hidden="true"><span style={{ left: `${masterVolumeDbToRatio(0) * 100}%` }}>0</span></div>
      </div>
      <span className="transport-master-row__value">{valueDb >= 0 ? `+${valueDb.toFixed(1)}` : valueDb.toFixed(1)} dB</span>
    </div>
  );
}

export function TransportAudioMeters({
  left,
  right,
  volumeDb,
  volumeLabel,
  onVolumeChange,
}: {
  left: { fill: number; hold: number };
  right: { fill: number; hold: number };
  volumeDb: number;
  volumeLabel: string;
  onVolumeChange: (db: number) => void;
}) {
  return (
    <div className="stage-master-area">
      <TickRow />
      <MeterRow label="L" fillPct={left.fill} holdPct={left.hold} />
      <MeterRow label="R" fillPct={right.fill} holdPct={right.hold} />
      <div className="transport-master-row">
        <div className="transport-master-row__label"><SpeakerHigh size={14} weight="bold" aria-hidden="true" /><span>{volumeLabel}</span></div>
        <VolumeRow label={volumeLabel} valueDb={volumeDb} onChange={onVolumeChange} />
      </div>
    </div>
  );
}
