/** Resolve the editor cursor used when a headphone audition starts. */
export function headphonePreviewStartPosition(
  isNumber: boolean,
  numberPositionMs: number,
  previewPositionMs: number | null,
  runtimePositionMs: number | null,
  trimStartMs: number | null | undefined,
): number {
  if (isNumber) return Math.max(0, numberPositionMs);
  return Math.max(0, previewPositionMs ?? runtimePositionMs ?? trimStartMs ?? 0);
}

export function editorPreviewCursorPosition(
  isVideo: boolean,
  videoPositionMs: number | null,
  fallbackPositionMs: number | null,
): number | null {
  return isVideo ? videoPositionMs ?? fallbackPositionMs : fallbackPositionMs;
}

/** Mirror a visual play/pause edge to the active headphone transport. */
export function mirrorHeadphonePlayback(
  currentlyPlaying: boolean,
  hasHeadphoneSession: boolean,
  send: (action: "pause" | "resume") => unknown,
): boolean {
  if (!hasHeadphoneSession) return false;
  send(currentlyPlaying ? "pause" : "resume");
  return true;
}

/** Send one explicit seek edge. Pointer-move updates do not call this helper. */
export function seekHeadphonePreview(
  hasHeadphoneSession: boolean,
  positionMs: number,
  send: (positionMs: number, pauseBeforeSeek: boolean) => unknown,
  pauseBeforeSeek = false,
): boolean {
  if (!hasHeadphoneSession) return false;
  send(Math.max(0, Math.round(positionMs)), pauseBeforeSeek);
  return true;
}

/** Run a frame-step state transition, then send the resulting paused cursor. */
export function stepPreviewAndSyncHeadphones(
  step: () => number,
  onSeek?: (positionMs: number, pauseBeforeSeek: boolean) => void,
): number {
  const positionMs = step();
  onSeek?.(positionMs, true);
  return positionMs;
}

export type PreviewSessionAction = "pause" | "resume" | "seek" | "seek_paused" | "mute" | "unmute";

/** Owns preview intent while native decode and control commands are pending. */
export class PreviewSessionController {
  private running = false;
  private session = false;
  private disposed = false;
  private generation = 0;
  private sessionEpoch = 0;
  private failedGeneration = -1;
  private desiredPlaying = false;
  private desiredSound = false;
  private desiredPosition = 0;
  private starting = false;
  private appliedPlaying = false;
  private appliedSound = false;
  private appliedPosition = -1;

  constructor(private readonly transport: {
    start: (positionMs: number, soundEnabled: boolean) => Promise<unknown>;
    control: (action: PreviewSessionAction, positionMs?: number, soundEnabled?: boolean) => Promise<unknown>;
    onSession: (active: boolean, result?: unknown) => void;
    onError: (error: unknown) => void;
    currentPosition?: () => number;
    cancelStart?: (result: unknown) => void;
  }) {}

  setIntent(playing: boolean, soundEnabled: boolean, positionMs: number): void {
    this.desiredPlaying = playing;
    this.desiredSound = soundEnabled;
    if (!this.session && !this.starting) this.desiredPosition = Math.max(0, Math.round(positionMs));
    this.generation++;
    this.failedGeneration = -1;
    void this.pump();
  }

  seek(positionMs: number, pauseBeforeSeek = false): void {
    this.desiredPosition = Math.max(0, Math.round(positionMs));
    if (pauseBeforeSeek) this.desiredPlaying = false;
    this.generation++;
    this.failedGeneration = -1;
    void this.pump();
  }

  end(): void {
    this.sessionEpoch++;
    this.session = false;
    this.desiredPlaying = false;
    this.appliedPlaying = false;
    this.appliedPosition = -1;
    this.transport.onSession(false);
  }

  activate(): void {
    this.sessionEpoch++;
    this.disposed = false;
    this.session = false;
    this.starting = false;
    this.desiredPlaying = false;
    this.appliedPlaying = false;
    this.appliedSound = false;
    this.appliedPosition = -1;
    this.generation++;
    this.failedGeneration = -1;
  }

