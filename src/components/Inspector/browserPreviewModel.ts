export type BrowserPreviewStatus = "invalid" | "loading";

export interface BrowserPreviewModel {
  src: string | null;
  frameKey: string;
  status: BrowserPreviewStatus;
}

/** Return a browser-loadable HTTP(S) URL, or null without throwing. */
export function validateBrowserPreviewUrl(raw: string): string | null {
  const value = raw.trim();
  if (!value) return null;
  try {
    const url = new URL(value);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || !url.hostname) return null;
    return url.toString();
  } catch {
    return null;
  }
}

/** The key is separate from src so Reload can recreate the iframe safely. */
export function createBrowserPreviewModel(raw: string, reloadNonce = 0): BrowserPreviewModel {
  const src = validateBrowserPreviewUrl(raw);
  return {
    src,
    frameKey: src ? `${src}::preview-${reloadNonce}` : "invalid-browser-preview",
    status: src ? "loading" : "invalid",
  };
}
