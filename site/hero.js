// The hero's scene: a white beam enters a glass prism and leaves as a spectrum, drawn in text.
//
// The scene lives in world units where one character cell is 1 wide and 2 tall. The prism is the
// logo's: an equilateral face, with the back face moved right and showing only where it clears the
// front. The beam meets the middle of the left face; every wavelength refracts by Snell's law with a
// Cauchy index fitted so 550 nm is close to the logo's n = sqrt(2), and leaves the right face on its own
// ray. Each cell is sampled 8 times, so edges are antialiased by coverage, then mapped to a density
// ramp and coloured by wavelength.
//
// In the browser this file draws the scene on a canvas at the size of its box: the beam pulses, motes
// ride the spectrum, stars twinkle; 30 fps at most, paused off screen and in background tabs, a single
// still frame with reduced motion. Under node it exports the scene for site/hero-frame.js, which writes
// the still frame shown without JavaScript.
(function () {
  "use strict";

  const RAMP = " .:-=+*#%@";
  const SQ3 = Math.sqrt(3);
  const NM = [400, 700];
  // Cauchy n(λ) = A + B/λ², λ in µm: n(700) = 1.33, n(400) = 1.60, so n(550) ≈ 1.41, the logo's sqrt(2).
  // Real glass disperses about ten times less; this keeps the order and the shape, wide enough to see.
  const CB = 0.27 / (1 / 0.16 - 1 / 0.49);
  const CA = 1.33 - CB / 0.49;
  const index = (nm) => CA + CB / ((nm / 1000) * (nm / 1000));

  /** The colour of a wavelength (Bruton's approximation), lifted a little toward white for a dark page. */
  function spectral(nm) {
    let r = 0, g = 0, b = 0;
    if (nm < 440) { r = (440 - nm) / 60; b = 1; }
    else if (nm < 490) { g = (nm - 440) / 50; b = 1; }
    else if (nm < 510) { g = 1; b = (510 - nm) / 20; }
    else if (nm < 580) { r = (nm - 510) / 70; g = 1; }
    else if (nm < 645) { r = 1; g = (645 - nm) / 65; }
    else r = 1;
    const fade = nm < 420 ? 0.55 + 0.45 * (nm - 400) / 20 : nm > 680 ? 0.55 + 0.45 * (700 - nm) / 20 : 1;
    return [r, g, b].map((v) => Math.round(255 * (0.2 + 0.8 * Math.pow(v * fade, 0.8))));
  }

  const sub = (a, b) => [a[0] - b[0], a[1] - b[1]];
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1];
  const unit = (v) => { const l = Math.hypot(v[0], v[1]); return [v[0] / l, v[1] / l]; };
  const crossz = (a, b) => a[0] * b[1] - a[1] * b[0];

  /** Refract unit direction d through a surface with unit normal n facing the incoming side; eta = n1/n2. */
  function refract(d, n, eta) {
    const cos = -dot(n, d);
    const k = 1 - eta * eta * (1 - cos * cos);
    if (k < 0) return null;
    const c = eta * cos - Math.sqrt(k);
    return unit([eta * d[0] + c * n[0], eta * d[1] + c * n[1]]);
  }

  /** Where the ray p + s·d crosses the line through a and b, as the distance s. */
  function hit(p, d, a, b) {
    const e = sub(b, a);
    return crossz(sub(a, p), e) / crossz(d, e);
  }

  function segDist(p, a, b) {
    const ab = sub(b, a);
    const t = Math.max(0, Math.min(1, dot(sub(p, a), ab) / dot(ab, ab)));
    return [Math.hypot(p[0] - a[0] - ab[0] * t, p[1] - a[1] - ab[1] * t), t];
  }

  function inside(p, tri) {
    const [a, b, c] = tri;
    const s1 = crossz(sub(b, a), sub(p, a)), s2 = crossz(sub(c, b), sub(p, b)), s3 = crossz(sub(a, c), sub(p, c));
    return (s1 >= 0 && s2 >= 0 && s3 >= 0) || (s1 <= 0 && s2 <= 0 && s3 <= 0);
  }

  function grow(tri, d) {
    const c = [(tri[0][0] + tri[1][0] + tri[2][0]) / 3, (tri[0][1] + tri[1][1] + tri[2][1]) / 3];
    const k = 1 + (2 * d) / Math.hypot(tri[0][0] - c[0], tri[0][1] - c[1]); // an equilateral's inradius is R/2
    return tri.map((p) => [c[0] + (p[0] - c[0]) * k, c[1] + (p[1] - c[1]) * k]);
  }

  /** A fixed pseudo-random number in [0, 1) per integer pair. */
  function hash(x, y) {
    let n = (Math.imul(x, 374761393) + Math.imul(y, 668265263)) >>> 0;
    n = Math.imul(n ^ (n >>> 13), 1274126177) >>> 0;
    return ((n ^ (n >>> 16)) >>> 0) / 4294967296;
  }

  /** The scene for a grid of cols x rows cells: geometry and every cell's still layers. */
  function scene(cols, rows) {
    const steps = sceneSteps(cols, rows);
    let step = steps.next();
    while (!step.done) step = steps.next();
    return step.value;
  }

  /** scene(), one row per step, so the browser can build it in slices between frames. */
  function* sceneSteps(cols, rows) {
    const W = cols, H = rows * 2;
    const side = Math.min((H * 0.66) / (SQ3 / 2), W * (W < 120 ? 0.34 : 0.24));
    const cx = W * (W < 120 ? 0.44 : 0.5), base = H * 0.84, h = (side * SQ3) / 2;
    const A = [cx, base - h], R = [cx + side / 2, base], L = [cx - side / 2, base];
    const front = [A, R, L];
    const depth = side * (40 / 170);
    const back = front.map((p) => [p[0] + depth, p[1]]);
    const shield = grow(front, side * (9 / 170));
    const entry = [(A[0] + L[0]) / 2, (A[1] + L[1]) / 2];
    const din = unit([Math.cos(Math.PI / 12), -Math.sin(Math.PI / 12)]); // 15° up
    const start = [0, entry[1] + entry[0] * Math.tan(Math.PI / 12)];
    const beamLen = Math.hypot(entry[0] - start[0], entry[1] - start[1]);
    // Outward normals of the faces.
    const nLeft = unit([-(A[1] - L[1]), A[0] - L[0]]);
    const nLeftOut = dot(nLeft, sub(L, R)) > 0 ? nLeft : [-nLeft[0], -nLeft[1]];
    const nRight = unit([-(R[1] - A[1]), R[0] - A[0]]);
    const nRightOut = dot(nRight, sub(R, L)) > 0 ? nRight : [-nRight[0], -nRight[1]];
    const rays = [];
    for (let nm = NM[0]; nm <= NM[1]; nm += 5) {
      const n = index(nm);
      const t = refract(din, nLeftOut, 1 / n);
      const x = [entry[0] + t[0] * hit(entry, t, A, R), entry[1] + t[1] * hit(entry, t, A, R)];
      const o = refract(t, [-nRightOut[0], -nRightOut[1]], n);
      rays.push({ nm, t, x, o, inAng: Math.atan2(t[1], t[0]), outAng: Math.atan2(o[1], o[0]) });
    }
    const inAt = (ang) => lookup(rays, "inAng", ang);
    const reach = Math.hypot(W, H);

    const n = cols * rows;
    const layers = {
      edge: new Float32Array(n), back: new Float32Array(n), beam: new Float32Array(n), along: new Float32Array(n),
      glow: new Float32Array(n), fan: new Float32Array(n), nm: new Float32Array(n), dist: new Float32Array(n),
      star: new Float32Array(n), phase: new Float32Array(n),
    };
    const SX = [0.25, 0.75], SY = [0.25, 0.75, 1.25, 1.75];
    const halfEdge = 0.5, halfBeam = 1.15;
    // Only samples near the glass or the beam need the costly tests.
    const glass = [L[0] - 2, A[1] - 2, back[1][0] + 2, base + 2];
    const beamBox = [-1, entry[1] - 3, entry[0] + 2, start[1] + 3];
    const within = (p, b) => p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3];
    for (let row = 0; row < rows; row++) {
      if (row) yield;
      for (let col = 0; col < cols; col++) {
        const i = row * cols + col;
        let edge = 0, bk = 0, beam = 0, fan = 0, nmSum = 0, fanW = 0, dsum = 0;
        for (const ox of SX) for (const oy of SY) {
          const p = [col + ox, row * 2 + oy];
          const near = within(p, glass);
          if (near) {
            const de = Math.min(segDist(p, A, R)[0], segDist(p, R, L)[0], segDist(p, L, A)[0]);
            if (de < halfEdge) edge++;
            if (!inside(p, shield)) {
              const db = Math.min(segDist(p, back[0], back[1])[0], segDist(p, back[1], back[2])[0], segDist(p, back[2], back[0])[0]);
              if (db < halfEdge * 0.9) bk++;
            }
          }
          if (within(p, beamBox) && segDist(p, start, entry)[0] < halfBeam) beam++;
          if (near && inside(p, front)) {
            const ang = Math.atan2(p[1] - entry[1], p[0] - entry[0]);
            const r = inAt(ang);
            if (r) { fan += 0.32 * r.w; nmSum += r.nm * r.w; fanW += r.w; }
          } else if (crossz(sub(R, A), sub(p, A)) < 0 && p[0] > A[0]) {
            const r = onRay(rays, p);
            if (r) {
              const d = r.d;
              const w = r.w * Math.exp(-d / (reach * 0.85));
              fan += w; nmSum += r.nm * w; fanW += w; dsum += d * w;
            }
          }
        }
        const c = [col + 0.5, row * 2 + 1];
        const [dc, along] = segDist(c, start, entry);
        layers.edge[i] = edge / 8;
        layers.back[i] = bk / 8;
        layers.beam[i] = beam / 8;
        layers.along[i] = along * beamLen;
        layers.glow[i] = c[0] < entry[0] ? Math.exp(-((dc / 2.2) ** 2)) : 0;
        layers.fan[i] = fan / 8;
        layers.nm[i] = fanW ? nmSum / fanW : 0;
        layers.dist[i] = fanW ? dsum / fanW : 0;
        const roll = hash(col, row);
        if (roll < 0.012) { layers.star[i] = 0.1 + 0.14 * hash(row, col); layers.phase[i] = roll * 6283; }
      }
    }
    return { cols, rows, layers, rays, reach, beamLen };
  }

  /** The ray whose angle `key` is nearest `ang`, with a weight that softens the spectrum's two edges. */
  function lookup(rays, key, ang) {
    const lo = Math.min(rays[0][key], rays[rays.length - 1][key]);
    const hi = Math.max(rays[0][key], rays[rays.length - 1][key]);
    const pad = (hi - lo) * 0.08;
    if (ang < lo - pad || ang > hi + pad) return null;
    const f = (ang - rays[0][key]) / (rays[rays.length - 1][key] - rays[0][key]);
    const t = Math.max(0, Math.min(1, f));
    const nm = NM[0] + (NM[1] - NM[0]) * t;
    const edge = Math.min(1, (Math.min(ang - (lo - pad), hi + pad - ang)) / (pad * 2));
    return { nm, w: edge * (0.75 + 0.25 * Math.sin(Math.PI * t)) };
  }

  /**
   * The wavelength whose ray out of the right face passes through p, with how far along it p lies and a
   * weight that softens the spectrum's two edges. The signed distance from p to each ray falls steadily
   * from violet to red, so a binary search finds the pair of rays p sits between.
   */
  function onRay(rays, p) {
    const side = (k) => crossz(rays[k].o, sub(p, rays[k].x));
    let lo = 0, hi = rays.length - 1;
    const flo = side(lo), fhi = side(hi);
    const t = (k, f0, f1) => (k + f0 / (f0 - f1)) / (rays.length - 1);
    let frac, w;
    if (flo * fhi > 0) {
      const k = Math.abs(flo) < Math.abs(fhi) ? lo : hi;
      const off = Math.abs(side(k));
      if (off > 1.6) return null;
      frac = k / (rays.length - 1);
      w = 1 - off / 1.6;
    } else {
      while (hi - lo > 1) {
        const mid = (lo + hi) >> 1;
        if (side(mid) * flo > 0) lo = mid; else hi = mid;
      }
      frac = t(lo, side(lo), side(hi));
      w = 1;
    }
    const k = Math.round(frac * (rays.length - 1));
    const d = dot(sub(p, rays[k].x), rays[k].o);
    if (d <= 0) return null;
    return { nm: NM[0] + (NM[1] - NM[0]) * frac, d, w: w * (0.75 + 0.25 * Math.sin(Math.PI * frac)) };
  }

  /** Motes riding the spectrum: a wavelength, a starting distance, and a speed each, from a fixed seed. */
  function motes(sc) {
    const out = [];
    const count = Math.round(sc.cols / 5);
    for (let k = 0; k < count; k++) {
      const nm = NM[0] + (NM[1] - NM[0]) * hash(k, 7);
      const ray = sc.rays[Math.round((nm - NM[0]) / 5)];
      out.push({ nm, ray, r0: hash(k, 11) * sc.reach, speed: 6 + 10 * hash(k, 13) });
    }
    return out;
  }

  /** Every cell at time t (seconds): an intensity in [0, 1] and a colour key (0 none, 1 white, 2 back face, 3 star, nm). */
  function frame(sc, t, out) {
    const { cols, rows, layers: L } = sc;
    const n = cols * rows;
    const level = out.level, key = out.key;
    level.fill(0);
    key.fill(0);
    const wave = (x) => 0.78 + 0.22 * Math.sin(x * 0.22 - t * 2.2);
    for (let i = 0; i < n; i++) {
      const beamPulse = wave(L.along[i]);
      const white = Math.max(L.edge[i] * 0.82, (L.beam[i] * 0.95 + L.glow[i] * 0.14) * beamPulse);
      const back = L.back[i] * 0.5;
      const fan = L.fan[i] * wave(sc.beamLen + L.dist[i]);
      const star = L.star[i] * (0.55 + 0.45 * Math.sin(t * 1.3 + L.phase[i]));
      const best = Math.max(white, back, fan, star);
      if (best < 0.05) continue;
      level[i] = Math.min(1, best);
      key[i] = best === white ? 1 : best === fan ? L.nm[i] : best === back ? 2 : 3;
    }
    for (const m of out.motes || []) {
      const r = (m.r0 + m.speed * t) % sc.reach;
      const p = [m.ray.x[0] + m.ray.o[0] * r, m.ray.x[1] + m.ray.o[1] * r];
      const col = Math.floor(p[0]), row = Math.floor(p[1] / 2);
      if (col < 0 || col >= cols || row < 0 || row >= rows) continue;
      const i = row * cols + col;
      if (L.fan[i] < 0.1) continue;
      level[i] = Math.min(1, level[i] + 0.45);
      key[i] = m.nm;
    }
    return out;
  }

  function buffers(sc) {
    return { level: new Float32Array(sc.cols * sc.rows), key: new Float32Array(sc.cols * sc.rows), motes: motes(sc) };
  }

  const glyph = (level) => RAMP[Math.min(RAMP.length - 1, Math.max(1, Math.round(level * (RAMP.length - 1))))];
  const api = { RAMP, scene, sceneSteps, frame, buffers, glyph, spectral, NM };

  if (typeof module === "object" && module.exports) {
    module.exports = api;
    return;
  }

  // ---- Browser ----
  const art = document.querySelector(".art");
  if (!art) return;
  const reduce = matchMedia("(prefers-reduced-motion: reduce)");
  const canvas = document.createElement("canvas");
  const glow = document.createElement("canvas");
  glow.className = "glow";
  canvas.setAttribute("aria-hidden", "true");
  glow.setAttribute("aria-hidden", "true");
  art.append(glow, canvas);
  const ctx = canvas.getContext("2d");
  const gctx = glow.getContext("2d");
  const FRAME = 1000 / 30;
  const WHITE = [236, 237, 240], BACK = [150, 156, 170], STAR = [120, 126, 140];
  const palette = new Map();
  let sc, buf, cw, ch, fs, img, raf = 0, last = 0, visible = false, building = 0, started = false, measured = [0, 0], t0 = performance.now();

  function rgb(key) {
    if (key === 1) return WHITE;
    if (key === 2) return BACK;
    if (key === 3) return STAR;
    const nm = Math.round(key / 5) * 5;
    if (!palette.has(nm)) palette.set(nm, spectral(nm));
    return palette.get(nm);
  }

  function layout() {
    const box = art.getBoundingClientRect();
    measured = [art.clientWidth, art.clientHeight];
    if (!box.width || !box.height) return;
    fs = box.width >= 900 ? 12 : box.width >= 600 ? 10 : 8;
    const font = `${fs}px ui-monospace, SFMono-Regular, Menlo, Consolas, "DejaVu Sans Mono", "Liberation Mono", monospace`;
    ctx.font = font;
    cw = ctx.measureText("M").width;
    ch = fs * 1.2;
    const cols = Math.floor(box.width / cw), rows = Math.floor(box.height / ch);
    const dpr = Math.min(2, devicePixelRatio || 1);
    canvas.width = Math.round(cols * cw * dpr);
    canvas.height = Math.round(rows * ch * dpr);
    canvas.style.width = `${cols * cw}px`;
    canvas.style.height = `${rows * ch}px`;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.font = font;
    ctx.textBaseline = "top";
    glow.width = cols;
    glow.height = rows;
    glow.style.width = canvas.style.width;
    glow.style.height = canvas.style.height;
    img = gctx.createImageData(cols, rows);
    sc = null;
    const steps = sceneSteps(cols, rows);
    const job = ++building;
    // Build in slices of about 6 ms, so the page never waits on a long task.
    const slice = () => {
      if (job !== building) return;
      const until = performance.now() + 6;
      let step;
      do step = steps.next(); while (!step.done && performance.now() < until);
      if (!step.done) { setTimeout(slice, 0); return; }
      sc = step.value;
      buf = buffers(sc);
      ready();
    };
    slice();
  }

  function draw(t) {
    frame(sc, t, buf);
    const { cols, rows } = sc;
    ctx.clearRect(0, 0, cols * cw, rows * ch);
    const px = img.data;
    for (let row = 0; row < rows; row++) {
      for (let col = 0; col < cols; col++) {
        const i = row * cols + col;
        const level = buf.level[i];
        const o = i * 4;
        if (!level) { px[o + 3] = 0; continue; }
        const c = rgb(buf.key[i]);
        ctx.fillStyle = `rgba(${c[0]},${c[1]},${c[2]},${(0.45 + 0.55 * level).toFixed(2)})`;
        ctx.fillText(glyph(level), col * cw, row * ch);
        px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2];
        px[o + 3] = buf.key[i] === 3 ? 0 : Math.round(255 * level * 0.8);
      }
    }
    gctx.putImageData(img, 0, 0);
  }

  function tick(now) {
    raf = 0;
    if (!sc || !visible || document.hidden || reduce.matches) return;
    // At most 30 fps on average: 2 ms of slack lets a 60 Hz screen draw every other frame, and carrying
    // the remainder (early or late) keeps the long-run rate at exactly 30, not 20 or 30.3.
    const elapsed = now - last;
    if (elapsed >= FRAME - 2) {
      last = elapsed < 2 * FRAME ? now - (elapsed - FRAME) : now;
      draw((now - t0) / 1000);
    }
    raf = requestAnimationFrame(tick);
  }

  function run() {
    if (!sc || reduce.matches) return;
    if (visible && !document.hidden && !raf) raf = requestAnimationFrame(tick);
  }

  /** A scene finished building: show it, and wire the observers the first time. */
  function ready() {
    draw((performance.now() - t0) / 1000);
    art.classList.add("live");
    if (started) { run(); return; }
    started = true;
    new IntersectionObserver(([entry]) => { visible = entry.isIntersecting; run(); }).observe(art);
    document.addEventListener("visibilitychange", run);
    reduce.addEventListener("change", run);
  }

  function start() {
    layout();
    // Rebuild when the band's size differs from the one the current (or pending) scene was built for.
    let pending = 0;
    new ResizeObserver(() => {
      if (art.clientWidth === measured[0] && art.clientHeight === measured[1]) return;
      clearTimeout(pending);
      pending = setTimeout(layout, 120);
    }).observe(art);
  }

  // Draw after the page has painted, so the scene never delays the text.
  if (document.readyState === "complete") start();
  else addEventListener("load", start, { once: true });
})();
