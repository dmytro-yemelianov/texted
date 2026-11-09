// Lace-of-letters core: a monoline stroke model (lines + circular arcs),
// automatic connectors, and link search between neighbouring letters.
// World units: x-height 8, cap 12, descender -4, letter width 8. y points up.
const D2R = Math.PI / 180;
const add = (a, b) => [a[0] + b[0], a[1] + b[1]];
const sub = (a, b) => [a[0] - b[0], a[1] - b[1]];
const mul = (a, k) => [a[0] * k, a[1] * k];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1];
const cross = (a, b) => a[0] * b[1] - a[1] * b[0];
const len = (a) => Math.hypot(a[0], a[1]);
const norm = (a) => { const l = len(a) || 1; return [a[0] / l, a[1] / l]; };
const neg = (a) => [-a[0], -a[1]];
const dist = (a, b) => len(sub(a, b));
const angDeg = (u, v) => Math.acos(Math.max(-1, Math.min(1, dot(norm(u), norm(v))))) / D2R;
const angOf = (v) => Math.atan2(v[1], v[0]) / D2R;
const wrap180 = (d) => ((d % 360) + 540) % 360 - 180;

const L = (x1, y1, x2, y2) => ({ t: 'L', a: [x1, y1], b: [x2, y2] });
const A = (cx, cy, r, a0, a1) => ({ t: 'A', c: [cx, cy], r, a0, a1, full: false });
const O = (cx, cy, r) => ({ t: 'A', c: [cx, cy], r, a0: 0, a1: 360, full: true });

const FONT = {};
const def = (chars, prims) => { for (const ch of chars) FONT[ch] = prims; };
// Cyrillic and Latin homoglyphs share one drawing in this model.
def('АA', [L(0, 0, 4, 12), L(8, 0, 4, 12), L(4 / 3, 4, 8 - 4 / 3, 4)]);
def('аa', [L(8, 0, 8, 6.95), A(5.5, 4.5, 3.5, 44.4, 160), A(8, 2.4, 2.4, 90, 270)]);
def('оo', [O(4, 4, 4)]);
def('сc', [A(4, 4, 4, 40, 320)]);
def('еe', [L(0, 4, 8, 4), A(4, 4, 4, 0, 320)]);
def('рp', [L(0, -4, 0, 8), O(4, 4, 4)]);
def('іi', [L(4, 0, 4, 8), O(4, 10.5, 0.7)]);
def('т', [L(0, 8, 8, 8), L(4, 0, 4, 8)]);
def('t', [L(3, 2, 3, 11), L(0, 8, 7, 8), A(5, 2, 2, 180, 270)]);
def('ж', [L(4, 0, 4, 8), L(0, 8, 4, 4), L(8, 8, 4, 4), L(0, 0, 4, 4), L(8, 0, 4, 4)]);
def('к', [L(0, 0, 0, 8), L(0, 4, 7, 8), L(0, 4, 8, 0)]);
def('в', [L(0, 0, 0, 8), L(0, 8, 5, 8), A(5, 6, 2, -90, 90), L(0, 4, 5.5, 4), A(5.5, 2, 2, -90, 90), L(0, 0, 5.5, 0)]);
def('л', [L(2.5, 8, 8, 8), L(8, 0, 8, 8), L(2.5, 8, 0, 0)]);
def('м', [L(0, 0, 0, 8), L(8, 0, 8, 8), L(0, 8, 4, 3), L(8, 8, 4, 3)]);
def('и', [L(0, 0, 0, 8), L(8, 0, 8, 8), L(0, 0, 8, 8)]);
def('п', [L(0, 0, 0, 8), L(8, 0, 8, 8), L(0, 8, 8, 8)]);
def('l', [L(4, 0, 4, 12)]);
def('v', [L(0, 8, 4, 0), L(8, 8, 4, 0)]);
def('f', [L(3, 0, 3, 9.5), A(5.5, 9.5, 2.5, 30, 180), L(0, 8, 6.5, 8)]);
def('j', [L(5, -2, 5, 8), A(3, -2, 2, 180, 360), O(5, 10.5, 0.7)]);
def('b', [L(0, 0, 0, 12), O(4, 4, 4)]);
def('h', [L(0, 0, 0, 12), A(4, 4, 4, 0, 180), L(8, 0, 8, 4)]);
def('n', [L(0, 0, 0, 8), A(4, 4, 4, 0, 180), L(8, 0, 8, 4)]);
def('k', [L(0, 0, 0, 12), L(0, 4, 7, 8), L(0, 4, 8, 0)]);
def('TТ', [L(0, 12, 8, 12), L(4, 0, 4, 12)]);
def('s', [A(4, 6, 2, 30, 270), A(4, 2, 2, -150, 90)]);
def('г', [L(0, 0, 0, 8), L(0, 8, 7, 8)]);
def('Г', [L(0, 0, 0, 12), L(0, 12, 8, 12)]);
def('б', [O(4, 4, 4), L(0, 4, 0, 9), A(3, 9, 3, 90, 180), L(3, 12, 8, 12)]);
def('ф', [L(4, -4, 4, 12), O(4, 4, 4)]);
def('ї', [L(4, 0, 4, 8), O(2.5, 10.5, 0.7), O(5.5, 10.5, 0.7)]);
def('й', [L(0, 0, 0, 8), L(8, 0, 8, 8), L(0, 0, 8, 8), A(4, 12, 2.5, 200, 340)]);
def('є', [A(4, 4, 4, 40, 320), L(0, 4, 5, 4)]);
def('н', [L(0, 0, 0, 8), L(8, 0, 8, 8), L(0, 4, 8, 4)]);

