// The line health dashboard with its charts swapped for stand-ins (jsdom has no canvas): the
// numbers, the attention list, filters in the address, clicks that filter or open a tower, and
// the rules behind them.

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import * as S from "../api/schemas";
import { fakeApi, ME } from "../test/fake-api";
import { renderApp } from "../test/render";
import { foldTally, TALLY_BARS, type Tally } from "./charts";
import { checkWide, defaultLayouts, loadWide, saveWide } from "./layout";
import { attention, defectChoices, filtersFrom, Labels, partChoices } from "./model";

import healthFixture from "../api/fixtures/health.json";
import schemaFixture from "../api/fixtures/schema.json";
import surveyFixture from "../api/fixtures/survey.json";

vi.mock("./EChart", () => ({
  default: ({ label, onPick }: { label: string; onPick?: (s: number, i: number) => void }) => (
    <div role="img" aria-label={label}>
      {onPick ? (
        <button type="button" onClick={() => onPick(0, 0)}>
          pick first
        </button>
      ) : null}
    </div>
  ),
}));

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const SURVEY = S.SurveyDetail.parse(surveyFixture);
const SCHEMA = S.DomainSchema.parse(schemaFixture);
const HEALTH = S.LineHealth.parse(healthFixture);
const ID = SURVEY.summary.id;

function api() {
  return fakeApi({
    me: vi.fn(() => Promise.resolve(ME)),
    survey: vi.fn(() => Promise.resolve(SURVEY)),
    schema: vi.fn(() => Promise.resolve(SCHEMA)),
    lineHealth: vi.fn((_id: string, query: S.HealthQuery) => Promise.resolve({ ...HEALTH, query })),
  });
}

const here = () => screen.getByTestId("here").textContent;

describe("the line health page", () => {
  it("shows the headline numbers, the charts and the towers needing attention", async () => {
    renderApp(api(), `/surveys/${ID}/health`);
    const glance = await screen.findByRole("region", { name: "At a glance" });
    const tile = (name: string) => within(glance).getByText(name).nextElementSibling?.textContent;
    expect(tile("Towers")).toBe("3");
    expect(tile("Inspected")).toBe("2");
    expect(tile("Need action (high or critical)")).toBe("1");
    expect(tile("Confirmed findings")).toBe("2");
    expect(tile("Waiting for review")).toBe("1");
    expect(await screen.findByRole("img", { name: /Inspected towers by their worst finding/ })).toBeTruthy();

    const table = within(screen.getByRole("region", { name: "Towers needing attention (1)" })).getByRole("table");
    await waitFor(() =>
      expect(
        within(table)
          .getAllByRole("cell")
          .map((c) => c.textContent),
      ).toEqual(["101", "Critical", "Hot joint or connector, Y phase insulator, left circuit", "2", "1"]),
    );
    expect(within(table).getByRole("link", { name: "101" }).getAttribute("href")).toBe(`/surveys/${ID}?tower=t1`);

    const peaks = within(screen.getByRole("region", { name: "Highest readings" }));
    expect(peaks.getAllByRole("row").map((r) => r.textContent)).toEqual([
      "ReadingValueTower",
      "Hottest spot78.2 °C101",
      "Hottest rise41.7 °C101",
    ]);
  });

  it("keeps the filters in the address and asks the server with them", async () => {
    const a = api();
    renderApp(a, `/surveys/${ID}/health`);
    const severity = await screen.findByRole("combobox", { name: "Severity" });
    fireEvent.change(severity, { target: { value: "high" } });
    await waitFor(() => expect(here()).toBe(`/surveys/${ID}/health?minSeverity=high`));
    await waitFor(() => expect(a.lineHealth).toHaveBeenLastCalledWith(ID, { minSeverity: "high" }, expect.anything()));
    expect(await screen.findByRole("region", { name: "At a glance, matching the filters" })).toBeTruthy();

    // The part list offers families first ("Insulator, all") then each part.
    const part = screen.getByRole("combobox", { name: "Part" });
    expect(within(part).getByRole("option", { name: "Insulator, all" })).toBeTruthy();
    fireEvent.change(part, { target: { value: "insulator" } });
    await waitFor(() => expect(here()).toBe(`/surveys/${ID}/health?minSeverity=high&part=insulator`));
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(here()).toBe(`/surveys/${ID}/health`));
  });

  it("filters by a defect from its bar, and opens a tower from the line chart", async () => {
    renderApp(api(), `/surveys/${ID}/health`);
    const defects = await screen.findByRole("region", { name: "Findings by defect" });
    fireEvent.click(await within(defects).findByRole("button", { name: "pick first" }));
    await waitFor(() => expect(here()).toBe(`/surveys/${ID}/health?defect=hotspot.joint`));
    const along = screen.getByRole("region", { name: "Along the line" });
    fireEvent.click(await within(along).findByRole("button", { name: "pick first" }));
    await waitFor(() => expect(here()).toBe(`/surveys/${ID}?tower=t1`));
  });

  it("shows each chart's numbers as a table", async () => {
    renderApp(api(), `/surveys/${ID}/health`);
    const parts = await screen.findByRole("region", { name: "Findings by tower part" });
    fireEvent.click(within(parts).getByRole("button", { name: "Show table" }));
    // Labels come from the schema, which may arrive just after the numbers.
    await waitFor(() =>
      expect(
        within(parts)
          .getAllByRole("row")
          .map((r) => r.textContent),
      ).toEqual(["PartConfirmedWorst", "Y phase insulator, left circuit1Critical", "Whole tower1Medium"]),
    );
  });

  it("is linked from the survey page", async () => {
    const a = fakeApi({
      me: vi.fn(() => Promise.resolve(ME)),
      survey: vi.fn(() => Promise.resolve(SURVEY)),
      towers: vi.fn(() => Promise.resolve({ type: "FeatureCollection" as const, features: [] })),
      schema: vi.fn(() => Promise.resolve(SCHEMA)),
    });
    renderApp(a, `/surveys/${ID}`);
    expect((await screen.findByRole("link", { name: "Line health" })).getAttribute("href")).toBe(`/surveys/${ID}/health`);
  });

  it("says when the survey isn't there", async () => {
    renderApp(
      fakeApi({
        me: vi.fn(() => Promise.resolve(ME)),
        survey: vi.fn(() => Promise.resolve(null)),
        lineHealth: vi.fn(() => Promise.resolve(null)),
      }),
      `/surveys/srv_x/health`,
    );
    expect((await screen.findByRole("alert")).textContent).toMatch(/doesn't exist/);
  });
});

