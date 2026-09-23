import { getCurrentWindow } from "@tauri-apps/api/window";
import { DiagnosticsPage } from "./DiagnosticsPage";

export function DiagnosticsStandalone() {
  return <DiagnosticsPage onClose={() => void getCurrentWindow().hide()} />;
}
