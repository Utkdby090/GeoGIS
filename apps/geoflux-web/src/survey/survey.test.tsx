// The survey page with the map swapped for a stand-in (jsdom has no WebGL): the list, selecting a
// tower, its panel with photos and markup, walking the line, and the report links.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import * as S from "../api/schemas";
import { fakeApi, ME } from "../test/fake-api";
import { renderApp } from "../test/render";
import { markupElements } from "./markup";
import { groupFields } from "./TowerPanel";

import schemaFixture from "../api/fixtures/schema.json";
import surveyFixture from "../api/fixtures/survey.json";
import towerFixture from "../api/fixtures/tower.json";
import towersFixture from "../api/fixtures/towers.json";

vi.mock("./LineMap", () => ({
  default: ({ towers, selected, onSelect }: { towers: S.TowerCollection; selected: string | null; onSelect: (id: string) => void }) => (
    <div data-testid="map" data-selected={selected ?? ""}>
      {towers.features
        .filter((f) => f.geometry)
        .map((f) => (
          <button key={f.id} type="button" onClick={() => onSelect(f.id)}>
            dot {f.properties.label}
          </button>
        ))}
    </div>
  ),
}));

afterEach(cleanup);

const SURVEY = S.SurveyDetail.parse(surveyFixture);
const TOWERS = S.TowerCollection.parse(towersFixture);
const TOWER = S.TowerDetail.parse(towerFixture);
const SCHEMA = S.DomainSchema.parse(schemaFixture);
const ID = SURVEY.summary.id;

function api() {
  return fakeApi({
    me: vi.fn(() => Promise.resolve(ME)),
    survey: vi.fn(() => Promise.resolve(SURVEY)),
    towers: vi.fn(() => Promise.resolve(TOWERS)),
    tower: vi.fn((_s: string, id: string) => Promise.resolve({ ...TOWER, asset: { ...TOWER.asset, id } })),
    schema: vi.fn(() => Promise.resolve(SCHEMA)),
  });
}

describe("the survey page", () => {
  it("lists the towers in walking order and links the line's ZIP", async () => {
    renderApp(api(), `/surveys/${ID}`);
    const list = await screen.findByRole("navigation", { name: "Towers" });
    expect(within(list).getAllByRole("button").map((b) => b.textContent)).toEqual(["101critical", "102", "t3no positionmedium"]);
    expect(screen.getByRole("link", { name: "All tower PDFs (ZIP)" }).getAttribute("href")).toBe(`/report/${ID}`);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Line 4");
  });

  it("opens a tower from the map, shows its form, photos and markup, and walks the line", async () => {
    const a = api();
    renderApp(a, `/surveys/${ID}`);
    fireEvent.click(await screen.findByRole("button", { name: "dot 101" }));
    const panel = await screen.findByRole("region", { name: "Tower 101" });
    expect(screen.getByTestId("here").textContent).toBe(`/surveys/${ID}?tower=t1`);
    await within(panel).findByText("Hot spot on B phase jumper");
    // Schema order and labels; a field the tower has no value for reads N/A, as in the PDF.
    expect(within(panel).getByText("Fault Analysis")).toBeTruthy();
    expect(within(panel).getAllByText("N/A").length).toBeGreaterThan(0);
    // Every image slot, empty ones too, in the schema's order.
    const thermal = within(panel).getByRole("img", { name: "Thermal Image, tower 101" });
    expect(thermal.getAttribute("src")).toBe(`/photo/${ID}/${TOWER.photos[0]!.sha256}`);
    expect(within(panel).getAllByText("No image")).toHaveLength(2);
    // The markup is drawn once the photo's size is known.
    Object.defineProperty(thermal, "naturalWidth", { value: 640 });
    Object.defineProperty(thermal, "naturalHeight", { value: 512 });
    fireEvent.load(thermal);
    expect(panel.querySelectorAll("svg rect, svg circle")).toHaveLength(2);
    fireEvent.click(within(panel).getByRole("checkbox", { name: "Markup" }));
    expect(panel.querySelectorAll("svg rect")).toHaveLength(0);
    expect(within(panel).getByRole("link", { name: "Tower PDF" }).getAttribute("href")).toBe(`/report/${ID}/t1`);

    expect((within(panel).getByRole("button", { name: "Previous tower" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(within(panel).getByRole("button", { name: "Next tower" }));
    await screen.findByRole("region", { name: "Tower 102" });
    expect(a.tower).toHaveBeenLastCalledWith(ID, "t2", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: /^Tower/ })).toBeNull());
  });

  it("finds towers and keeps to findings when asked", async () => {
    renderApp(api(), `/surveys/${ID}`);
    const list = await screen.findByRole("navigation", { name: "Towers" });
    fireEvent.click(within(list).getByRole("checkbox", { name: "Findings only" }));
    expect(within(list).getAllByRole("button")).toHaveLength(2);
    fireEvent.change(within(list).getByRole("searchbox"), { target: { value: "10" } });
    expect(within(list).getAllByRole("button").map((b) => b.textContent)).toEqual(["101critical"]);
    fireEvent.change(within(list).getByRole("searchbox"), { target: { value: "zzz" } });
    expect(within(list).getByText("No towers match.")).toBeTruthy();
  });

  it("says when a survey isn't there", async () => {
    renderApp(fakeApi({ me: vi.fn(() => Promise.resolve(ME)), survey: vi.fn(() => Promise.resolve(null)), towers: vi.fn(() => Promise.resolve(null)) }), "/surveys/srv_x");
    expect((await screen.findByRole("alert")).textContent).toMatch(/doesn't exist/);
  });
});

describe("markup", () => {
  it("draws each shape where the PDF does", () => {
    const shapes = S.PhotoRef.parse(TOWER.photos[0]).markup ?? [];
    const { container } = render(<svg>{markupElements(shapes, 1000, 500)}</svg>);
    const rect = container.querySelector("rect")!;
    expect([rect.getAttribute("x"), rect.getAttribute("y"), rect.getAttribute("width"), rect.getAttribute("height")]).toEqual(["250", "125", "250", "250"]);
    // Stroke width: 6 thousandths of the shorter side.
    expect(rect.getAttribute("stroke-width")).toBe("3");
    const circle = container.querySelector("circle")!;
    expect([circle.getAttribute("cx"), circle.getAttribute("cy"), circle.getAttribute("r")]).toEqual(["750", "250", "22.5"]);
    expect(container.querySelector("text")!.textContent).toBe("1");
  });

  it("keeps shapes on the photo", () => {
    const { container } = render(
      <svg>{markupElements([{ kind: "marker", at: { x: 7, y: Number.NaN }, number: 2, color: "#ff0000", width: 6 }], 100, 100)}</svg>,
    );
    const c = container.querySelector("circle")!;
    expect([c.getAttribute("cx"), c.getAttribute("cy")]).toEqual(["100", "0"]);
  });
});

it("groups fields by the schema and keeps values it doesn't know", () => {
  const groups = groupFields(SCHEMA, ["towerNumber", "inspectorNote"]);
  expect(groups.map(([g]) => g)).toEqual(["Location & Temporal Data", "Environmental & Load Data", "Fault Analysis", "Other"]);
  expect(groups.at(-1)![1]).toEqual([{ key: "inspectorNote", label: "inspectorNote" }]);
});
