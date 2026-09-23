import { describe, expect, it } from "vitest";
import { createBrowserPreviewModel, validateBrowserPreviewUrl } from "./browserPreviewModel";

describe("browser preview model", () => {
  it("accepts HTTP(S) localhost pages and canonicalises them", () => {
    expect(validateBrowserPreviewUrl("  http://localhost:3000/dashboard ")).toBe(
      "http://localhost:3000/dashboard",
    );
    expect(validateBrowserPreviewUrl("https://example.test/live")).toBe("https://example.test/live");
  });

  it("rejects non-web, malformed, and hostless URLs", () => {
    expect(validateBrowserPreviewUrl("")).toBeNull();
    expect(validateBrowserPreviewUrl("file:///tmp/page.html")).toBeNull();
    expect(validateBrowserPreviewUrl("javascript:alert(1)")).toBeNull();
    expect(validateBrowserPreviewUrl("data:text/html,hello")).toBeNull();
    expect(validateBrowserPreviewUrl("http://")).toBeNull();
  });

  it("does not create a frame source for an invalid URL", () => {
    const model = createBrowserPreviewModel("not a URL");
    expect(model.src).toBeNull();
    expect(model.status).toBe("invalid");
    expect(model.frameKey).toBe("invalid-browser-preview");
  });

  it("changes iframe identity only when the URL or reload nonce changes", () => {
    const first = createBrowserPreviewModel("http://localhost:3000", 0);
    const same = createBrowserPreviewModel("http://localhost:3000", 0);
    const reloaded = createBrowserPreviewModel("http://localhost:3000", 1);
    const otherPage = createBrowserPreviewModel("http://localhost:3001", 0);
    expect(same.frameKey).toBe(first.frameKey);
    expect(reloaded.frameKey).not.toBe(first.frameKey);
    expect(otherPage.frameKey).not.toBe(first.frameKey);
  });
});
