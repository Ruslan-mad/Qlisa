import { describe, expect, it } from "vitest";
import {
  cloneOutputTransform,
  cloneNetworkOutputSettings,
  createOutputId,
  DEFAULT_OUTPUT_DESTINATION,
  getDefaultOutputDestination,
  normalizeOutputDestinations,
  validateOutputDestinations,
  type OutputDestination,
} from "../types";

const destination = (overrides: Partial<OutputDestination> = {}): OutputDestination => ({
  ...DEFAULT_OUTPUT_DESTINATION,
  ...overrides,
  transform: cloneOutputTransform(overrides.transform ?? DEFAULT_OUTPUT_DESTINATION.transform),
});

describe("named output preference helpers", () => {
  it("creates independent nested transforms for drafts", () => {
    const original = destination();
    const normalized = normalizeOutputDestinations([original], "default");

    normalized.destinations[0].transform.corners[0][0] = 0.25;
    expect(original.transform.corners[0][0]).toBe(0);
    expect(DEFAULT_OUTPUT_DESTINATION.transform.corners[0][0]).toBe(0);
  });

  it("keeps a requested enabled default and repairs removed/disabled defaults", () => {
    const outputs = [
      destination({ id: "main", name: "Main", enabled: true }),
      destination({ id: "projector", name: "Projector", enabled: true }),
    ];
    expect(normalizeOutputDestinations(outputs, "projector").defaultOutputId).toBe("projector");
    expect(normalizeOutputDestinations(outputs, "missing").defaultOutputId).toBe("main");
    expect(normalizeOutputDestinations([
      outputs[0],
      { ...outputs[1], enabled: false },
    ], "projector").defaultOutputId).toBe("main");
  });

  it("migrates legacy display settings and forces fullscreen safety", () => {
    const migrated = normalizeOutputDestinations(undefined, undefined, 2);
    expect(migrated.destinations).toHaveLength(1);
    expect(migrated.destinations[0].monitor).toBe(2);
    expect(migrated.destinations[0].fullscreen_locked).toBe(true);
    expect(migrated.destinations[0].always_on_top).toBe(false);
    expect(migrated.destinations[0].hide_cursor).toBe(false);

    const unsafe = destination({ fullscreen_locked: false });
    expect(normalizeOutputDestinations([unsafe], "default").destinations[0].fullscreen_locked).toBe(true);
  });

  it("defaults legacy window behaviour flags and preserves explicit choices", () => {
    const legacy = { ...destination() } as Partial<OutputDestination>;
    delete legacy.always_on_top;
    delete legacy.hide_cursor;
    const normalizedLegacy = normalizeOutputDestinations([legacy as OutputDestination], "default").destinations[0];
    expect(normalizedLegacy.always_on_top).toBe(false);
    expect(normalizedLegacy.hide_cursor).toBe(false);

    const configured = normalizeOutputDestinations([
      destination({ always_on_top: true, hide_cursor: true }),
    ], "default").destinations[0];
    expect(configured.always_on_top).toBe(true);
    expect(configured.hide_cursor).toBe(true);
  });

  it("returns a unique UUID-shaped id for each new output", () => {
    const first = createOutputId();
    const second = createOutputId();
    expect(first).not.toBe(second);
    expect(first).toMatch(/^[0-9a-f-]{36}$/i);
  });

  it("resolves the requested default only when it is enabled", () => {
    const outputs = [
      destination({ id: "main", name: "Main", enabled: true }),
      destination({ id: "projector", name: "Projector", enabled: false }),
    ];
    expect(getDefaultOutputDestination(outputs, "projector")?.id).toBe("main");
  });

  it("rejects blank, padded, duplicate, and all-disabled output lists", () => {
    expect(validateOutputDestinations([destination({ id: " ", name: "Main" })])).toMatch(/непустые/);
    expect(validateOutputDestinations([destination({ id: " main ", name: "Main" })])).toMatch(/пробелы/);
    expect(validateOutputDestinations([
      destination({ id: "main", name: "Main" }),
      destination({ id: "main", name: "Projector" }),
    ])).toMatch(/используется несколько раз/);
    expect(validateOutputDestinations([destination({ enabled: false })])).toMatch(/Включите/);
  });

  it("rejects duplicate monitors only for enabled display outputs", () => {
    const duplicate = [
      destination({ id: "main", name: "Main", monitor: 0 }),
      destination({ id: "preview", name: "Preview", monitor: 0 }),
    ];
    expect(validateOutputDestinations(duplicate)).toMatch(/Монитор 1/);
    expect(validateOutputDestinations([
      duplicate[0],
      { ...duplicate[1], enabled: false },
    ])).toBeNull();
  });

  it("migrates network output settings and keeps stream defaults isolated", () => {
    const network = cloneNetworkOutputSettings();
    const output = destination({ id: "ndi", name: "NDI", sink_kind: "ndi", network });
    const normalized = normalizeOutputDestinations([destination(), output], "ndi");
    const ndi = normalized.destinations.find((item) => item.id === "ndi")!;
    expect(normalized.defaultOutputId).toBe("default");
    expect(ndi.network?.ndi.stream_name).toBe("Qlisa Program");
    ndi.network!.ndi.stream_name = "Preview";
    expect(network.ndi.stream_name).toBe("Qlisa Program");
  });

  it("validates enabled NDI and SRT endpoint basics", () => {
    const ndi = destination({ id: "ndi", name: "NDI", sink_kind: "ndi", network: { ...cloneNetworkOutputSettings(), ndi: { ...cloneNetworkOutputSettings().ndi, enabled: true, stream_name: " " } } });
    expect(validateOutputDestinations([destination(), ndi])).toMatch(/имя потока/);
    const srt = destination({ id: "srt", name: "SRT", sink_kind: "srt", network: { ...cloneNetworkOutputSettings(), srt: { ...cloneNetworkOutputSettings().srt, enabled: true, mode: "caller", host: "", port: 9000 } } });
    expect(validateOutputDestinations([destination(), srt])).toMatch(/IP-адрес/);
  });
});
