// The 3D tower: one static model for every tower (tower.glb), turned with the mouse or a finger,
// with a marker at each part that has findings, coloured by its worst confirmed finding. Clicking
// a marker picks that part. Drawn only when something changes, and everything it made (the GPU
// context included) is released when it goes, so opening tower after tower doesn't leak.

import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";

import type { PartDef, PartHealth, Severity } from "../api/schemas";
import { spots, type Vec3 } from "./spots";
import modelUrl from "./tower.glb?url";

export interface TowerModelProps {
  parts: readonly PartHealth[];
  defs: readonly PartDef[];
  /** The picked part's key (a part id, or "" for the whole tower), or null. */
  selected: string | null;
  onPick: (key: string) => void;
  /** Colours per severity, from the page's CSS. */
  colours: Record<Severity, string>;
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

interface Live {
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  controls: OrbitControls;
  nodes: Map<string, Vec3>;
  markers: THREE.Group;
  /** Shared by every marker; released with the scene. */
  sphere: THREE.SphereGeometry;
  ring: THREE.TorusGeometry;
  render: () => void;
}

export default function TowerModel({ parts, defs, selected, onPick, colours, onFail }: TowerModelProps) {
  const host = useRef<HTMLDivElement>(null);
  const live = useRef<Live | null>(null);
  const [ready, setReady] = useState(false);
  const pick = useRef(onPick);
  pick.current = onPick;
  const fail = useRef(onFail);
  fail.current = onFail;

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
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    el.appendChild(renderer.domElement);
    const scene = new THREE.Scene();
    scene.add(new THREE.HemisphereLight(0xffffff, 0x445566, 2.2));
    const camera = new THREE.PerspectiveCamera(35, 1, 1, 1000);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.copy(TARGET);
    controls.minDistance = 30;
    controls.maxDistance = 400;
    controls.maxPolarAngle = Math.PI * 0.55;
    controls.enablePan = false;
    const ground = new THREE.GridHelper(40, 8, 0x888888, 0x888888);
    (ground.material as THREE.Material).opacity = 0.25;
    (ground.material as THREE.Material).transparent = true;
    scene.add(ground);
    const markers = new THREE.Group();
    scene.add(markers);
    const sphere = new THREE.SphereGeometry(1.3, 20, 14);
    const ring = new THREE.TorusGeometry(2.1, 0.25, 8, 32);

    let frame = 0;
    const render = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        renderer.render(scene, camera);
      });
    };
    // Once the person turns the tower, a resize keeps their view.
    let moved = false;
    controls.addEventListener("start", () => {
      moved = true;
    });
    controls.addEventListener("change", render);
    const size = () => {
      const w = el.clientWidth || 300;
      const h = el.clientHeight || 300;
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      if (!moved) {
        // Far enough back that the whole tower fits, whatever the panel's shape.
        const t = Math.tan(THREE.MathUtils.degToRad(camera.fov / 2));
        const distance = Math.max(HALF_H / t, HALF_W / (t * camera.aspect));
        camera.position.copy(TARGET).addScaledVector(VIEW, distance);
        controls.update();
      }
      render();
    };
    size();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(size);
    ro?.observe(el);

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
      const hit = ray.intersectObjects(markers.children, false)[0];
      const key = hit?.object.userData.key;
      if (typeof key === "string") pick.current(key);
    };
    renderer.domElement.addEventListener("pointerdown", onDown);
    renderer.domElement.addEventListener("pointerup", onUp);

    const nodes = new Map<string, Vec3>();
    live.current = { renderer, scene, camera, controls, nodes, markers, sphere, ring, render };
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
      renderer.domElement.removeEventListener("pointerdown", onDown);
      renderer.domElement.removeEventListener("pointerup", onUp);
      controls.dispose();
      dispose(scene);
      sphere.dispose();
      ring.dispose();
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
    for (const s of spots(parts, defs, l.nodes, CENTRE, ABOVE)) {
      // A part with only proposals waiting has no rating yet: it shows as a hollow ring.
      const colour = colours[s.severity];
      const mesh =
        s.counted > 0
          ? new THREE.Mesh(l.sphere, new THREE.MeshBasicMaterial({ color: colour, depthTest: false }))
          : new THREE.Mesh(l.ring, new THREE.MeshBasicMaterial({ color: colours.none, depthTest: false }));
      mesh.position.set(...s.at);
      mesh.userData.key = s.key;
      if (s.key === selected) mesh.scale.setScalar(1.7);
      if (s.counted === 0) mesh.lookAt(l.camera.position);
      l.markers.add(mesh);
    }
    l.render();
  }, [parts, defs, selected, colours, ready]);

  return <div ref={host} className="tower-3d" />;
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
