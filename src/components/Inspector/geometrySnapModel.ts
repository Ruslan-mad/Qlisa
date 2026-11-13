export interface GeometryCenterDetentState {
  latched: boolean;
  holdNextMove: boolean;
  releasePosition: number | null;
}

export function idleGeometryCenterDetent(): GeometryCenterDetentState {
  return { latched: false, holdNextMove: false, releasePosition: null };
}

export function advanceGeometryCenterDetent(
  state: GeometryCenterDetentState,
  previousX: number,
  currentX: number,
  centerX: number,
  activeTrackWidth: number,
  enterRadiusPx = 8,
  exitRadiusPx = 14,
): GeometryCenterDetentState {
  const distance = Math.abs(currentX - centerX);
  const crossedCenter = previousX !== currentX
    && ((previousX < centerX && currentX >= centerX) || (previousX > centerX && currentX <= centerX));

  if (!state.latched) {
    if (distance <= enterRadiusPx || crossedCenter) {
      return {
        latched: true,
        holdNextMove: crossedCenter && distance > exitRadiusPx,
        releasePosition: null,
      };
    }
    return state;
  }

  if (state.holdNextMove) return { ...state, holdNextMove: false };
  if (distance <= exitRadiusPx) return state;

  const side = currentX < centerX ? -1 : 1;
  const activeWidth = Math.max(1, activeTrackWidth);
  return {
    latched: false,
    holdNextMove: false,
    releasePosition: Math.min(1, Math.max(0, 0.5 + side * exitRadiusPx / activeWidth)),
  };
}

export function releaseGeometryCenterDetent(
  state: GeometryCenterDetentState,
  currentX: number,
  centerX: number,
  activeTrackWidth: number,
  exitRadiusPx = 14,
): GeometryCenterDetentState {
  if (!state.latched || Math.abs(currentX - centerX) <= exitRadiusPx) return state;
  const side = currentX < centerX ? -1 : 1;
  const activeWidth = Math.max(1, activeTrackWidth);
  return {
    latched: false,
    holdNextMove: false,
    releasePosition: Math.min(1, Math.max(0, 0.5 + side * exitRadiusPx / activeWidth)),
  };
}

export function geometryValueAtDetentPosition(
  position: number,
  min: number,
  max: number,
  neutral: number,
  state: GeometryCenterDetentState,
): number {
  if (state.latched) return neutral;
  const bounded = Math.min(1, Math.max(0, position));
  const anchor = state.releasePosition;
  if (anchor == null) return centeredValue(bounded, min, max, neutral);

  if (anchor < 0.5) {
    if (bounded >= anchor) return neutral;
    return min + (neutral - min) * bounded / Math.max(anchor, Number.EPSILON);
  }
  if (bounded <= anchor) return neutral;
  return neutral + (max - neutral) * (bounded - anchor) / Math.max(1 - anchor, Number.EPSILON);
}

export function geometryValueAtDetentRelease(
  position: number,
  min: number,
  max: number,
  neutral: number,
  state: GeometryCenterDetentState,
): number {
  return geometryValueAtDetentPosition(position, min, max, neutral, {
    latched: false,
    holdNextMove: false,
    releasePosition: state.releasePosition,
  });
}

export function geometrySliderPositionFromPointer(
  clientX: number,
  inputLeft: number,
  inputWidth: number,
  thumbWidth = 16,
): number {
  const activeTrackWidth = Math.max(1, inputWidth - thumbWidth);
  return Math.min(1, Math.max(0, (clientX - inputLeft - thumbWidth / 2) / activeTrackWidth));
}

export function geometryEffectivePointerX(clientX: number, grabOffset: number): number {
  return clientX - grabOffset;
}

function centeredValue(position: number, min: number, max: number, neutral: number): number {
  return position <= 0.5
    ? min + (neutral - min) * position * 2
    : neutral + (max - neutral) * (position - 0.5) * 2;
}
