import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { InkueMark } from "../common/InkueMark";
import { useLocale } from "../../i18n";
import { useUpdateStore } from "../../stores/updateStore";
import { openExternalUrl } from "../../lib/commands";
import licenseText from "../../../LICENSE?raw";
import thirdPartyNotices from "../../../THIRD_PARTY_NOTICES.md?raw";

type AboutDocument = "license" | "thirdParty";

export function AboutDialog({ onClose }: { onClose: () => void }) {
  const { t } = useLocale();
  const [version, setVersion] = useState("…");
  const [document, setDocument] = useState<AboutDocument | null>(null);

  useEffect(() => {
    void getVersion().then(setVersion).catch(() => setVersion(import.meta.env.VITE_APP_VERSION));
  }, []);

  return (
    <div
      style={{
        position: "fixed", inset: 0, zIndex: 99999,
        background: "rgba(0,0,0,0.6)",
        display: "flex", alignItems: "center", justifyContent: "center",
      }}
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)",
          borderRadius: 12, padding: "28px 32px", width: 420,
          boxShadow: "0 16px 48px rgba(0,0,0,0.8)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
          <InkueMark size={28} />
          <span style={{ fontSize: 20, fontWeight: 700, color: "var(--wc-text-bright)" }}>Qlisa</span>
          <span style={{ fontSize: 13, color: "var(--wc-text-muted)" }}>v{version}</span>
        </div>

        <div style={{ fontSize: 12, color: "var(--wc-text-secondary)", marginBottom: 20 }}>
          {t("aboutUi.description")}
        </div>
        <div style={{ fontSize: 11, color: "var(--wc-text-muted)", marginTop: -12, marginBottom: 18, lineHeight: 1.45 }}>
          {t("aboutUi.mediaRuntime")}
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 6, fontSize: 12, color: "var(--wc-text-secondary)", marginBottom: 20 }}>
          <Row label={t("aboutUi.builtWith")} value="Tauri v2 · Rust · React · TypeScript" />
          <Row label={t("aboutUi.audio")} value="cpal · symphonia" />
          <Row label={t("aboutUi.video")} value="libmpv (OpenGL Render API)" />
          <Row label={t("aboutUi.dmx")} value="sACN E1.31 · Art-Net" />
        </div>

        <div
          style={{
            background: "rgba(255,255,255,0.04)", border: "1px solid var(--wc-border)",
            borderRadius: 6, padding: "10px 12px", fontSize: 11,
            color: "var(--wc-text-muted)", marginBottom: 20, lineHeight: 1.6,
          }}
        >
          <strong style={{ color: "var(--wc-text-secondary)" }}>Qlisa</strong> {t("aboutUi.legalFree")} {" "}
          <strong>GNU General Public License v3 or later</strong> (GPL-3.0-or-later).
          <br />
          {t("aboutUi.origin")}: Inkue by FonograF.
          <br />
          {t("aboutUi.ndiNotice")}
          <br />
          {t("aboutUi.qlabNotice")}
        </div>

        <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 16 }}>
          <button onClick={() => void openExternalUrl("https://github.com/Ruslan-mad/Qlisa").catch(console.error)} style={linkButtonStyle}>{t("aboutUi.qlisaSource")}</button>
          <button onClick={() => void openExternalUrl("https://github.com/FonograF/Inkue").catch(console.error)} style={linkButtonStyle}>{t("aboutUi.inkue")}</button>
          <button onClick={() => setDocument("license")} style={linkButtonStyle}>{t("aboutUi.license")}</button>
          <button onClick={() => setDocument("thirdParty")} style={linkButtonStyle}>{t("aboutUi.thirdParty")}</button>
        </div>

        <div style={{ display: "flex", justifyContent: "flex-end", alignItems: "center" }}>
          <button
            onClick={() => void useUpdateStore.getState().checkForUpdates()}
            style={{ marginRight: "auto", background: "transparent", border: "1px solid var(--wc-border-strong)", borderRadius: 6, color: "var(--wc-text)", cursor: "pointer", fontSize: 12, padding: "6px 10px" }}
          >{t("systemUi.checkUpdates")}</button>
          <button
            onClick={onClose}
            style={{
              background: "var(--wc-bg-hover)", border: "1px solid var(--wc-border-strong)",
              borderRadius: 6, color: "var(--wc-text)", cursor: "pointer",
              fontSize: 13, padding: "6px 18px",
            }}
          >
            {t("common.close")}
          </button>
        </div>
      </div>
      {document && <div
        style={{ position: "fixed", inset: 0, zIndex: 100001, background: "rgba(0,0,0,0.72)", display: "flex", alignItems: "center", justifyContent: "center", padding: 24 }}
        onClick={() => setDocument(null)}
      >
        <div onClick={(event) => event.stopPropagation()} style={{ width: 720, maxWidth: "90vw", maxHeight: "82vh", display: "flex", flexDirection: "column", background: "var(--wc-bg-surface)", border: "1px solid var(--wc-border-strong)", borderRadius: 10, padding: 20, boxShadow: "0 16px 48px rgba(0,0,0,0.8)" }}>
          <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
            <strong style={{ color: "var(--wc-text-bright)" }}>{t(document === "license" ? "aboutUi.license" : "aboutUi.thirdParty")}</strong>
            <button onClick={() => setDocument(null)} style={{ ...linkButtonStyle, marginLeft: "auto" }}>{t("common.close")}</button>
          </div>
          <pre style={{ margin: 0, overflow: "auto", whiteSpace: "pre-wrap", overflowWrap: "anywhere", font: "12px/1.55 ui-monospace, Consolas, monospace", color: "var(--wc-text-secondary)" }}>
            {document === "license" ? licenseText : thirdPartyNotices}
          </pre>
        </div>
      </div>}
    </div>
  );
}

const linkButtonStyle: React.CSSProperties = {
  background: "transparent", border: "1px solid var(--wc-border-strong)", borderRadius: 6,
  color: "var(--wc-text)", cursor: "pointer", fontSize: 12, padding: "6px 10px",
};

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: "flex", gap: 8 }}>
      <span style={{ width: 70, flexShrink: 0, color: "var(--wc-text-faint)" }}>{label}</span>
      <span>{value}</span>
    </div>
  );
}
