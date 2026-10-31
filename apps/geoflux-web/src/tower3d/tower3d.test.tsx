// The tower's health in the tower panel, with the 3D view swapped for a list of its markers
// (jsdom has no WebGL), where markers go, and the model file itself.

import { readFileSync } from "node:fs";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import * as S from "../api/schemas";
import { fakeApi, ME } from "../test/fake-api";
import { renderApp } from "../test/render";
import { nodeName, spots, WHOLE_TOWER, type Vec3 } from "./spots";

import schemaFixture from "../api/fixtures/schema.json";
import surveyFixture from "../api/fixtures/survey.json";
import towerFixture from "../api/fixtures/tower.json";
import towerHealthFixture from "../api/fixtures/tower-health.json";
import towersFixture from "../api/fixtures/towers.json";

vi.mock("./TowerModel", () => ({
  default: ({ parts, onPick, selected }: { parts: S.PartHealth[]; onPick: (key: string) => void; selected: string | null }) => (
    <div data-testid="tower-3d" data-selected={selected ?? ""}>
      {parts.map((p) => (
        <button key={p.part ?? ""} type="button" onClick={() => onPick(p.part ?? "")}>
          marker {p.part ?? "whole tower"}
        </button>
      ))}
    </div>
  ),
}));

vi.mock("../survey/LineMap", () => ({ default: () => <div data-testid="map" /> }));

afterEach(cleanup);

const SURVEY = S.SurveyDetail.parse(surveyFixture);
const SCHEMA = S.DomainSchema.parse(schemaFixture);
const TOWER = S.TowerDetail.parse(towerFixture);
const TOWERS = S.TowerCollection.parse(towersFixture);
const HEALTH = S.TowerHealth.parse(towerHealthFixture);
const ID = SURVEY.summary.id;

function api(health: S.TowerHealth | null = HEALTH) {
  return fakeApi({
    me: vi.fn(() => Promise.resolve(ME)),
    survey: vi.fn(() => Promise.resolve(SURVEY)),
    towers: vi.fn(() => Promise.resolve(TOWERS)),
    tower: vi.fn(() => Promise.resolve(TOWER)),
    schema: vi.fn(() => Promise.resolve(SCHEMA)),
    towerHealth: vi.fn(() => Promise.resolve(health)),
  });
}

describe("a tower's health", () => {
  it("lists the parts with findings and shows the picked part's findings", async () => {
    renderApp(api(), `/surveys/${ID}?tower=t1`);
    const section = await screen.findByRole("region", { name: "Tower health" });
    const parts = within(section).getByRole("list", { name: "Parts with findings" });
    await waitFor(() =>
      expect(
        within(parts)
          .getAllByRole("button")
          .map((b) => b.textContent),
      ).toEqual([
        "Y phase insulator, left circuitCritical1 confirmed",
        "Whole towerMedium1 confirmed",
        "R phase insulator, right circuit1 waiting",
      ]),
    );
    const findings = () =>
      within(section)
        .getAllByRole("listitem")
        .filter((li) => li.closest(".finding-list"));
    expect(findings()).toHaveLength(3);
    expect(findings()[2]!.textContent).toContain("Waiting for review");

    // A marker on the 3D tower picks its part; picking it again shows everything.
    fireEvent.click(await within(section).findByRole("button", { name: "marker insulator.left.y" }));
    expect(screen.getByTestId("tower-3d").getAttribute("data-selected")).toBe("insulator.left.y");
    expect(findings().map((li) => li.textContent)).toEqual([expect.stringContaining("Hot joint or connector")]);
    expect(findings()[0]!.textContent).toContain("Hottest rise 41.7 °C");
    fireEvent.click(within(section).getByRole("button", { name: "marker whole tower" }));
    expect(findings().map((li) => li.textContent)).toEqual([expect.stringContaining("Neem tree 3 m from B phase")]);
    fireEvent.click(within(section).getByRole("button", { name: "Show all" }));
    expect(findings()).toHaveLength(3);

    fireEvent.click(within(section).getByRole("button", { name: "Hide 3D" }));
    expect(screen.queryByTestId("tower-3d")).toBeNull();
  });

  it("says so when the tower has no synced findings", async () => {
    renderApp(api({ ...HEALTH, parts: [], findings: [] }), `/surveys/${ID}?tower=t1`);
    expect(await screen.findByText("No findings were synced for this tower.")).toBeTruthy();
    expect(screen.queryByTestId("tower-3d")).toBeNull();
  });
});

describe("markers on the 3D tower", () => {
  const defs: S.PartDef[] = [
    { id: "leg.a", label: "Leg A" },
    { id: "body", label: "Body", node: "Body_Anchor" },
  ];
  const nodes = new Map<string, Vec3>([
    ["leg.a", [-7, 1, -7]],
    ["Body_Anchor", [0, 22, 0]],
  ]);
  const centre: Vec3 = [0, 30, 0];
  const above: Vec3 = [0, 70, 0];
  const p = (part: string | null, severity: S.Severity = "low"): S.PartHealth => ({ part, severity, counted: 1, pending: 0 });

  it("sit at the part's node, its own node name, the centre when missing, or above for the whole tower", () => {
    expect(nodeName("body", defs)).toBe("Body_Anchor");
    expect(nodeName("leg.b", defs)).toBe("leg.b");
    const s = spots([p("leg.a"), p("body"), p("insulator.left.r"), p(null)], defs, nodes, centre, above);
    expect(s.map((x) => [x.key, x.at, x.placed])).toEqual([
      ["leg.a", [-7, 1, -7], true],
      ["body", [0, 22, 0], true],
      ["insulator.left.r", centre, false],
      [WHOLE_TOWER, above, true],
    ]);
  });

  it("has a node in the model for every part of the built-in schema", () => {
    const glb = readFileSync("src/tower3d/tower.glb"); // tests run from the app folder;
    expect(glb.readUInt32LE(0)).toBe(0x46546c67); // "glTF"
    const json = JSON.parse(glb.subarray(20, 20 + glb.readUInt32LE(12)).toString("utf8")) as { nodes: { name?: string }[] };
    const names = new Set(json.nodes.map((n) => n.name));
    const wanted = (SCHEMA.parts ?? []).map((d) => nodeName(d.id, SCHEMA.parts ?? []));
    expect(wanted).toHaveLength(14);
    expect(wanted.filter((n) => !names.has(n))).toEqual([]);
  });
});