describe("the dashboard's rules", () => {
  it("reads only real filters from the address", () => {
    expect(filtersFrom(new URLSearchParams("minSeverity=high&defect=hotspot&part=insulator.left"))).toEqual({
      minSeverity: "high",
      defect: "hotspot",
      part: "insulator.left",
    });
    expect(filtersFrom(new URLSearchParams("minSeverity=none&defect=Hot%20Spot&part=&x=1"))).toEqual({});
    expect(filtersFrom(new URLSearchParams(`minSeverity=worst&part=${"a".repeat(65)}`))).toEqual({});
  });

  it("offers families with more than one member, then every type with its label", () => {
    const d = defectChoices(SCHEMA).map((c) => c.value);
    expect(d.slice(0, 5)).toEqual(["hotspot", "insulator", "conductor", "hardware", "tower"]);
    expect(d).toContain("bird_nest");
    expect(d).not.toContain("earthing"); // one member only
    expect(
      partChoices(SCHEMA)
        .slice(0, 3)
        .map((c) => c.label),
    ).toEqual(["Earthwire, all", "Insulator, all", "Leg, all"]);
    expect(defectChoices(null)).toEqual([]);
  });

  it("names unknown ids as themselves and no part as the whole tower", () => {
    const l = new Labels(SCHEMA);
    expect(l.defect("hotspot.joint")).toBe("Hot joint or connector");
    expect(l.defect("gone.away")).toBe("gone.away");
    expect(l.part(null)).toBe("Whole tower");
    expect(l.part("leg.a")).not.toBe("leg.a");
  });

  it("lists flagged towers worst first, then in walking order, up to the limit", () => {
    const row = (id: string, position: number, severity: S.Severity): S.TowerHealthRow => ({
      id,
      label: id,
      position,
      inspected: true,
      severity,
      findings: 1,
      pending: 0,
    });
    const rows = [row("a", 0, "low"), row("b", 1, "critical"), row("c", 2, "none"), row("d", 3, "critical"), row("e", 4, "medium")];
    expect(attention(rows).rows.map((r) => r.id)).toEqual(["b", "d", "e", "a"]);
    expect(attention(rows, 2)).toEqual({ rows: [rows[1], rows[3]], total: 4 });
  });

  it("folds a long tally into Others with the worst of them", () => {
    const items: Tally[] = Array.from({ length: 15 }, (_, i) => ({
      key: `d${i}`,
      label: `D${i}`,
      count: 15 - i,
      worst: i === 14 ? "critical" : "low",
    }));
    const folded = foldTally(items);
    expect(folded).toHaveLength(TALLY_BARS);
    expect(folded.at(-1)).toEqual({ key: null, label: "4 others", count: 4 + 3 + 2 + 1, worst: "critical" });
    expect(foldTally(items.slice(0, 3))).toEqual(items.slice(0, 3));
  });

  it("keeps a valid arrangement and ignores anything else", () => {
    const wide = defaultLayouts().lg;
    const moved = wide.map((p) => (p.i === "peaks" ? { ...p, x: 0, w: 12 } : p));
    saveWide(moved);
    expect(loadWide().find((p) => p.i === "peaks")).toMatchObject({ x: 0, w: 12, minW: 3 });
    expect(checkWide(wide.slice(1))).toBeNull(); // a panel missing
    expect(checkWide([...wide.slice(1), wide[1]])).toBeNull(); // one twice
    expect(checkWide(wide.map((p) => (p.i === "tiles" ? { ...p, x: 6 } : p)))).toBeNull(); // off the grid
    expect(checkWide(wide.map((p) => (p.i === "along" ? { ...p, w: 1.5 } : p)))).toBeNull();
    localStorage.setItem("geoflux.dashboard.layout.v1", "{not json");
    expect(loadWide()).toBe(wide);
  });
});
