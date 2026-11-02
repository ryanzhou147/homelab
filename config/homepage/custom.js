// Draws small graphs of the last 6 hours in the top bar, so every metric fits on one screen
// without scrolling. The data comes from Prometheus through Grafana's data source proxy, which
// Grafana's anonymous viewer may use; the lab login guards /grafana. Homepage can draw the bar
// again, so the graphs are added back when that happens.
(() => {
  const API = "/grafana/api/datasources/proxy/uid/prometheus/api/v1/query_range";
  const HOURS = 6;
  const STEP_S = 300;
  const REFRESH_MS = 60_000;
  const speed = (v) => (v >= 1e6 ? `${(v / 1e6).toFixed(1)} MB/s` : `${Math.round(v / 1e3)} kB/s`);
  const percent = (v) => `${Math.round(v)}%`;
  const GRAPHS = [
    { label: "Down", query: "sum(rate(homelab_network_receive_bytes_total[2m]))", format: speed },
    { label: "Up", query: "sum(rate(homelab_network_transmit_bytes_total[2m]))", format: speed },
    { label: "Services", query: "100 * avg(homelab_service_up)", format: percent, min: 0, max: 100 },
    { label: "Songs", query: 'homelab_spotdl_songs{state="done"}', format: (v) => `${Math.round(v)}` },
    { label: "CPU", query: "100 * rate(homelab_cpu_busy_seconds_total[2m]) / rate(homelab_cpu_seconds_total[2m])",
      format: percent, min: 0, max: 100 },
  ];
  const latest = new Map(); // label -> numbers, oldest first

  async function load(graph) {
    const end = Math.floor(Date.now() / 1000);
    const params = new URLSearchParams({
      query: graph.query, start: end - HOURS * 3600, end, step: STEP_S,
    });
    try {
      const reply = await fetch(`${API}?${params}`, { credentials: "same-origin" });
      const series = (await reply.json()).data.result[0];
      latest.set(graph.label, series ? series.values.map(([, v]) => Number(v)) : []);
    } catch {
      latest.set(graph.label, []);
    }
  }

  // An SVG path for the values, scaled into a 100 x 24 box; "area" closes it along the bottom.
  function path(values, graph, area) {
    const lo = graph.min ?? Math.min(...values);
    const hi = Math.max(graph.max ?? -Infinity, ...values);
    const span = hi - lo || 1;
    const points = values.map((v, i) =>
      `${((i / Math.max(values.length - 1, 1)) * 100).toFixed(1)},${(23 - ((v - lo) / span) * 22).toFixed(1)}`);
    return area ? `M0,24 L${points.join(" L")} L100,24 Z` : `M${points.join(" L")}`;
  }

  function render(box) {
    box.replaceChildren(...GRAPHS.map((graph) => {
      const values = latest.get(graph.label) || [];
      const tile = document.createElement("div");
      tile.className = "lab-graph";
      const text = document.createElement("div");
      const name = document.createElement("span");
      name.textContent = graph.label;
      const value = document.createElement("b");
      value.textContent = values.length ? graph.format(values[values.length - 1]) : "–";
      text.append(name, value);
      tile.append(text);
      if (values.length > 1) {
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("viewBox", "0 0 100 24");
        svg.setAttribute("preserveAspectRatio", "none");
        for (const area of [true, false]) {
          const line = document.createElementNS("http://www.w3.org/2000/svg", "path");
          line.setAttribute("d", path(values, graph, area));
          line.setAttribute("class", area ? "area" : "line");
          svg.append(line);
        }
        tile.append(svg);
      }
      return tile;
    }));
  }

  function attach() {
    const bar = document.querySelector("#information-widgets #widgets-wrap");
    if (!bar || bar.querySelector(".lab-graphs")) return;
    const box = document.createElement("div");
    box.className = "lab-graphs";
    bar.insertBefore(box, bar.lastElementChild); // before the clock
    render(box);
  }

  async function refresh() {
    await Promise.all(GRAPHS.map(load));
    const box = document.querySelector(".lab-graphs");
    if (box) render(box);
  }

  new MutationObserver(attach).observe(document.body, { childList: true, subtree: true });
  attach();
  refresh();
  setInterval(refresh, REFRESH_MS);
})();
