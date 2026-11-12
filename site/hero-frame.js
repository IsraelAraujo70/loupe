#!/usr/bin/env node
// Writes the hero's still frame from hero.js, for pages without JavaScript:
//   site/hero-frame.html  the scene at t = 0 on a 160 x 32 grid, as text in coloured spans
//   site/hero.css         the colours of those spans (build.py appends it to style.css)
// Usage: node site/hero-frame.js [--check]   (--check writes nothing and fails if a file is stale)
"use strict";
const fs = require("fs");
const path = require("path");
const hero = require("./hero.js");

const COLS = 160;
const ROWS = 32;
const STEP = 25; // one colour class per 25 nm

const nmClass = (nm) => `h${Math.round(nm / STEP) * STEP}`;

function frameHtml() {
  const sc = hero.scene(COLS, ROWS);
  const buf = hero.buffers(sc);
  hero.frame(sc, 0, buf);
  const lines = [];
  for (let row = 0; row < ROWS; row++) {
    let line = "", run = "", cls = null;
    const flush = () => {
      if (run) line += cls ? `<span class="${cls}">${run}</span>` : run;
      run = "";
    };
    for (let col = 0; col < COLS; col++) {
      const i = row * COLS + col;
      const level = buf.level[i];
      const key = buf.key[i];
      let c = null, ch = " ";
      if (level) {
        ch = hero.glyph(level);
        c = key === 1 ? "w" : key === 2 ? "b" : key === 3 ? "s" : nmClass(key);
        if (level < 0.35 && key > 3) c += " f";
      }
      if (c !== cls && ch !== " ") { flush(); cls = c; }
      run += ch;
    }
    flush();
    lines.push(line.replace(/\s+$/, ""));
  }
  return lines.join("\n") + "\n";
}

function css() {
  const rules = [
    ".art .w{color:#ecedf0}",
    ".art .b{color:#969caa}",
    ".art .s{color:#787e8c}",
    ".art .f{opacity:.6}",
  ];
  for (let nm = hero.NM[0]; nm <= hero.NM[1]; nm += STEP) {
    const [r, g, b] = hero.spectral(nm);
    rules.push(`.art .${nmClass(nm)}{color:rgb(${r},${g},${b})}`);
  }
  return "/* Written by site/hero-frame.js: the colours of the hero's still frame. */\n" + rules.join("\n") + "\n";
}

const files = {
  [path.join(__dirname, "hero-frame.html")]: frameHtml(),
  [path.join(__dirname, "hero.css")]: css(),
};

if (process.argv[2] === "--check") {
  const stale = Object.entries(files).filter(([p, text]) => !fs.existsSync(p) || fs.readFileSync(p, "utf8") !== text);
  if (stale.length) {
    console.error("out of date, run node site/hero-frame.js: " + stale.map(([p]) => path.relative(process.cwd(), p)).join(", "));
    process.exit(1);
  }
  console.log("hero frame is up to date");
} else {
  for (const [p, text] of Object.entries(files)) {
    fs.writeFileSync(p, text);
    console.log("wrote " + path.relative(process.cwd(), p));
  }
}
