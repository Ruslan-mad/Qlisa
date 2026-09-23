import { describe, expect, it } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import indexHtml from "../../index.html?raw";
import { QLISA_FAVICON_SRC, QLISA_LOGO_ALT, QLISA_LOGO_SRC } from "./brandAssets";

describe("Qlisa brand assets", () => {
  it("uses the new logo path and accessible alt text", () => {
    expect(QLISA_LOGO_SRC).toBe("/qlisa-logo.png");
    expect(QLISA_LOGO_ALT).toBe("Qlisa");
  });

  it("uses the Qlisa favicon and does not reference the legacy runtime favicon", () => {
    expect(QLISA_FAVICON_SRC).toBe("/favicon.ico");
    expect(indexHtml).toContain('href="/favicon.ico"');
    expect(indexHtml).not.toContain("inkue-icon.svg");
  });

  it("ships every icon referenced by the web manifest", () => {
    const manifestPath = resolve(process.cwd(), "public", "site.webmanifest");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as {
      icons: Array<{ src: string }>;
    };

    expect(manifest.icons.length).toBeGreaterThan(0);
    for (const icon of manifest.icons) {
      expect(existsSync(resolve(process.cwd(), "public", icon.src.slice(1)))).toBe(true);
    }
  });
});
