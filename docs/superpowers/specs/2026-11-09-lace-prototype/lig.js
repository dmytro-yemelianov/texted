// Ligatures from the connector model, in four situations.
//  in a word:   fuse (top bars meet), barstem (bar runs onto a stem top), share (strokes coincide when touching),
//               hook / arch (a free arc end leaves its arc tangentially and lands on a stem top, two arcs),
//               +dot (the stem top was latent under a dot; the dot is absorbed).
//  between words: rail (top bars across the space), swash (a two-arc bridge over or under the space).
//  between lines: stem (a descender continues straight into a stem below), tail (a two-arc bridge downwards).
function arcPoints(S, ts, E) {
  const n = [-ts[1], ts[0]], w = sub(E, S), k = dot(n, w);
  if (Math.abs(k) < 1e-6) return { pts: [S, E], turn: 0, r: Infinity };
  const s = dot(w, w) / (2 * k), C = add(S, mul(n, s)), r = Math.abs(s);
  const aS = angOf(sub(S, C)), aE = angOf(sub(E, C));
  const sweep = s > 0 ? (((aE - aS) % 360) + 360) % 360 : -((((aS - aE) % 360) + 360) % 360);
  const pts = [], steps = Math.max(2, Math.ceil(Math.abs(sweep) / 6));
  for (let i = 0; i <= steps; i++) pts.push(arcPt({ c: C, r }, aS + (sweep * i) / steps));
  return { pts, turn: Math.abs(sweep), r };
}
function biarc(P1, t1, P2, t2) {
  const v = sub(P2, P1), t = add(t1, t2), denom = 2 * (1 - dot(t1, t2));
  const vt = dot(v, t);
  let alpha;
  if (denom < 1e-6) { if (vt <= 1e-9) return null; alpha = dot(v, v) / (2 * vt); } // parallel tangents: S-curve
  else alpha = (-vt + Math.sqrt(vt * vt + denom * dot(v, v))) / denom;
  if (!(alpha > 0)) return null;
  const J = mul(add(add(P1, mul(t1, alpha)), sub(P2, mul(t2, alpha))), 0.5);
  const a1 = arcPoints(P1, t1, J), a2r = arcPoints(P2, neg(t2), J);
  const pts = a1.pts.concat(a2r.pts.slice(0, -1).reverse());
  let length = 0;
  for (let i = 1; i < pts.length; i++) length += dist(pts[i - 1], pts[i]);
  return { pts, turn: a1.turn + a2r.turn, rmin: Math.min(a1.r, a2r.r), length };
}

