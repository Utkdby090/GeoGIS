// The 3D tower: one static model for every tower (tower.glb), turned with the mouse or a finger,
// with a marker at each part that has findings, coloured by its worst confirmed finding, and a
// numbered badge over it matching the findings list. Clicking a marker or badge picks that part
// and the camera moves to it. Drawn only when something changes or moves (turning, moving to a
// part), paused while scrolled out of sight, and everything it made (the GPU context included) is
// released when it goes, so opening tower after tower doesn't leak.

import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";

import type { PartDef, PartHealth, Severity } from "../api/schemas";
import { nodeName, type Spot, spots, type Vec3 } from "./spots";
import modelUrl from "./tower.glb?url";

export interface Badge {
  /** What the badge shows: the numbers of the part's findings in the list. */
  text: string;
  /** What a screen reader says for it. */
  label: string;
  colour: string;
  ink: string;
}

export interface TowerModelProps {
  parts: readonly PartHealth[];
  defs: readonly PartDef[];
  /** The picked part's key (a part id, or "" for the whole tower), or null. */
  selected: string | null;
  onPick: (key: string) => void;
  /** Colours per severity, from the page's CSS. */
  colours: Record<Severity, string>;
  /** The colour of a part with only findings waiting for review, and of the "all parts" ghosts. */
  quiet: string;
  /** A badge per part key; parts without one show only their marker. */
  badges?: ReadonlyMap<string, Badge>;
  /** Show a faint marker at every part of the schema, so it's clear where findings can go. */
  allParts?: boolean;
  /** Turn the tower slowly; stops (and onTurnStop is called) when the person grabs it. */
  turn?: boolean;
  onTurnStop?: () => void;
  /** Changing this number moves the camera back to the first view. */
  home?: number;
  /** Shown instead of the model when it can't be drawn (no WebGL). */
  onFail?: (why: string) => void;
}

const TARGET = new THREE.Vector3(0, 34, 0);
/** Half the tower's height and width (metres) the first view must show, with a margin. */
const HALF_H = 40;
const HALF_W = 20;
/** The first view: from the front of the line, a little to the side and above. */
const VIEW = new THREE.Vector3(0.35, 0.2, 1).normalize();
const CENTRE: Vec3 = [0, 30, 0];
const ABOVE: Vec3 = [0, 70, 0];
/** How far (metres) the camera stays from a picked part. */
const CLOSE = 60;
/** How far the badge sits above its marker on screen (pixels). */
const BADGE_LIFT = 20;

interface Live {
  camera: THREE.PerspectiveCamera;
  controls: OrbitControls;
  nodes: Map<string, Vec3>;
  markers: THREE.Group;
  /** Shared by every marker; released with the scene. */
  sphere: THREE.SphereGeometry;
  ring: THREE.TorusGeometry;
  ghost: THREE.SphereGeometry;
  /** Where each part's marker is, from the last time markers were drawn. */
  spots: Spot[];
  render: () => void;
  /** Move the camera smoothly to look at `at` from CLOSE, or back to the first view. */
  goTo: (at: Vec3 | null) => void;
  reduced: boolean;
}

