// A photo's markup drawn over it as SVG, the same way the PDF report draws it (geoflux-report's
// pdf.rs), so the web, the desktop and the report agree. Positions are fractions of the photo;
// widths and sizes thousandths of its shorter side. The photo itself is never changed.

import type { ReactElement } from "react";

import type { Annotation } from "../api/schemas";

/** The SVG elements for `shapes` on a photo `w` × `h` pixels (its natural size). */
export function markupElements(shapes: readonly Annotation[], w: number, h: number): ReactElement[] {
  const short = Math.min(w, h);
  const px = (x: number) => clamp01(x) * w;
  const py = (y: number) => clamp01(y) * h;
  return shapes.map((a, i) => {
    const color = a.color;
    const lw = Math.max((a.width / 1000) * short, 0.3);
    const stroke = { stroke: color, strokeWidth: lw, fill: "none", strokeLinecap: "round", strokeLinejoin: "round" } as const;
    switch (a.kind) {
      case "pen":
        return <polyline key={i} points={a.points.map((p) => `${px(p.x)},${py(p.y)}`).join(" ")} {...stroke} />;
      case "arrow": {
        const [x1, y1, x2, y2] = [px(a.from.x), py(a.from.y), px(a.to.x), py(a.to.y)];
        const len = Math.max(Math.hypot(x2 - x1, y2 - y1), 0.001);
        const [ux, uy] = [(x2 - x1) / len, (y2 - y1) / len];
        const head = Math.min(lw * 4.5, len * 0.6);
        const [bx, by] = [x2 - ux * head, y2 - uy * head];
        const [nx, ny] = [-uy * head * 0.5, ux * head * 0.5];
        return (
          <g key={i}>
            <line x1={x1} y1={y1} x2={bx} y2={by} {...stroke} />
            <polygon points={`${x2},${y2} ${bx + nx},${by + ny} ${bx - nx},${by - ny}`} fill={color} />
          </g>
        );
      }
      case "rect": {
        const [x, y] = [Math.min(px(a.a.x), px(a.b.x)), Math.min(py(a.a.y), py(a.b.y))];
        return <rect key={i} x={x} y={y} width={Math.abs(px(a.b.x) - px(a.a.x))} height={Math.abs(py(a.b.y) - py(a.a.y))} {...stroke} />;
      }
      case "ellipse":
        return (
          <ellipse
            key={i}
            cx={(px(a.a.x) + px(a.b.x)) / 2}
            cy={(py(a.a.y) + py(a.b.y)) / 2}
            rx={Math.abs(px(a.b.x) - px(a.a.x)) / 2}
            ry={Math.abs(py(a.b.y) - py(a.a.y)) / 2}
            {...stroke}
          />
        );
      case "text": {
        const cap = (a.size / 1000) * short;
        return (
          <text key={i} x={px(a.at.x)} y={py(a.at.y) + cap} fontSize={cap / 0.718} fontWeight={700} fill={color}>
            {a.text}
          </text>
        );
      }
      case "marker": {
        const r = Math.max(lw * 2.2, short * 0.045);
        const [cx, cy] = [px(a.at.x), py(a.at.y)];
        return (
          <g key={i}>
            <circle cx={cx} cy={cy} r={r} fill={color} />
            <text x={cx} y={cy} fontSize={r * 1.15} fontWeight={700} fill="#fff" textAnchor="middle" dominantBaseline="central">
              {a.number}
            </text>
          </g>
        );
      }
    }
  });
}

function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}