const WORDS = ['мереживо', 'lattice', 'плетиво', 'toile', 'квітка', 'veil', 'сітка', 'lace', 'мітла', 'voile', 'літо', 'tile', 'вітер', 'тло'];

function arcPt(p, a) { return [p.c[0] + p.r * Math.cos(a * D2R), p.c[1] + p.r * Math.sin(a * D2R)]; }
function inArc(p, a, m = 0) {
  if (p.full) return true;
  const d = (((a - p.a0) % 360) + 360) % 360;
  return d >= m - 1e-6 && d <= p.a1 - p.a0 - m + 1e-6;
}
function distSeg(P, a, b) {
  const ab = sub(b, a);
  const t = Math.max(0, Math.min(1, dot(sub(P, a), ab) / (dot(ab, ab) || 1)));
  return dist(P, add(a, mul(ab, t)));
}
function distPrim(q, X) {
  if (q.t === 'L') return distSeg(X, q.a, q.b);
  if (inArc(q, angOf(sub(X, q.c)))) return Math.abs(dist(X, q.c) - q.r);
  return Math.min(dist(X, arcPt(q, q.a0)), dist(X, arcPt(q, q.a1)));
}
// Directions in which stroke q leaves point P (empty if q does not touch P).
function leaveDirs(q, P) {
  if (q.t === 'L') {
    if (dist(q.a, P) < 0.05) return [norm(sub(q.b, q.a))];
    if (dist(q.b, P) < 0.05) return [norm(sub(q.a, q.b))];
    if (distSeg(P, q.a, q.b) < 0.05) { const v = norm(sub(q.b, q.a)); return [v, neg(v)]; }
    return [];
  }
  if (Math.abs(dist(P, q.c) - q.r) > 0.05) return [];
  const a = angOf(sub(P, q.c));
  if (!q.full) {
    if (Math.abs(wrap180(a - q.a0)) < 0.5) return [[-Math.sin(q.a0 * D2R), Math.cos(q.a0 * D2R)]];
    if (Math.abs(wrap180(a - q.a1)) < 0.5) return [[Math.sin(q.a1 * D2R), -Math.cos(q.a1 * D2R)]];
    if (!inArc(q, a)) return [];
  }
  const t = [-Math.sin(a * D2R), Math.cos(a * D2R)];
  return [t, neg(t)];
}
function primPts(q) {
  if (q.t === 'L') return [q.a, q.b];
  const span = q.full ? 360 : q.a1 - q.a0, pts = [];
  for (let k = 0; k <= 48; k++) pts.push(arcPt(q, q.a0 + (span * k) / 48));
  return pts;
}
function bboxOf(prims) {
  const b = { x0: 1e9, y0: 1e9, x1: -1e9, y1: -1e9 };
  for (const q of prims) for (const p of primPts(q)) {
    b.x0 = Math.min(b.x0, p[0]); b.y0 = Math.min(b.y0, p[1]);
    b.x1 = Math.max(b.x1, p[0]); b.y1 = Math.max(b.y1, p[1]);
  }
  return b;
}