export default function TowerModel({
  parts,
  defs,
  selected,
  onPick,
  colours,
  quiet,
  badges,
  allParts = false,
  turn = false,
  onTurnStop,
  home = 0,
  onFail,
}: TowerModelProps) {
  const host = useRef<HTMLDivElement>(null);
  const live = useRef<Live | null>(null);
  const [ready, setReady] = useState(false);
  const pick = useRef(onPick);
  pick.current = onPick;
  const fail = useRef(onFail);
  fail.current = onFail;
  const stopped = useRef(onTurnStop);
  stopped.current = onTurnStop;
  /** The badge buttons by part key, positioned by the render loop. */
  const badgeEls = useRef(new Map<string, HTMLButtonElement>());

  // The scene, made once per mount.
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    } catch (e) {
      fail.current?.(e instanceof Error ? e.message : String(e));
      return;
    }
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    el.prepend(renderer.domElement);
    const scene = new THREE.Scene();
    scene.add(new THREE.HemisphereLight(0xffffff, 0x445566, 2.2));
    const camera = new THREE.PerspectiveCamera(35, 1, 1, 1000);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.copy(TARGET);
    controls.minDistance = 15;
    controls.maxDistance = 400;
    controls.maxPolarAngle = Math.PI * 0.55;
    controls.enablePan = false;
    controls.autoRotateSpeed = 1;
    const ground = new THREE.GridHelper(40, 8, 0x888888, 0x888888);
    (ground.material as THREE.Material).opacity = 0.25;
    (ground.material as THREE.Material).transparent = true;
    scene.add(ground);
    const markers = new THREE.Group();
    scene.add(markers);
    const sphere = new THREE.SphereGeometry(1.3, 20, 14);
    const ring = new THREE.TorusGeometry(2.1, 0.25, 8, 32);
    const ghost = new THREE.SphereGeometry(0.8, 10, 8);

    let width = 300;
    let height = 300;
    const homeDistance = () => {
      // Far enough back that the whole tower fits, whatever the panel's shape.
      const t = Math.tan(THREE.MathUtils.degToRad(camera.fov / 2));
      return Math.max(HALF_H / t, HALF_W / (t * camera.aspect));
    };

    // Reused every frame, so drawing allocates nothing.
    const at = new THREE.Vector3();
    const dir = new THREE.Vector3();
    const placeBadges = () => {
      for (const m of markers.children) {
        const key = m.userData.key;
        const b = typeof key === "string" ? badgeEls.current.get(key) : undefined;
        if (!b) continue;
        at.copy(m.position).project(camera);
        const off = at.z > 1 || at.x < -1.1 || at.x > 1.1 || at.y < -1.1 || at.y > 1.1;
        b.style.visibility = off ? "hidden" : "visible";
        if (!off) b.style.transform = `translate(${(((at.x + 1) / 2) * width).toFixed(1)}px, ${(((1 - at.y) / 2) * height - BADGE_LIFT).toFixed(1)}px)`;
      }
    };

    // One loop: it runs while the camera is moving to a part or the tower turns, and draws a
    // single frame for anything else (a drag, a resize, new markers).
    let goal: { target: THREE.Vector3; dist: number } | null = null;
    let visible = true;
    let frame = 0;
    let stepping = false;
    const step = () => {
      frame = 0;
      if (!visible) return;
      stepping = true;
      let again = false;
      if (goal) {
        const k = reduced ? 1 : 0.14;
        controls.target.lerp(goal.target, k);
        dir.copy(camera.position).sub(controls.target);
        const d = dir.length();
        const next = d + (goal.dist - d) * k;
        camera.position.copy(controls.target).addScaledVector(dir.normalize(), next);
        if (controls.target.distanceTo(goal.target) < 0.05 && Math.abs(next - goal.dist) < 0.05) goal = null;
        else again = true;
      }
      if (controls.autoRotate) again = true;
      controls.update();
      renderer.render(scene, camera);
      placeBadges();
      stepping = false;
      if (again) frame = requestAnimationFrame(step);
    };
    const render = () => {
      if (frame || stepping) return;
      frame = requestAnimationFrame(step);
    };
    const goTo = (p: Vec3 | null) => {
      goal = p ? { target: new THREE.Vector3(...p), dist: CLOSE } : { target: TARGET.clone(), dist: homeDistance() };
      render();
    };

    // Once the person turns the tower, a resize keeps their view, and turning stops.
    let moved = false;
    controls.addEventListener("start", () => {
      moved = true;
      goal = null;
      if (controls.autoRotate) {
        controls.autoRotate = false;
        stopped.current?.();
      }
    });
    controls.addEventListener("change", render);
    const size = () => {
      width = el.clientWidth || 300;
      height = el.clientHeight || 300;
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      if (!moved && !goal) {
        camera.position.copy(TARGET).addScaledVector(VIEW, homeDistance());
        controls.update();
      }
      render();
    };
    size();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(size);
    ro?.observe(el);
    // Nothing is drawn while the panel is scrolled away; turning resumes when it's back.
    const io =
      typeof IntersectionObserver === "undefined"
        ? null
        : new IntersectionObserver((entries) => {
            visible = entries.some((e) => e.isIntersecting);
            if (visible) render();
          });
    io?.observe(el);

    // A click (not a drag) on a marker picks its part.
    let down: { x: number; y: number } | null = null;
    const ray = new THREE.Raycaster();
    const onDown = (e: PointerEvent) => {
      down = { x: e.clientX, y: e.clientY };
    };
    const onUp = (e: PointerEvent) => {
      if (!down || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
      down = null;
      const r = renderer.domElement.getBoundingClientRect();
      ray.setFromCamera(new THREE.Vector2(((e.clientX - r.left) / r.width) * 2 - 1, -((e.clientY - r.top) / r.height) * 2 + 1), camera);
      const hit = ray.intersectObjects(markers.children, false).find((h) => typeof h.object.userData.key === "string");
      const key = hit?.object.userData.key;
      if (typeof key === "string") pick.current(key);
    };
    renderer.domElement.addEventListener("pointerdown", onDown);
    renderer.domElement.addEventListener("pointerup", onUp);

    const nodes = new Map<string, Vec3>();
    live.current = { camera, controls, nodes, markers, sphere, ring, ghost, spots: [], render, goTo, reduced };
    let gone = false;
    new GLTFLoader()
      .loadAsync(modelUrl)
      .then((gltf) => {
        if (gone) {
          dispose(gltf.scene);
          return;
        }
        gltf.scene.updateMatrixWorld(true);
        // The loader strips dots from object names ("insulator.left.r" becomes
        // "insulatorleftr"), so each node's name is read from the file itself.
        const json = gltf.parser.json as { nodes?: { name?: string }[] };
        gltf.scene.traverse((o) => {
          const index = gltf.parser.associations.get(o)?.nodes;
          const name = index === undefined ? undefined : json.nodes?.[index]?.name;
          if (name && !(o as THREE.Mesh).isMesh) {
            const p = o.getWorldPosition(new THREE.Vector3());
            nodes.set(name, [p.x, p.y, p.z]);
          }
        });
        scene.add(gltf.scene);
        setReady(true);
        render();
      })
      .catch((e: unknown) => {
        if (!gone) fail.current?.(`The tower model didn't load: ${e instanceof Error ? e.message : String(e)}`);
      });

    return () => {
      gone = true;
      if (frame) cancelAnimationFrame(frame);
      ro?.disconnect();
      io?.disconnect();
      renderer.domElement.removeEventListener("pointerdown", onDown);
      renderer.domElement.removeEventListener("pointerup", onUp);
      controls.dispose();
      dispose(scene);
      sphere.dispose();
      ring.dispose();
      ghost.dispose();
      renderer.dispose();
      renderer.forceContextLoss();
      renderer.domElement.remove();
      live.current = null;
    };
  }, []);

  // The markers, redrawn when the parts, the pick or the colours change.
  useEffect(() => {
    const l = live.current;
    if (!l || !ready) return;
    // Each marker's own material goes; the shared shapes stay for the next markers.
    for (const m of l.markers.children) ((m as THREE.Mesh).material as THREE.Material).dispose();
    l.markers.clear();
    l.spots = spots(parts, defs, l.nodes, CENTRE, ABOVE);
    for (const s of l.spots) {
      // A part with only proposals waiting has no rating yet: it shows as a hollow ring.
      const mesh =
        s.counted > 0
          ? new THREE.Mesh(l.sphere, new THREE.MeshBasicMaterial({ color: colours[s.severity], depthTest: false }))
          : new THREE.Mesh(l.ring, new THREE.MeshBasicMaterial({ color: quiet, depthTest: false }));
      mesh.position.set(...s.at);
      mesh.userData.key = s.key;
      mesh.renderOrder = 1;
      if (s.key === selected) mesh.scale.setScalar(1.7);
      if (s.counted === 0) mesh.lookAt(l.camera.position);
      l.markers.add(mesh);
    }
    if (allParts) {
      const used = new Set(parts.map((p) => p.part));
      for (const d of defs) {
        const p = used.has(d.id) ? undefined : l.nodes.get(nodeName(d.id, defs));
        if (!p) continue;
        const mesh = new THREE.Mesh(l.ghost, new THREE.MeshBasicMaterial({ color: quiet, wireframe: true }));
        mesh.position.set(...p);
        l.markers.add(mesh);
      }
    }
    l.render();
  }, [parts, defs, selected, colours, quiet, allParts, badges, ready]);

  // The camera moves to a picked part, and back to the whole tower when the pick is cleared.
  const lastPick = useRef<string | null>(null);
  useEffect(() => {
    const l = live.current;
    if (!l || !ready || selected === lastPick.current) return;
    lastPick.current = selected;
    const spot = selected === null ? undefined : l.spots.find((s) => s.key === selected);
    if (selected === null) l.goTo(null);
    else if (spot) l.goTo(spot.at);
  }, [selected, ready]);

  const lastHome = useRef(home);
  useEffect(() => {
    if (home === lastHome.current) return;
    lastHome.current = home;
    live.current?.goTo(null);
  }, [home]);

  useEffect(() => {
    const l = live.current;
    if (!l || !ready) return;
    l.controls.autoRotate = turn && !l.reduced;
    l.render();
  }, [turn, ready]);

  return (
    <div ref={host} className="tower-3d">
      {badges && ready ? (
        <div className="badges">
          {[...badges].map(([key, b]) => (
            <button
              key={key}
              ref={(e) => {
                if (e) badgeEls.current.set(key, e);
                else badgeEls.current.delete(key);
              }}
              type="button"
              className="badge"
              aria-label={b.label}
              aria-pressed={selected === key}
              style={{ background: b.colour, color: b.ink }}
              onClick={() => onPick(key)}
            >
              {b.text}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** Releases every geometry and material under `root`, each once. */
function dispose(root: THREE.Object3D) {
  const seen = new Set<object>();
  root.traverse((o) => {
    const m = o as THREE.Mesh;
    if (m.geometry && !seen.has(m.geometry)) {
      seen.add(m.geometry);
      m.geometry.dispose();
    }
    const mats = Array.isArray(m.material) ? m.material : m.material ? [m.material] : [];
    for (const mat of mats) {
      if (!seen.has(mat)) {
        seen.add(mat);
        mat.dispose();
      }
    }
  });
}
