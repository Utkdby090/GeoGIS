// One ECharts chart in a panel: drawn on a canvas (a line of thousands of towers stays fast),
// resized with its panel, and disposed when the panel goes. Loaded only with the dashboard, and only the chart
// types the dashboard uses are bundled.

import { BarChart, ScatterChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import * as echarts from "echarts/core";
import type { EChartsCoreOption } from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import { useEffect, useRef } from "react";

echarts.use([BarChart, ScatterChart, GridComponent, TooltipComponent, CanvasRenderer]);

export interface EChartProps {
  option: EChartsCoreOption;
  /** What the chart shows, for a screen reader; the panel's table says it in full. */
  label: string;
  /** A click on a mark: its series and the index of its data item. */
  onPick?: (seriesIndex: number, dataIndex: number) => void;
}

export default function EChart({ option, label, onPick }: EChartProps) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<echarts.ECharts | null>(null);
  const pick = useRef(onPick);
  pick.current = onPick;

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const c = echarts.init(el, null, { renderer: "canvas" });
    chart.current = c;
    c.on("click", (e) => {
      if (typeof e.dataIndex === "number") pick.current?.(e.seriesIndex ?? 0, e.dataIndex);
    });
    // One resize per frame at most, however often the panel's size changes while dragging.
    let frame = 0;
    const ro = new ResizeObserver(() => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        if (!c.isDisposed()) c.resize();
      });
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (frame) cancelAnimationFrame(frame);
      chart.current = null;
      c.dispose();
    };
  }, []);

  useEffect(() => {
    chart.current?.setOption(option, { notMerge: true, lazyUpdate: true });
  }, [option]);

  return <div ref={host} className="echart" role="img" aria-label={label} />;
}