// Connectors of one letter, in its own coordinates.
//  ray:  a straight line end, or an arc end continued along its tangent;
//  host: an arc or circle that another line may touch tangentially.
const ANALYSIS = {};
function analyse(ch) {
  if (ANALYSIS[ch]) return ANALYSIS[ch];
  const prims = FONT[ch], bbox = bboxOf(prims), rays = [], hosts = [], latent = [];
  const tryRay = (P, d, i, kind) => {
    const leaving = [];
    prims.forEach((q, j) => { if (j !== i) leaving.push(...leaveDirs(q, P)); });
    const minAng = leaving.length ? Math.min(...leaving.map((v) => angDeg(d, v))) : 180;
    // A line may continue through a joint unless it runs into another stroke;
    // an arc end continues only when it leaves cleanly across the joint.
    if (minAng < (kind === 'arc' ? 60 : 25)) return;
    const touching = new Set();
    prims.forEach((q, j) => { if (j !== i && distPrim(q, P) < 0.05) touching.add(j); });
    for (let t = 0.4; t < 40; t += 0.2) {
      const X = add(P, mul(d, t));
      if (X[0] < bbox.x0 - 0.3 || X[0] > bbox.x1 + 0.3 || X[1] < bbox.y0 - 0.3 || X[1] > bbox.y1 + 0.3) {
        rays.push({ P, d, prim: i, kind });
        return;
      }
      for (let j = 0; j < prims.length; j++) {
        if (j === i || (touching.has(j) && t < 0.6)) continue;
        if (distPrim(prims[j], X) < 0.3) {
          // Blocked only by a dot: the connector is latent and opens when a ligature absorbs the dot.
          if (prims[j].full && prims[j].r < 1) latent.push({ P, d, prim: i, kind, absorbs: j });
          return; // runs into its own letter
        }
      }
    }
  };
  prims.forEach((q, i) => {
    if (q.t === 'L') {
      tryRay(q.a, norm(sub(q.a, q.b)), i, 'line');
      tryRay(q.b, norm(sub(q.b, q.a)), i, 'line');
    } else {
      hosts.push({ prim: i });
      if (!q.full) {
        tryRay(arcPt(q, q.a0), [Math.sin(q.a0 * D2R), -Math.cos(q.a0 * D2R)], i, 'arc');
        tryRay(arcPt(q, q.a1), [-Math.sin(q.a1 * D2R), Math.cos(q.a1 * D2R)], i, 'arc');
      }
    }
  });
  return (ANALYSIS[ch] = { rays, hosts, latent, bbox, center: [(bbox.x0 + bbox.x1) / 2, (bbox.y0 + bbox.y1) / 2] });
}

