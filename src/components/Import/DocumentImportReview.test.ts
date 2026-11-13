import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { getLocale, setLocale } from "../../i18n/locale";
import { DocumentImportReview, type DocumentImportReviewResult } from "./DocumentImportReview";

const result: DocumentImportReviewResult = {
  cue_id: "presentation-1",
  title: "Sample deck",
  source: "C:/Shows/sample.pptx",
  page_count: 1,
  video_count: 1,
  warnings: [],
  report: [],
  media_scan_error: null,
  slides: [{
    number: 2,
    image_cue_id: "page-1",
    image_path: "C:/Shows/page.png",
    document_title: "Sample deck",
    document_source: "C:/Shows/sample.pptx",
    videos: [
      { name: "clip.mp4", status: "added", cue_id: "video-1", error: null, warning: "Original slide placement is unsupported." },
      { name: "missing.mp4", status: "missing", cue_id: null, error: "File not found.", warning: null },
    ],
  }],
};

describe("presentation import review", () => {
  it("shows source, slide, successful video note, and a distinct missing-video failure", () => {
    const previousLocale = getLocale();
    setLocale("en");
    const markup = renderToStaticMarkup(createElement(DocumentImportReview, {
      pending: false,
      result,
      error: null,
      notice: null,
      onClose: () => undefined,
    }));
    setLocale(previousLocale);

    expect(markup).toContain("Sample deck");
    expect(markup).toContain("sample.pptx");
    expect(markup).toContain("Slide 2");
    expect(markup).toContain("Video added: clip.mp4");
    expect(markup).toContain("Original slide placement is unsupported.");
    expect(markup).toContain("Video not added: missing.mp4. File not found.");
    expect(markup).toContain("aria-modal=\"true\"");
  });

  it("shows a non-modal preparation status while an import is pending", () => {
    const previousLocale = getLocale();
    setLocale("en");
    const markup = renderToStaticMarkup(createElement(DocumentImportReview, {
      pending: true,
      result: null,
      error: null,
      notice: null,
      onClose: () => undefined,
    }));
    setLocale(previousLocale);

    expect(markup).toContain("Preparing presentation");
    expect(markup).not.toContain("Close");
    expect(markup).not.toContain("aria-modal=\"true\"");
  });
});