  dispose(): void {
    this.disposed = true;
    this.sessionEpoch++;
    this.session = false;
    this.desiredPlaying = false;
    this.generation++;
  }

  private async pump(): Promise<void> {
    if (this.running || this.disposed || this.failedGeneration === this.generation) return;
    this.running = true;
    const operationEpoch = this.sessionEpoch;
    try {
      while (!this.disposed) {
        if (!this.session) {
          if (!this.desiredPlaying || !this.desiredSound) break;
          const generation = this.generation;
          const epoch = this.sessionEpoch;
          const startPosition = this.desiredPosition;
          const startSound = this.desiredSound;
          // Always decode paused. Latest cursor, mute, and play intent are
          // applied after decode, so a late start can never autoplay stale UI state.
          this.starting = true;
          const started = await this.transport.start(startPosition, startSound);
          this.starting = false;
          if (this.disposed || epoch !== this.sessionEpoch) {
            this.transport.cancelStart?.(started);
            break;
          }
          this.session = true;
          this.appliedPosition = startPosition;
          this.appliedSound = startSound;
          this.appliedPlaying = false;
          const currentPosition = this.transport.currentPosition?.();
          if (currentPosition != null && Number.isFinite(currentPosition)) this.desiredPosition = Math.max(0, Math.round(currentPosition));
          this.transport.onSession(true, started);
          if (generation !== this.generation) continue;
        }

        const revision = this.generation;
        if (this.desiredPosition !== this.appliedPosition) {
          const epoch = this.sessionEpoch;
          const sentPosition = this.desiredPosition;
          const sentPlaying = this.desiredPlaying;
          const result = await this.transport.control(sentPlaying ? "seek" : "seek_paused", sentPosition, this.desiredSound);
          if (this.disposed || epoch !== this.sessionEpoch) break;
          this.appliedPosition = sentPosition;
          if (this.isEndedResult(result)) { this.end(); break; }
          this.transport.onSession(true, result);
          continue;
        }
        if (this.desiredSound !== this.appliedSound) {
          const epoch = this.sessionEpoch;
          const sentSound = this.desiredSound;
          const result = await this.transport.control(sentSound ? "unmute" : "mute", undefined, sentSound);
          if (this.disposed || epoch !== this.sessionEpoch) break;
          this.appliedSound = sentSound;
          if (this.isEndedResult(result)) { this.end(); break; }
          this.transport.onSession(true, result);
          continue;
        }
        if (this.desiredPlaying !== this.appliedPlaying) {
          const epoch = this.sessionEpoch;
          const sentPlaying = this.desiredPlaying;
          const result = await this.transport.control(sentPlaying ? "resume" : "pause", undefined, this.desiredSound);
          if (this.disposed || epoch !== this.sessionEpoch) break;
          this.appliedPlaying = sentPlaying;
          if (this.isEndedResult(result)) { this.end(); break; }
          this.transport.onSession(true, result);
          continue;
        }
        if (revision === this.generation) break;
      }
    } catch (error) {
      this.starting = false;
      if (this.disposed || operationEpoch !== this.sessionEpoch) return;
      this.failedGeneration = this.generation;
      this.session = false;
      this.appliedPosition = -1;
      this.transport.onSession(false);
      this.transport.onError(error);
    } finally {
      this.running = false;
      if (!this.disposed && this.failedGeneration !== this.generation && this.session && (
        this.desiredPosition !== this.appliedPosition
        || this.desiredSound !== this.appliedSound
        || this.desiredPlaying !== this.appliedPlaying
      )) void this.pump();
      else if (!this.disposed && this.failedGeneration !== this.generation && !this.session && this.desiredPlaying && this.desiredSound) void this.pump();
    }
  }

  private isEndedResult(result: unknown): boolean {
    return !!result && typeof result === "object" && "voice_id" in result
      && (result as { voice_id?: unknown }).voice_id == null;
  }
}