function tfPt(tf, p) { const c = Math.cos(tf.rot), s = Math.sin(tf.rot); return [tf.x + c * p[0] - s * p[1], tf.y + s * p[0] + c * p[1]]; }
function tfDir(tf, d) { const c = Math.cos(tf.rot), s = Math.sin(tf.rot); return [c * d[0] - s * d[1], s * d[0] + c * d[1]]; }
function tfPrim(tf, q) {
  if (q.t === 'L') return { t: 'L', a: tfPt(tf, q.a), b: tfPt(tf, q.b) };
  const deg = tf.rot / D2R;
  return { t: 'A', c: tfPt(tf, q.c), r: q.r, a0: q.a0 + deg, a1: q.a1 + deg, full: q.full };
}
function makeGlyph(ch, tf, meta) {
  const an = analyse(ch);
  return Object.assign({
    ch, tf,
    prims: FONT[ch].map((q) => tfPrim(tf, q)),
    rays: an.rays.map((r) => ({ P: tfPt(tf, r.P), d: tfDir(tf, r.d), prim: r.prim, kind: r.kind, used: false })),
    hosts: an.hosts.map((h) => ({ prim: h.prim, att: [] })),
    latent: an.latent.map((r) => ({ P: tfPt(tf, r.P), d: tfDir(tf, r.d), prim: r.prim, absorbs: r.absorbs })),
    center: tfPt(tf, an.center),
  }, meta);
}

function rayRay(a, b) {
  const cr = cross(a.d, b.d), w = sub(b.P, a.P);
  if (Math.abs(cr) < Math.sin(4 * D2R)) {
    if (dot(a.d, b.d) > -0.99) return null; // parallel, not facing
    const lat = Math.abs(cross(w, a.d)), along = dot(w, a.d);
    if (lat < 0.4 && along >= -0.01) return { segs: [[a.P, b.P]], len: Math.max(0, along), t: along / 2, s: along / 2 };
    return null;
  }
  const t = cross(w, b.d) / cr, s = cross(w, a.d) / cr;
  if (t < 0.3 || s < 0.3) return null;
  const X = add(a.P, mul(a.d, t));
  return { segs: [[a.P, X], [b.P, X]], len: t + s, t, s };
}
function tangentsFrom(P, c, r) {
  const v = sub(c, P), D = len(v);
  if (D <= r + 1e-6) return [];
  const base = Math.atan2(v[1], v[0]), phi = Math.asin(r / D), l = Math.sqrt(D * D - r * r);
  return [-1, 1].map((s) => { const a = base + s * phi, dir = [Math.cos(a), Math.sin(a)]; return { dir, T: add(P, mul(dir, l)), len: l }; });
}
function commonTangents(c1, r1, c2, r2) {
  const d = sub(c2, c1), D = len(d);
  if (D < 1e-6) return [];
  const v = mul(d, 1 / D), out = [];
  for (const k of [1, -1]) { // 1: outer (belt), -1: crossed
    const h = (r1 - k * r2) / D;
    if (Math.abs(h) > 1) continue;
    const s = Math.sqrt(1 - h * h);
    for (const side of [1, -1]) {
      const n = [v[0] * h - side * v[1] * s, v[1] * h + side * v[0] * s];
      out.push({ T1: add(c1, mul(n, r1)), T2: add(c2, mul(n, k * r2)) });
    }
  }
  return out;
}

