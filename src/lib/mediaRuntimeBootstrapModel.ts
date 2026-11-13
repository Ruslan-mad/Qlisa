import type { MediaRuntimeStatus } from "./types";

export type MediaRuntimeStartupAction = "run" | "prepare" | "repair" | "invalid";
export type MediaRuntimeFailureActions = {
  view: "fallback" | "repair" | "error";
  primary: "schedule-update" | "schedule-repair" | "retry-missing";
  secondary?: "run-installed";
};

export function resolveMediaRuntimeStartup(status: MediaRuntimeStatus): MediaRuntimeStartupAction {
  if (status.operation_pending || status.state === "missing") return "prepare";
  if (status.state === "repair-required") return "repair";
  if (status.ready && (status.state === "ready" || status.state === "update-available")) return "run";
  return "invalid";
}

export function resolveMediaRuntimeFailure(status: MediaRuntimeStatus): MediaRuntimeFailureActions {
  if (status.ready && !status.operation_pending) {
    return { view: "fallback", primary: "schedule-update", secondary: "run-installed" };
  }
  if (status.state === "repair-required") return { view: "repair", primary: "schedule-repair" };
  return { view: "error", primary: "retry-missing" };
}