// Free ends a ligature may reroute. From a free end, walk back along the stroke while it continues end to end
// (short lines and arcs, up to 110° of turning and 9 units): every arc on the way can be the departure point;
// everything between it and the free end is replaced by the bridge.
const LIG = { additive: false, bandPairs: true }; // additive: ligatures only add strokes; nothing is trimmed or absorbed
function freeTerminals(G) {
  const out = [];
  const touching = (P, except) => G.prims.map((_, oi) => oi).filter((oi) => !except.includes(oi) && distPrim(G.prims[oi], P) < 0.05);
  const ends = (q) => (q.t === 'L' ? [['a', q.a], ['b', q.b]] : q.full ? [] : [['a0', arcPt(q, q.a0)], ['a1', arcPt(q, q.a1)]]);
  const plen = (q) => (q.t === 'L' ? dist(q.a, q.b) : (q.r * (q.a1 - q.a0) * Math.PI) / 180);
  G.prims.forEach((q0, i0) => {
    for (const [e0, F] of ends(q0)) {
      if (touching(F, [i0]).length) continue;
      const rk = G.rays.findIndex((r) => r.prim === i0 && dist(r.P, F) < 0.05);
      const base = [G.id + 'T' + i0 + e0].concat(rk >= 0 ? [G.id + 'r' + rk] : []);
      const drops = [];
      let cur = i0, entry = e0, turn = 0, length = 0;
      for (let step = 0; step < 6; step++) {
        const q = G.prims[cur];
        if (q.t === 'A') out.push({ qi: cur, end: entry, drops: drops.slice(), maxCut: Math.max(0, Math.min(q.a1 - q.a0 - 5, 110 - turn)), keys: base.concat(drops.map((d) => G.id + 'D' + d), [G.id + 'Q' + cur]) });
        if (q.t === 'A') turn += q.a1 - q.a0;
        length += plen(q);
        if (LIG.additive || turn >= 110 || length > 9) break;
        const other = ends(q).find(([e]) => e !== entry);
        if (!other) break;
        const next = touching(other[1], [cur]);
        if (next.length !== 1) break;
        const ne = ends(G.prims[next[0]]).find(([, Pn]) => dist(Pn, other[1]) < 0.05);
        if (!ne) break;
        drops.push(cur); cur = next[0]; entry = ne[0];
      }
    }
  });
  return out;
}
function departures(G, term) {
  const q = G.prims[term.qi], out = [];
  // A ligature may reroute only the end of an arc: at most 100° and 60% of it, so the letter stays readable.
  const maxCut = LIG.additive ? 0 : term.drops.length ? term.maxCut : Math.min(100, 0.6 * (q.a1 - q.a0));
  for (let k = 0; k * 5 <= maxCut; k++) {
    const th = term.end === 'a0' ? q.a0 + k * 5 : q.a1 - k * 5;
    if ((term.end === 'a0' && th > q.a1 - 10) || (term.end === 'a1' && th < q.a0 + 10)) break;
    out.push({
      S: arcPt(q, th),
      d: term.end === 'a0' ? [Math.sin(th * D2R), -Math.cos(th * D2R)] : [-Math.sin(th * D2R), Math.cos(th * D2R)],
      kept: Object.assign({}, q, term.end === 'a0' ? { a0: th } : { a1: th }),
    });
  }
  return out;
}
// Every height and direction test runs in a letter's own frame, so rotated letters behave like upright ones.
const rotv = (v, a) => [Math.cos(a) * v[0] - Math.sin(a) * v[1], Math.sin(a) * v[0] + Math.cos(a) * v[1]];
const locP = (G, p) => rotv(sub(p, [G.tf.x, G.tf.y]), -G.tf.rot);
const locD = (G, d) => rotv(d, -G.tf.rot);
function targetsOf(H, pred) {
  const out = [];
  const ok = (r) => pred({ P: locP(H, r.P), d: locD(H, r.d) });
  H.rays.forEach((r, k) => { if (ok(r)) out.push({ P: r.P, d: r.d, prim: r.prim, absorbs: -1, keys: [H.id + 'r' + k] }); });
  if (!LIG.additive) (H.latent || []).forEach((r, k) => { if (ok(r)) out.push({ P: r.P, d: r.d, prim: r.prim, absorbs: r.absorbs, keys: [H.id + 'l' + k, H.id + 'dot' + r.absorbs] }); });
  return out;
}
function clearPath(pts, S, T, near, skip, over) {
  for (const p of pts) {
    if (dist(p, S) < 0.7 || dist(p, T) < 0.7) continue;
    for (const G of near) for (let qi = 0; qi < G.prims.length; qi++) {
      const k = G.id + ':' + qi;
      let q = G.prims[qi];
      if (over[k]) q = over[k];
      else if (skip.has(k)) continue;
      if (distPrim(q, p) < 0.4) return false;
    }
  }
  return true;
}
function tryBridge(G, S, d, t, o, near, skip, over) {
  const v = sub(t.P, S), l = len(v);
  let b;
  if (l > 0.05 && Math.abs(cross(d, v)) / l < 0.03 && dot(d, v) > 0 && dot(neg(t.d), mul(v, 1 / l)) > 0.999)
    b = { pts: [S, t.P], turn: 0, length: l, rmin: Infinity };
  else b = biarc(S, d, t.P, neg(t.d));
  if (!b || b.rmin < (o.rmin || 1.4) || b.turn > o.maxTurn || b.length > o.maxLen) return null;
  const ly = b.pts.map((p) => locP(G, p)[1]);
  if (o.noDive && Math.min(...ly) < Math.min(locP(G, S)[1], locP(G, t.P)[1]) - 0.6) return null;
  if (o.band && !ly.every(o.band)) return null;
  if (!clearPath(b.pts, S, t.P, near, skip, over)) return null;
  return { pts: b.pts, turn: b.turn, length: b.length, cost: b.length + b.turn * 0.05, straight: b.turn === 0 };
}
function bridges(G, H, o, near) {
  const out = [], tg = targetsOf(H, o.target);
  const skipT = (t) => [H.id + ':' + t.prim].concat(t.absorbs >= 0 ? [H.id + ':' + t.absorbs] : []);
  const plen = (q) => (q.t === 'L' ? dist(q.a, q.b) : (q.r * (q.full ? 360 : q.a1 - q.a0) * Math.PI) / 180);
  const ink = G.prims.reduce((s, q) => s + plen(q), 0);
  if (o.depart) for (const term of freeTerminals(G)) for (const dep of departures(G, term)) {
    const local = { S: locP(G, dep.S), d: locD(G, dep.d) };
    if (!o.depart(local)) continue;
    // A ligature reroutes a stroke end; it never takes away more than a quarter of the letter.
    const removed = term.drops.reduce((s, d) => s + plen(G.prims[d]), 0) + plen(G.prims[term.qi]) - plen(dep.kept);
    if (removed > 0.25 * ink) continue;
    for (const t of tg) {
      const skip = new Set(skipT(t));
      for (const d of term.drops) skip.add(G.id + ':' + d);
      const b = tryBridge(G, dep.S, dep.d, t, o, near, skip, { [G.id + ':' + term.qi]: dep.kept });
      if (b) out.push(Object.assign(b, { keys: [...term.keys, ...t.keys], trim: [G, term.qi, dep.kept], drop: term.drops.length ? [G, term.drops] : null, absorb: t.absorbs >= 0 ? [H, t.absorbs] : null, srcY: local.S[1] }));
    }
  }
  if (o.fromRay) G.rays.forEach((r, k) => {
    const local = { P: locP(G, r.P), d: locD(G, r.d) };
    if (!o.fromRay(local)) return;
    for (const t of tg) {
      const b = tryBridge(G, r.P, r.d, t, o, near, new Set([G.id + ':' + r.prim, ...skipT(t)]), {});
      if (b) out.push(Object.assign(b, { keys: [G.id + 'r' + k, ...t.keys], trim: null, absorb: t.absorbs >= 0 ? [H, t.absorbs] : null, srcY: local.P[1] }));
    }
  });
  return out;
}
function barJoins(G, H, maxGap, fuseOnly) {
  const out = [];
  G.rays.forEach((a, i) => H.rays.forEach((b, j) => {
    const A_ = locP(G, a.P), B_ = locP(G, b.P), ad = locD(G, a.d), bd = locD(G, b.d);
    const dy = Math.abs(A_[1] - B_[1]), dx = B_[0] - A_[0];
    if (dy > 1.0 || dx < -0.3 || dx > maxGap || A_[1] < 6.0) return;
    const keys = [G.id + 'r' + i, H.id + 'r' + j];
    if (ad[0] > 0.85 && bd[0] < -0.85) out.push({ mech: 'fuse', segs: [[a.P, b.P]], keys, cost: Math.max(0, dx) });
    else if (!fuseOnly && ((ad[0] > 0.85 && bd[1] > 0.9) || (bd[0] < -0.85 && ad[1] > 0.9))) out.push({ mech: 'barstem', segs: [[a.P, b.P]], keys, cost: Math.max(0, dx) + 0.5 });
  }));
  return out;
}
function sharedStrokes(G, H) {
  const out = [];
  for (const p of G.prims) for (const q of H.prims) {
    if (p.t !== 'L' || q.t !== 'L') continue;
    const u = norm(sub(p.b, p.a)), v = norm(sub(q.b, q.a));
    if (Math.abs(cross(u, v)) > 0.01 || (distSeg(q.a, p.a, p.b) > 0.3 && distSeg(q.b, p.a, p.b) > 0.3 && distSeg(p.a, q.a, q.b) > 0.3)) continue;
    if (Math.abs(cross(sub(q.a, p.a), u)) > 0.3) continue;
    out.push({ mech: 'share', segs: [[p.a, p.b]], keys: [], cost: 0 });
  }
  return out;
}
const WORD_RULES = {
  // A stem top, or a serif at the top that points back towards the source.
  target: (r) => (r.d[1] > 0.9 && r.P[1] >= 6.0) || (r.d[0] < -0.85 && r.P[1] >= 6.0),
  depart: () => true, maxTurn: 240, maxLen: 18, noDive: true, rmin: 1.4,
};
function pairCandidates(G, H, near, maxGap = 4.5) {
  const out = [...barJoins(G, H, maxGap, false), ...sharedStrokes(G, H)];
  for (const b of bridges(G, H, WORD_RULES, near)) out.push(Object.assign(b, { mech: b.srcY >= 9.5 ? 'hook' : 'arch' }));
  return out;
}
function wordCandidates(G, H, gap, near) {
  const out = barJoins(G, H, gap + 3, true).map((c) => Object.assign(c, { mech: 'rail' }));
  const rules = {
    target: (r) => (r.d[1] > 0.9 && r.P[1] >= 6.0) || r.d[0] < -0.7,
    depart: (dep) => dep.d[0] > 0.2, fromRay: (r) => r.d[0] > 0.7 || (r.d[1] > 0.9 && r.P[1] >= 6.0),
    maxTurn: 220, maxLen: gap * 1.6 + 14, band: (y) => y > -3.5 && y < 15.5, rmin: 1.6,
  };
  for (const b of bridges(G, H, rules, near)) out.push(Object.assign(b, { mech: 'swash' }));
  return out;
}
function lineCandidates(G, H, leading, near) {
  const rules = {
    target: (r) => r.d[1] > 0.7, depart: (dep) => dep.d[1] < -0.3 && dep.S[1] < 2,
    fromRay: (r) => r.d[1] < -0.7, maxTurn: 110, maxLen: leading + 6, rmin: 2,
  };
  return bridges(G, H, rules, near).map((b) => Object.assign(b, { mech: b.straight ? 'stem' : 'tail' }));
}
const MECH_ORDER = { share: 0, fuse: 1, barstem: 1, hook: 1, arch: 1, rail: 2, swash: 2, stem: 3, tail: 3 };

