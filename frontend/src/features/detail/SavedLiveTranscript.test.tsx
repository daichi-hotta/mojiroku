import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { I18nCtx, dicts } from "@/i18n";
import { SavedLiveCaptionRows } from "./SavedLiveTranscript";

const render = (rows: Parameters<typeof SavedLiveCaptionRows>[0]["rows"]) =>
  renderToStaticMarkup(<I18nCtx.Provider value={{ lang: "en", t: dicts.en, setLang: vi.fn() }}>
    <SavedLiveCaptionRows rows={rows} />
  </I18nCtx.Provider>);

describe("saved live transcript", () => {
  it("shows every saved caption in order and marks the unfinished line", () => {
    const html = render([
      { caption_id: 0, text: "First live line", committed: true },
      { caption_id: 1, text: "Second live line", committed: true },
      { caption_id: 2, text: "Cut off at", committed: false },
    ]);
    expect(html.indexOf("First live line")).toBeLessThan(html.indexOf("Second live line"));
    expect(html).toContain("Cut off at");
    expect(html.match(/Unfinished at Stop/g)).toHaveLength(1);
    expect(html).toContain("may differ from the final transcript");
  });

  it("explains when a recording has no live transcript", () => {
    expect(render([])).toContain("No live transcript was saved for this recording.");
  });
});