function mulberry32(seed) {
  return function () {
    seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// P: { reach, angTol, density, hostCap, enabled:{ctx:bool}, weight:{ctx:number}, ctxOf(g,h) }
function buildLinks(glyphs, P, rng) {
  const cell = 12 + P.reach, hash = new Map(), key = (i, j) => i + ',' + j;
  glyphs.forEach((g, gi) => {
    g.id = gi;
    const k = key(Math.floor(g.center[0] / cell), Math.floor(g.center[1] / cell));
    if (!hash.has(k)) hash.set(k, []);
    hash.get(k).push(g);
  });
  const near = (x, y, rad) => {
    const out = [];
    for (let i = Math.floor((x - rad) / cell); i <= Math.floor((x + rad) / cell); i++)
      for (let j = Math.floor((y - rad) / cell); j <= Math.floor((y + rad) / cell); j++) {
        const a = hash.get(key(i, j));
        if (a) for (const g of a) if (dist(g.center, [x, y]) <= rad) out.push(g);
      }
    return out;
  };
  const cands = [];
  for (const g of glyphs) {
    for (const h of near(g.center[0], g.center[1], 12 + P.reach)) {
      if (h.id <= g.id) continue;
      const ctx = P.ctxOf(g, h);
      if (!ctx || !P.enabled[ctx]) continue;
      for (const a of g.rays) for (const b of h.rays) {
        const r = rayRay(a, b);
        if (!r || r.t > P.reach || r.s > P.reach) continue;
        cands.push({ kind: 'rr', ctx, segs: r.segs, cost: r.len, rays: [a, b], hosts: [], excl: [[g, a.prim], [h, b.prim]] });
      }
      for (const [G1, G2] of [[g, h], [h, g]]) for (const a of G1.rays) for (const ho of G2.hosts) {
        const q = G2.prims[ho.prim];
        for (const tg of tangentsFrom(a.P, q.c, q.r)) {
          if (tg.len > P.reach || tg.len < 0.3) continue;
          const dev = angDeg(a.d, tg.dir);
          if (dev > P.angTol) continue;
          const ta = angOf(sub(tg.T, q.c));
          if (!inArc(q, ta, 8)) continue;
          cands.push({ kind: 'rh', ctx, segs: [[a.P, tg.T]], cost: tg.len + dev * 0.15, rays: [a], hosts: [[ho, ta]], excl: [[G1, a.prim], [G2, ho.prim]] });
        }
      }
      for (const h1 of g.hosts) for (const h2 of h.hosts) {
        const q1 = g.prims[h1.prim], q2 = h.prims[h2.prim];
        for (const ct of commonTangents(q1.c, q1.r, q2.c, q2.r)) {
          const l = dist(ct.T1, ct.T2);
          if (l > P.reach || l < 0.3) continue;
          const a1 = angOf(sub(ct.T1, q1.c)), a2 = angOf(sub(ct.T2, q2.c));
          if (!inArc(q1, a1, 8) || !inArc(q2, a2, 8)) continue;
          cands.push({ kind: 'hh', ctx, segs: [[ct.T1, ct.T2]], cost: l + 1.5, rays: [], hosts: [[h1, a1], [h2, a2]], excl: [[g, h1.prim], [h, h2.prim]] });
        }
      }
    }
  }
  const clear = (c) => {
    for (const [A_, B_] of c.segs) {
      const l = dist(A_, B_);
      if (l < 1) continue;
      const dir = norm(sub(B_, A_)), mid = mul(add(A_, B_), 0.5);
      const gs = near(mid[0], mid[1], l / 2 + 12);
      for (let t = 0.5; t <= l - 0.5; t += 0.35) {
        const X = add(A_, mul(dir, t));
        for (const G of gs) {
          if (dist(G.center, X) > 11) continue;
          for (let qi = 0; qi < G.prims.length; qi++) {
            if (c.excl.some(([EG, ei]) => EG === G && ei === qi)) continue;
            if (distPrim(G.prims[qi], X) < 0.3) return false;
          }
        }
      }
    }
    return true;
  };
  const w = (c) => c.cost * (P.weight[c.ctx] || 1);
  cands.sort((a, b) => w(a) - w(b));
  const out = [];
  for (const c of cands) {
    if (c.rays.some((r) => r.used)) continue;
    if (c.hosts.some(([ho, a]) => ho.att.length >= P.hostCap || ho.att.some((b) => Math.abs(wrap180(a - b)) < 30))) continue;
    if (!clear(c)) continue;
    if (rng() > P.density) continue;
    c.rays.forEach((r) => (r.used = true));
    c.hosts.forEach(([ho, a]) => ho.att.push(a));
    out.push(c);
  }
  return out;
}

// Layouts --------------------------------------------------------------
const rowCtx = (g, h) => {
  if (g.line === h.line) {
    if (g.word === h.word) return Math.abs(g.idx - h.idx) === 1 ? 'in' : null;
    return Math.abs(g.word - h.word) === 1 ? 'words' : null;
  }
  return Math.abs(g.line - h.line) === 1 ? 'lines' : null;
};
function layoutRows(lines, o) { // lines: array of arrays of words; o: tracking, wordGap, leading, x0, top
  const glyphs = [];
  lines.forEach((words, li) => {
    let x = o.x0 || 0;
    const y = (o.top || 0) - li * o.leading;
    words.forEach((word, wi) => {
      [...word].forEach((ch, ci) => {
        glyphs.push(makeGlyph(ch, { x, y, rot: 0 }, { line: li, word: wi, idx: ci, last: ci === [...word].length - 1 }));
        x += 8 + o.tracking;
      });
      x += o.wordGap - o.tracking;
    });
  });
  return glyphs;
}
function wordWidth(w, o) { return [...w].length * (8 + o.tracking) - o.tracking; }
function layoutGrid(W, H, o, seed) {
  const rng = mulberry32(seed), lines = [];
  let k = Math.floor(rng() * WORDS.length);
  for (let y = H - 12; y >= 4; y -= o.leading) {
    const words = [];
    let x = 0;
    while (true) {
      const w = WORDS[k % WORDS.length];
      if (x + wordWidth(w, o) > W && words.length) break;
      words.push(w); k++;
      x += wordWidth(w, o) + o.wordGap;
      if (x > W) break;
    }
    k += 3;
    lines.push(words);
  }
  return layoutRows(lines, Object.assign({}, o, { x0: 0, top: H - 12 }));
}
function layoutDoily(R, o, seed) {
  const rng = mulberry32(seed), glyphs = [];
  let k = Math.floor(rng() * WORDS.length);
  for (let ring = 0, r = 16; r + 12 <= R; ring++, r += o.leading) {
    const circ = 2 * Math.PI * r, th0 = rng() * 2 * Math.PI;
    let s = 0, wi = 0;
    while (true) {
      const w = WORDS[k % WORDS.length], ww = wordWidth(w, o);
      if (s + ww > circ - o.wordGap) break;
      [...w].forEach((ch, ci) => {
        const th = th0 - (s + ci * (8 + o.tracking)) / r;
        const base = [r * Math.cos(th), r * Math.sin(th)];
        glyphs.push(makeGlyph(ch, { x: base[0], y: base[1], rot: th - Math.PI / 2 }, { line: ring, word: wi, idx: ci, last: ci === [...w].length - 1 }));
      });
      s += ww + o.wordGap; wi++; k++;
    }
    k += 2;
  }
  return glyphs;
}
function layoutScatter(W, H, o, seed) {
  const rng = mulberry32(seed), pts = [], md = 11 + o.tracking * 2;
  for (let i = 0; i < 6000 && pts.length < 400; i++) {
    const p = [rng() * W, rng() * H];
    if (pts.every((q) => dist(p, q) >= md)) pts.push(p);
  }
  const letters = WORDS.join('');
  return pts.map((p) => {
    const ch = letters[Math.floor(rng() * letters.length)];
    const an = analyse(ch), rot = rng() * 2 * Math.PI;
    const c = tfDir({ rot }, an.center);
    return makeGlyph(ch, { x: p[0] - c[0], y: p[1] - c[1], rot }, {});
  });
}

if (typeof module !== 'undefined') module.exports = { FONT, analyse, makeGlyph, tfPt, arcPt, distPrim, angOf, add, sub, mul, dot, len, norm, dist, D2R, wrap180, buildLinks, layoutGrid, layoutDoily, layoutScatter, layoutRows, rowCtx, mulberry32 };
