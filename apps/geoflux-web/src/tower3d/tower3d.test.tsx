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
  default: (props: {
    parts: S.PartHealth[];
    onPick: (key: string) => void;
    selected: string | null;
    badges?: ReadonlyMap<string, { text: string; label: string }>;
    allParts?: boolean;
    turn?: boolean;
    home?: number;
  }) => (
    <div
      data-testid="tower-3d"
      data-selected={props.selected ?? "none"}
      data-all={String(props.allParts)}
      data-turn={String(props.turn)}
      data-home={String(props.home)}
    >
      {props.parts.map((p) => (
        <button key={p.part ?? ""} type="button" onClick={() => props.onPick(p.part ?? "")}>
          marker {p.part ?? "whole tower"}
        </button>
      ))}
      {[...(props.badges ?? [])].map(([key, b]) => (
        <button key={`b${key}`} type="button" aria-label={b.label} onClick={() => props.onPick(key)}>
          {b.text}
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
  it("numbers the confirmed findings, badges their parts, and lists what waits for review", async () => {
    renderApp(api(), `/surveys/${ID}?tower=t1`);
    const section = await screen.findByRole("region", { name: "Tower health" });
    expect(await within(section).findByText("Confirmed findings on 101 (2)")).toBeTruthy();
    const confirmed = within(section).getByRole("list", { name: "Confirmed findings" });
    const items = () => within(confirmed).getAllByRole("button");
    // Labels come from the schema, which may arrive just after the findings.
    await waitFor(() => expect(items()[0]!.textContent).toBe("1Hot joint or connectorY phase insulator, left circuitHottest spot 78.2 °C · Hottest rise 41.7 °CCritical"));
    expect(items()[1]!.textContent).toMatch(/^2.*Whole tower.*Neem tree 3 m from B phase.*Medium$/);
    const waiting = within(section).getByRole("list", { name: "Findings waiting for review" });
    expect(within(waiting).getByRole("button").textContent).toMatch(/Flashover marks on insulator.*Waiting for review/);

    // One badge per part with confirmed findings, numbered like the list; none for the proposal.
    const model = await screen.findByTestId("tower-3d");
    const badge = within(model).getByRole("button", { name: "Finding 1: Y phase insulator, left circuit, critical" });
    expect(badge.textContent).toBe("1");
    expect(within(model).getByRole("button", { name: "Finding 2: Whole tower, medium" }).textContent).toBe("2");
    expect(within(model).queryByRole("button", { name: /R phase insulator/ })).toBeNull();

    // A badge, a marker or a list item picks the part; picking it again lets go.
    fireEvent.click(badge);
    expect(model.getAttribute("data-selected")).toBe("insulator.left.y");
    expect(items()[0]!.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(items()[1]!);
    expect(model.getAttribute("data-selected")).toBe("");
    fireEvent.click(within(model).getByRole("button", { name: "marker whole tower" }));
    expect(model.getAttribute("data-selected")).toBe("none");

    // The view's tools.
    fireEvent.click(items()[0]!);
    fireEvent.click(within(section).getByRole("button", { name: "Reset view" }));
    expect(model.getAttribute("data-selected")).toBe("none");
    expect(model.getAttribute("data-home")).toBe("1");
    expect(model.getAttribute("data-turn")).toBe("false"); // the survey page starts still
    fireEvent.click(within(section).getByRole("checkbox", { name: "Turn" }));
    expect(model.getAttribute("data-turn")).toBe("true");
    fireEvent.click(within(section).getByRole("checkbox", { name: "All parts" }));
    expect(model.getAttribute("data-all")).toBe("true");

    fireEvent.click(within(section).getByRole("button", { name: "Hide 3D" }));
    expect(screen.queryByTestId("tower-3d")).toBeNull();
  });

  it("still shows the tower, unmarked, when it has no synced findings", async () => {
    renderApp(api({ ...HEALTH, parts: [], findings: [] }), `/surveys/${ID}?tower=t1`);
    expect(await screen.findByText(/^No findings yet\./)).toBeTruthy();
    expect(await screen.findByTestId("tower-3d")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Confirmed findings" })).toBeNull();
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