// How g relates to h: 'word' (h follows g in a word), 'words' (h starts the next word), 'lines' (h sits under g).
function relation(g, h, o) {
  const p = locP(g, [h.tf.x, h.tf.y]);
  if (Math.abs(wrap180((h.tf.rot - g.tf.rot) / D2R)) > 35) return null;
  if (g.line !== undefined && h.line !== undefined) {
    if (g.line === h.line) {
      if (g.word === h.word) return h.idx === g.idx + 1 ? 'word' : null;
      return h.word === g.word + 1 && h.idx === 0 && g.last ? 'words' : null;
    }
    return Math.abs(g.line - h.line) === 1 && p[1] < -6 && Math.abs(p[0]) <= 9 ? 'lines' : null;
  }
  if (Math.abs(p[1]) < 3 && p[0] > 7 && p[0] < 8 + o.tracking + 3) return 'word';
  if (p[1] < -10 && p[1] > -(o.leading + 8) && Math.abs(p[0]) <= 9) return 'lines';
  return null;
}
// Choose ligatures for any layout. en: { word, words, lines }; o: tracking, wordGap, leading, density, seed.
function ligateGlyphs(glyphs, o, en) {
  glyphs.forEach((g, i) => (g.id = i));
  const rng = mulberry32((o.seed || 1) * 7919 + 13);
  const R = 8 + Math.max(o.leading, o.wordGap + 8) + 4;
  const cands = [];
  for (const g of glyphs) for (const h of glyphs) {
    if (g === h || Math.abs(g.tf.x - h.tf.x) > R || Math.abs(g.tf.y - h.tf.y) > R) continue;
    const rel = relation(g, h, o);
    if (!rel || !en[rel]) continue;
    const mid = mul(add(g.center, h.center), 0.5);
    const near = glyphs.filter((x) => dist(x.center, mid) < R + 12);
    const list = rel === 'word' ? pairCandidates(g, h, near) : rel === 'words' ? wordCandidates(g, h, o.wordGap, near) : lineCandidates(g, h, o.leading, near);
    const slots = ['pair:' + g.id + ':' + h.id].concat(rel === 'lines' ? [g.id + ':D', h.id + ':U'] : [g.id + ':F', h.id + ':B']);
    for (const c of list) { c.ctx = rel; c.keys = c.keys.concat(slots); cands.push(c); }
  }
  cands.sort((a, b) => MECH_ORDER[a.mech] - MECH_ORDER[b.mech] || a.cost - b.cost);
  const used = new Set(), out = [];
  for (const c of cands) {
    if (c.keys.some((k) => used.has(k))) continue;
    if (c.ctx !== 'word' && rng() > (o.density ?? 1)) continue;
    c.keys.forEach((k) => used.add(k));
    out.push(c);
  }
  return out;
}
const ligateText = ligateGlyphs;
// Rays a ligature consumed, so the thread layer leaves them alone.
function markUsed(glyphs, ligs) {
  for (const l of ligs) for (const k of l.keys) {
    const m = /^(\d+)r(\d+)$/.exec(k);
    if (m) glyphs[+m[1]].rays[+m[2]].used = true;
  }
}
// Prims to draw after ligatures: trimmed arcs and absorbed dots.
function ligatedPrims(G, ligs) {
  const prims = G.prims.slice();
  for (const l of ligs) {
    if (l.trim && l.trim[0] === G) {
      const [, qi, kept] = l.trim, cur = prims[qi];
      prims[qi] = Object.assign({}, cur, { a0: Math.max(cur.a0, kept.a0), a1: Math.min(cur.a1, kept.a1) });
    }
    if (l.absorb && l.absorb[0] === G) prims[l.absorb[1]] = null;
    if (l.drop && l.drop[0] === G) for (const d of [].concat(l.drop[1])) prims[d] = null;
  }
  return prims.filter(Boolean);
}
