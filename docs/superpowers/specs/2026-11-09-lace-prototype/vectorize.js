const VEC = { passes: 2, prune: 3, serifs: false }; // serifs: false → serifs are stripped before tracing
// Bitmap glyph → stroke model (lines + arcs), so the connector machinery runs on a real pixel font.
// 1. tiny components become dots; 2. Zhang–Suen thinning; 3. skeleton graph traced into paths between
// ends and junctions (junction clusters merged, 1-pixel spurs pruned); 4. Ramer–Douglas–Peucker;
// 5. gentle vertices (turn ≤ 60°) become tangent fillet arcs, sharp ones stay corners.
function vectorizeGlyph(rows, W, H, base, k) {
  const P = (c, r) => [(c + 0.5) * k, (base - r - 0.5) * k];
  const inkAt = (c, r) => r >= 0 && r < H && c >= 0 && c < W && (rows[r] & (0x80 >> c)) !== 0;
  const prims = [];
  // Components (8-connected)
  const seen = new Set(), comps = [];
  for (let r = 0; r < H; r++) for (let c = 0; c < W; c++) {
    if (!inkAt(c, r) || seen.has(c + ',' + r)) continue;
    const comp = [], stack = [[c, r]];
    seen.add(c + ',' + r);
    while (stack.length) {
      const [x, y] = stack.pop();
      comp.push([x, y]);
      for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
        const nx = x + dx, ny = y + dy;
        if (inkAt(nx, ny) && !seen.has(nx + ',' + ny)) { seen.add(nx + ',' + ny); stack.push([nx, ny]); }
      }
    }
    comps.push(comp);
  }
  const S = new Set();
  for (const comp of comps) {
    const xs = comp.map((p) => p[0]), ys = comp.map((p) => p[1]);
    if (comp.length <= 4 && Math.max(...xs) - Math.min(...xs) <= 1 && Math.max(...ys) - Math.min(...ys) <= 1) {
      const cx = xs.reduce((a, b) => a + b, 0) / comp.length, cy = ys.reduce((a, b) => a + b, 0) / comp.length;
      prims.push(O(...P(cx, cy), 0.7));
    } else for (const [x, y] of comp) S.add(x + ',' + y);
  }
  // Scale2x (EPX) twice: pixel staircases become diagonals, filled square corners stay square.
  const UP = 2 ** VEC.passes;
  for (let pass = 0; pass < VEC.passes; pass++) {
    const src = new Set(S), on = (x, y) => src.has(x + ',' + y);
    S.clear();
    const xs = [...src].map((q) => +q.split(',')[0]), ys = [...src].map((q) => +q.split(',')[1]);
    for (let y = Math.min(...ys) - 1; y <= Math.max(...ys) + 1; y++) for (let x = Math.min(...xs) - 1; x <= Math.max(...xs) + 1; x++) {
      const Pp = on(x, y), A_ = on(x, y - 1), B_ = on(x + 1, y), C_ = on(x - 1, y), D_ = on(x, y + 1);
      const q = [Pp, Pp, Pp, Pp];
      if (C_ === A_ && C_ !== D_ && A_ !== B_) q[0] = A_;
      if (A_ === B_ && A_ !== C_ && B_ !== D_) q[1] = B_;
      if (D_ === C_ && D_ !== B_ && C_ !== A_) q[2] = C_;
      if (B_ === D_ && B_ !== A_ && D_ !== C_) q[3] = D_;
      [[0, 0], [1, 0], [0, 1], [1, 1]].forEach(([dx, dy], i) => { if (q[i]) S.add((2 * x + dx) + ',' + (2 * y + dy)); });
    }
  }
  const Q = (x, y) => [((x + 0.5) / UP) * k, (base - (y + 0.5) / UP) * k];
  // Zhang–Suen thinning
  const has = (x, y) => S.has(x + ',' + y);
  for (let changed = true; changed;) {
    changed = false;
    for (const step of [0, 1]) {
      const del = [];
      for (const key of S) {
        const [x, y] = key.split(',').map(Number);
        const n = [has(x, y - 1), has(x + 1, y - 1), has(x + 1, y), has(x + 1, y + 1), has(x, y + 1), has(x - 1, y + 1), has(x - 1, y), has(x - 1, y - 1)];
        const B = n.filter(Boolean).length;
        let A_ = 0;
        for (let i = 0; i < 8; i++) if (!n[i] && n[(i + 1) % 8]) A_++;
        const [p2, , p4, , p6, , p8] = n;
        const ok = step === 0 ? !(p2 && p4 && p6) && !(p4 && p6 && p8) : !(p2 && p4 && p8) && !(p2 && p6 && p8);
        if (B >= 2 && B <= 6 && A_ === 1 && ok) del.push(key);
      }
      if (del.length) { changed = true; del.forEach((d) => S.delete(d)); }
    }
  }
  // Skeleton graph: 4-neighbours, plus diagonals not already bridged by a 4-neighbour.
  const nb = (key) => {
    const [x, y] = key.split(',').map(Number), out = [];
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) if (has(x + dx, y + dy)) out.push((x + dx) + ',' + (y + dy));
    for (const [dx, dy] of [[1, 1], [1, -1], [-1, 1], [-1, -1]])
      if (has(x + dx, y + dy) && !has(x + dx, y) && !has(x, y + dy)) out.push((x + dx) + ',' + (y + dy));
    return out;
  };
  const trace = () => {
    const deg = new Map([...S].map((p) => [p, nb(p).length]));
    const key = (p) => deg.get(p) !== 2;
    // Junction clusters share one centre point.
    const centre = new Map();
    for (const p of S) {
      if (deg.get(p) < 3 || centre.has(p)) continue;
      const cl = [p], st = [p];
      centre.set(p, null);
      while (st.length) for (const q of nb(st.pop())) if (deg.get(q) >= 3 && !centre.has(q)) { centre.set(q, null); cl.push(q); st.push(q); }
      const pts = cl.map((q) => q.split(',').map(Number));
      const c = [pts.reduce((a, b) => a + b[0], 0) / pts.length, pts.reduce((a, b) => a + b[1], 0) / pts.length];
      for (const q of cl) centre.set(q, c);
    }
    const used = new Set(), paths = [];
    const edge = (a, b) => (a < b ? a + '|' + b : b + '|' + a);
    const walk = (start, next) => {
      const path = [start, next];
      used.add(edge(start, next));
      let prev = start, cur = next;
      while (!key(cur)) {
        const nx = nb(cur).find((q) => q !== prev && !used.has(edge(cur, q)));
        if (!nx) break;
        used.add(edge(cur, nx)); path.push(nx); prev = cur; cur = nx;
        if (cur === start) break;
      }
      return path;
    };
    for (const p of S) if (key(p)) for (const q of nb(p)) if (!used.has(edge(p, q))) paths.push(walk(p, q));
    for (const p of S) for (const q of nb(p)) if (!used.has(edge(p, q))) paths.push(walk(p, q)); // loops
    return { paths, deg, centre };
  };
  let G = trace();
  // Prune spurs: short branches from an end to a junction (thinning whiskers, serif nubs).
  for (let round = 0; round < 3; round++) {
    let pruned = false;
    for (const path of G.paths) {
      const a = path[0], b = path[path.length - 1];
      const endA = G.deg.get(a) === 1, endB = G.deg.get(b) === 1, junA = G.deg.get(a) >= 3, junB = G.deg.get(b) >= 3;
      if (path.length <= VEC.prune && ((endA && junB) || (endB && junA))) { (endA ? path.slice(0, -1) : path.slice(1)).forEach((q) => S.delete(q)); pruned = true; }
    }
    if (!pruned) break;
    G = trace();
  }
  for (const path of G.paths) {
    let pts = path.map((p) => G.centre.get(p) || p.split(',').map(Number));
    pts = pts.filter((p, i) => i === 0 || p[0] !== pts[i - 1][0] || p[1] !== pts[i - 1][1]);
    if (pts.length < 2) continue;
    const closed = path.length > 2 && path[0] === path[path.length - 1];
    let poly = rdp(pts.map((p) => Q(p[0], p[1])), 0.3 * k);
    if (closed && poly.length < 4) poly = pts.filter((_, i) => i % 4 === 0).map((p) => Q(p[0], p[1]));
    prims.push(...polyToPrims(poly, closed));
  }
  // Thinning reads a one-pixel bar sticking out of a stem as a bump and erases it (the VGA f).
  // Put back every 1-pixel-high run of 3+ pixels that crosses at most a 2-pixel stem, unless traced already.
  for (let r = 0; r < H; r++) {
    for (let c = 0; c < W;) {
      if (!inkAt(c, r)) { c++; continue; }
      let e = c;
      while (e + 1 < W && inkAt(e + 1, r)) e++;
      const span = [], len_ = e - c + 1;
      for (let x = c; x <= e; x++) span.push(x);
      const above = span.filter((x) => inkAt(x, r - 1)).length, below = span.filter((x) => inkAt(x, r + 1)).length;
      if (len_ >= 3 && above <= 2 && below <= 2) {
        const [x0, y] = P(c, r), [x1] = P(e, r);
        let near = 0, total = 0;
        for (let x = x0; x <= x1 + 1e-6; x += 0.4) { total++; if (prims.some((q) => distPrim(q, [x, y]) < 0.8)) near++; }
        const covered = near >= 0.7 * total;
        if (!covered) prims.push(L(x0, y, x1, y));
      }
      c = e + 1;
    }
  }
  return prims;
}
function rdp(pts, eps) {
  if (pts.length < 3) return pts.slice();
  const [a, b] = [pts[0], pts[pts.length - 1]];
  let best = -1, bi = 0;
  for (let i = 1; i < pts.length - 1; i++) {
    const d = dist(a, b) < 1e-9 ? dist(pts[i], a) : Math.abs(cross(sub(b, a), sub(pts[i], a))) / dist(a, b);
    if (d > best) { best = d; bi = i; }
  }
  if (best <= eps) return [a, b];
  return rdp(pts.slice(0, bi + 1), eps).slice(0, -1).concat(rdp(pts.slice(bi), eps));
}
// Polyline → lines with tangent fillets at gentle vertices; sharp vertices stay as corners.
function polyToPrims(poly, closed) {
  const n = poly.length, out = [];
  const verts = closed ? poly.slice(0, -1) : poly;
  const m = verts.length;
  if (m < 2) return out;
  const segLen = (i) => dist(verts[i], verts[(i + 1) % m]);
  const trimStart = new Array(m).fill(0), trimEnd = new Array(m).fill(0), fillets = [];
  const vertexIdx = closed ? [...Array(m).keys()] : [...Array(m).keys()].slice(1, -1);
  for (const i of vertexIdx) {
    const prev = verts[(i - 1 + m) % m], V = verts[i], next = verts[(i + 1) % m];
    const u = norm(sub(V, prev)), w = norm(sub(next, V));
    const turn = Math.acos(Math.max(-1, Math.min(1, dot(u, w))));
    if (turn < 1e-3 || turn > (55 * Math.PI) / 180) continue;
    const t = Math.min(0.48 * dist(prev, V), 0.48 * dist(V, next));
    const rho = t / Math.tan(turn / 2);
    const P1 = sub(V, mul(u, t)), P2 = add(V, mul(w, t));
    const left = cross(u, w) > 0; // turning left: centre on the left of u
    const nrm = left ? [-u[1], u[0]] : [u[1], -u[0]];
    const C = add(P1, mul(nrm, rho));
    let a0 = angOf(sub(P1, C)), a1 = angOf(sub(P2, C));
    if (!left) [a0, a1] = [a1, a0];
    while (a1 < a0) a1 += 360;
    fillets.push({ t: 'A', c: C, r: rho, a0, a1, full: false });
    trimEnd[(i - 1 + m) % m] = t; trimStart[i] = t;
  }
  const segs = closed ? m : m - 1;
  for (let i = 0; i < segs; i++) {
    const A_ = verts[i], B_ = verts[(i + 1) % m], d = norm(sub(B_, A_));
    const a = add(A_, mul(d, trimStart[i])), b = sub(B_, mul(d, trimEnd[i]));
    if (dist(a, b) > 0.05) out.push({ t: 'L', a, b });
  }
  return out.concat(fillets);
}
// Serifs: short protrusions at a stroke's end, perpendicular to it.
//  1. a foot or head: the end row of a stem (≤ 2 px wide) sticks out by 1–2 px on a side, and the stroke does
//     not continue past that row (so a crossbar, which has stem on both sides, is never taken);
//  2. a nub: a 1–2 px vertical tick hanging off the end of a bar (4+ px), touching nothing else.
function stripSerifs(rows, W, H) {
  const ink = (c, r) => r >= 0 && r < H && c >= 0 && c < W && (rows[r] & (0x80 >> c)) !== 0;
  const out = rows.slice();
  const clear = (c, r) => { out[r] &= ~(0x80 >> c) & 0xff; };
  const runsIn = (r, c0, c1) => {
    const res = [];
    for (let c = c0; c <= c1;) {
      if (!ink(c, r)) { c++; continue; }
      let e = c;
      while (e + 1 <= c1 && ink(e + 1, r)) e++;
      res.push([c, e]); c = e + 1;
    }
    return res;
  };
  for (let r = 0; r < H; r++) for (const [c0, c1] of runsIn(r, 0, W - 1)) {
    for (const dir of [-1, 1]) { // stem above (foot) or below (head)
      const s = runsIn(r + dir, c0, c1);
      if (s.length !== 1) continue;
      const [s0, s1] = s[0];
      if (s1 - s0 + 1 > 2 || runsIn(r - dir, c0, c1).length || !runsIn(r + 2 * dir, s0, s1).length) continue;
      const el = s0 - c0, er = c1 - s1;
      if ((el === 0 && er === 0) || el > 2 || er > 2) continue;
      // A curve meeting the stem continues diagonally into the next row; a serif touches nothing there.
      const side = [];
      for (let c = c0; c < s0; c++) side.push(c);
      for (let c = s1 + 1; c <= c1; c++) side.push(c);
      if (side.some((c) => [-1, 0, 1].some((dc) => (c + dc < s0 || c + dc > s1) && ink(c + dc, r + dir)))) continue;
      side.forEach((c) => clear(c, r));
    }
    if (c1 - c0 + 1 < 4) continue;
    for (const c of [c0, c1]) for (const dir of [-1, 1]) {
      let n = 0;
      while (n < 3 && ink(c, r + dir * (n + 1))) n++;
      if (n === 0 || n > 2) continue;
      let lone = true;
      for (let k = 1; k <= n; k++) if (ink(c - 1, r + dir * k) || ink(c + 1, r + dir * k)) lone = false;
      const tip = r + dir * (n + 1);
      if (ink(c - 1, tip) || ink(c, tip) || ink(c + 1, tip)) lone = false;
      if (lone) for (let k = 1; k <= n; k++) clear(c, r + dir * k);
    }
  }
  return out;
}
// Replace the model letters with a vectorised bitmap face.
function useBitmapFont(font) {
  const k = 8 / (font.baseline - font.xTopRow - 0.5);
  font.used = {};
  for (const [ch, rows0] of Object.entries(font.glyphs)) {
    const rows = VEC.serifs ? rows0 : stripSerifs(rows0, font.width, font.height);
    font.used[ch] = rows;
    FONT[ch] = vectorizeGlyph(rows, font.width, font.height, font.baseline, k);
    delete ANALYSIS[ch];
  }
  return { k, cell: font.width * k, line: font.height * k };
}
