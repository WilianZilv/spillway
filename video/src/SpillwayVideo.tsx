import React from 'react';
import {AbsoluteFill, useCurrentFrame} from 'remotion';
import {C, ELASTIC, F, FPS, HEAVY, POP, TL, W, band, clamp01, easeIn, easeInOut, easeOut, lerp, life, prog, rand, sp} from './lib';
import {Background, Chip, Grain, Label, Line, Logo, Mask, useFonts} from './components/primitives';
import {CameraTile, DiskCard, DiskState, RAM_SLOTS, RamBlock, qb} from './components/parts';

const {s1, s2, s3, s4, s5, s6} = TL;

/** Word reveal values for a line: stagger 0.09 s, 0.8 s each. */
const wordKs = (t: number, start: number, n: number) => Array.from({length: n}, (_, i) => prog(t, start + i * 0.09, start + i * 0.09 + 0.8, easeOut));

const Headline: React.FC<{top: number; words: {w: string; accent?: boolean}[]; ks: number[]; out: number; size?: number}> = ({top, words, ks, out, size = 104}) =>
  out >= 1 || ks.every((k) => k <= 0) ? null : (
    <div style={{position: 'absolute', top, left: 0, right: 0, display: 'flex', justifyContent: 'center'}}>
      <Line words={words} ks={ks} out={out} size={size} />
    </div>
  );

// ---------------------------------------------------------------- geometry

const GRID = {cols: 6, rows: 4, left: 120, top: 470, dx: 144, dy: 106};
const RAM = {cx: 540, y: 500};
const DISK_Y = 1000;
const DISKS = [
  {cx: 220, name: 'disk1', cap: 'LOCAL · 4 TB', share: '57%', use: 0.38, remote: false},
  {cx: 540, name: 'disk2', cap: 'LOCAL · 2 TB', share: '29%', use: 0.46, remote: false},
  {cx: 860, name: 'node-b', cap: 'OUTRO SERVIDOR', share: '14%', use: 0.52, remote: true},
];

/** Destination of the n-th file: smooth weighted round-robin over free space (4:2:1), skipping a failed disk. */
const destOf = (n: number, spawn: number) => {
  const down = spawn >= s4.fail && spawn < s4.recover + 0.2;
  const w = down ? [4, 0, 1] : [4, 2, 1];
  const cur = [0, 0, 0];
  let pick = 0;
  for (let i = 0; i <= n; i++) {
    const total = w[0] + w[1] + w[2];
    for (let d = 0; d < 3; d++) cur[d] += w[d];
    pick = cur.indexOf(Math.max(...cur));
    cur[pick] -= total;
  }
  return pick;
};

/** RAM fill (0..1 of the block) over the whole piece. */
const ramFill = (t: number) => {
  const base = 0.45 * prog(t, s3.fill, s3.streams + 0.6, easeOut) + 0.04 * Math.sin(t * 2.3) * band(t, s3.streams, s3.streams + 1);
  const rise = prog(t, s4.rise, s4.ceiling, easeInOut) * (0.88 - 0.45);
  return base + rise;
};

const EVICT_N = 2;

export const SpillwayVideo: React.FC = () => {
  useFonts();
  const frame = useCurrentFrame();
  const t = frame / FPS;

  // ================================================================ scene 1: cameras
  const s1Out = prog(t, s1.exit, s1.exit + 0.45, easeIn);
  const tiles: React.ReactNode[] = [];
  const s1Chips: React.ReactNode[] = [];
  if (t < s1.exit + 0.6) {
    for (let r = 0; r < GRID.rows; r++) {
      for (let c = 0; c < GRID.cols; c++) {
        const j = r * GRID.cols + c;
        const x = GRID.left + c * GRID.dx;
        const y = GRID.top + r * GRID.dy + s1Out * 90 * (0.6 + rand(j));
        const k = sp(frame, s1.grid - 0.8 + (r + c) * 0.035, POP) * (1 - s1Out);
        tiles.push(<CameraTile key={j} x={x} y={y} k={clamp01(k)} t={t} seed={rand(j + 9)} />);
        for (let m = 0; m < 6; m++) {
          const ts = s1.flow + j * 0.045 + m * 0.5 + rand(j * 7 + m) * 0.2;
          const dt = t - ts;
          if (dt < 0 || dt > 1.4) continue;
          const cy = y + 80 + 160 * dt + 560 * dt * dt;
          const o = (1 - band(cy, 900, 1050)) * band(dt, 0, 0.08) * (1 - s1Out);
          if (o <= 0.01) continue;
          s1Chips.push(<Chip key={`${j}-${m}`} x={x + 59 + (rand(j + m * 3) - 0.5) * 30} y={cy} s={0.7} o={o} tone={m % 3 === 0 ? 'blue' : 'plain'} rot={(rand(j * m + 1) - 0.5) * 30 * dt} />);
        }
      }
    }
  }
  const count = Math.round(1200 * prog(t, s1.counter, s1.counter + 1.6, easeOut));
  const counterK = prog(t, s1.counter - 0.1, s1.counter + 0.5, easeOut) * (1 - s1Out);

  // ================================================================ scene 2: the old stack
  const STACK = [
    {a: 'tmpfs', b: '/ram', c: 'mount manual'},
    {a: 'mergerfs', b: '/videos', c: 'RAM + discos só leitura'},
    {a: 'mover.py', b: '', c: 'varre por mtime'},
  ];

  // ================================================================ scene 3/4: RAM → disks
  const lockIn = sp(frame, s3.logo, ELASTIC);
  const dock = prog(t, s3.line1 - 0.25, s3.line1 + 0.4, easeInOut);
  const heroK = sp(frame, s6.hit, ELASTIC);
  const toHero = prog(t, s6.merge, s6.hit, easeIn);
  const logoX = lerp(lerp(540, 430, dock), 540, toHero);
  const logoY = lerp(lerp(470, 122, dock), 520, toHero);
  const logoSize = lerp(lerp(220, 64, dock), 64, toHero) * (t < s6.hit ? 1 : 1 + 2.6 * heroK);
  const logoVisible = t >= s3.logo;
  const wordK = prog(t, s3.word, s3.word + 0.7, easeOut);
  const wordSize = lerp(130, 58, dock);
  // left edge: centred under the big mark, then beside the docked one (Instrument Serif "spillway" ≈ 3.35 em wide)
  const wordX = lerp(540 - (130 * 3.35) / 2, 478, dock);
  const wordY = lerp(690, 122, dock);
  const wordOut = prog(t, s6.merge, s6.merge + 0.3, easeIn);

  const sceneVis = life(t, s3.ram, 0.9, s4.exit, 0.5);
  const diskK = DISKS.map((_, i) => life(t, s3.disks[i], 0.8, s4.exit + i * 0.05, 0.45));
  const fill = ramFill(t);
  const evict = prog(t, s4.thief + 0.1, s4.thief + 0.6, easeInOut);
  const shift = prog(t, s4.thief + 0.6, s4.thief + 0.9, easeInOut);
  const evicted = t >= s4.thief + 0.9;
  const ramShown = evicted ? fill - EVICT_N / RAM_SLOTS : fill;
  const hot = band(t, s4.rise, s4.ceiling) * (1 - band(t, s4.thief + 0.6, s4.thief + 1.1));
  const markersK = prog(t, s4.q2, s4.q2 + 0.5, easeOut) * (1 - prog(t, s4.exit, s4.exit + 0.3, easeIn));
  const flame = sp(frame, s4.thief, POP) * (1 - prog(t, s4.thief + 1.1, s4.thief + 1.5, easeIn));

  // files arriving into RAM from the left (the cameras)
  const inflow: React.ReactNode[] = [];
  if (t > s3.fill - 0.2 && t < s4.exit + 0.4) {
    // steady arrivals, then a burst while the disks stall (the RAM climbs to the ceiling)
    const spawns: number[] = [];
    for (let ts = s3.fill; ts < s4.rise; ts += 0.2) spawns.push(ts);
    for (let ts = s4.rise; ts < s4.exit; ts += 0.11) spawns.push(ts);
    for (let n = 0; n < spawns.length; n++) {
      const dt = t - spawns[n];
      if (dt < 0 || dt > 0.55) continue;
      const k = easeOut(clamp01(dt / 0.55));
      const x = lerp(-40, 150, k);
      const y = RAM.y + 125 + (rand(n) - 0.5) * 40;
      inflow.push(<Chip key={`in${n}`} x={x} y={y} s={0.8} o={band(dt, 0, 0.1) * (1 - band(k, 0.85, 1)) * sceneVis} tone="blue" />);
    }
  }

  // files leaving RAM for the disks
  const streams: React.ReactNode[] = [];
  const outflowOn = (ts: number) => ts >= s3.streams && (ts < s4.rise || ts > s4.thief + 0.9) && ts < s4.exit;
  for (let n = 0; n < 230; n++) {
    const ts = s3.streams + n * 0.085;
    if (!outflowOn(ts)) continue;
    const dt = t - ts;
    const travel = 0.85;
    if (dt < 0 || dt > travel + 0.1) continue;
    const d = destOf(n, ts);
    const k = easeInOut(clamp01(dt / travel));
    const p0: [number, number] = [RAM.cx + (rand(n * 3) - 0.5) * 300, RAM.y + 215];
    const p2: [number, number] = [DISKS[d].cx + (rand(n * 5) - 0.5) * 90, DISK_Y + 30];
    const p1: [number, number] = [lerp(p0[0], p2[0], 0.5), RAM.y + 340];
    const [x, y] = qb(p0, p1, p2, k);
    // a file that was heading to a disk that just went down is refused
    const refused = d === 1 && ts < s4.fail && ts + travel > s4.fail;
    const rk = refused ? prog(t, s4.fail, s4.fail + 0.25, easeIn) : 0;
    const o = band(dt, 0, 0.08) * (1 - band(k, 0.88, 1)) * (1 - rk) * sceneVis;
    if (o <= 0.01) continue;
    streams.push(<Chip key={`o${n}`} x={x} y={y + rk * 30} s={0.95} o={o} tone={refused && rk > 0 ? 'red' : 'blue'} />);
  }

  const diskState = (i: number): [DiskState, number] => {
    if (i !== 1) return ['ok', 1];
    if (t >= s4.fail && t < s4.recover) return ['fail', prog(t, s4.fail, s4.fail + 0.3, easeOut)];
    if (t >= s4.recover && t < s4.recover + 1.3) return ['back', 1 - prog(t, s4.recover + 0.9, s4.recover + 1.3)];
    return ['ok', 1];
  };
  const shake = Math.sin((t - s4.fail) * 70) * 12 * (1 - prog(t, s4.fail, s4.fail + 0.5, easeOut)) * (t >= s4.fail ? 1 : 0);

  // ================================================================ scene 5: panel
  const panelK = life(t, s5.panel, 1.0, s5.exit, 0.5);
  const chartK = prog(t, s5.panel + 0.4, s5.panel + 2.2, easeInOut);
  const statsK = s5.stats.map((s) => sp(frame, s, POP) * (1 - prog(t, s5.exit + 0.05, s5.exit + 0.45, easeIn)));

  // ================================================================ scene 6: hero
  const merge = prog(t, s6.merge, s6.hit, easeIn);
  const ring = prog(t, s6.hit, s6.hit + 0.9, easeOut);
  const flash = band(t, s6.hit - 0.05, s6.hit) * (1 - band(t, s6.hit, s6.hit + 0.6));

  const glow = t < s3.logo ? 0.25 : t < s6.merge ? 0.35 + 0.25 * hot : 0.4 + 0.6 * flash;

  return (
    <AbsoluteFill style={{background: C.bg, overflow: 'hidden'}}>
      <Background t={t} glow={glow} glowY={t > s6.merge ? 40 : 52} hue={hot > 0.3 ? '229,83,75' : '61,139,242'} />

      {/* ---------- scene 1 ---------- */}
      <Headline top={150} words={[{w: 'Centenas'}, {w: 'de'}, {w: 'câmeras.'}]} ks={wordKs(t, s1.line1, 3)} out={prog(t, s1.exit, s1.exit + 0.4, easeIn)} size={96} />
      <Headline
        top={270}
        words={[{w: 'gravando'}, {w: 'ao', accent: true}, {w: 'mesmo', accent: true}, {w: 'tempo.', accent: true}]}
        ks={wordKs(t, s1.line2, 4)}
        out={prog(t, s1.exit + 0.05, s1.exit + 0.45, easeIn)}
        size={96}
      />
      {s1Chips}
      {tiles}
      {counterK > 0.01 && (
        <div style={{position: 'absolute', top: 1060, left: 0, right: 0, textAlign: 'center', opacity: counterK, transform: `translateY(${(1 - counterK) * 20}px)`}}>
          <div style={{fontFamily: F.serif, fontSize: 150, lineHeight: 1, color: C.ink, letterSpacing: '-0.02em', fontVariantNumeric: 'tabular-nums'}}>{count.toLocaleString('pt-BR')}</div>
          <Label style={{marginTop: 10}}>arquivos por minuto</Label>
        </div>
      )}

      {/* ---------- scene 2 ---------- */}
      <Headline top={150} words={[{w: 'Até'}, {w: 'hoje,'}]} ks={wordKs(t, s2.line, 2)} out={prog(t, s2.exit, s2.exit + 0.4, easeIn)} />
      <Headline top={270} words={[{w: 'uma'}, {w: 'gambiarra.', accent: true}]} ks={wordKs(t, s2.line + 0.4, 2)} out={prog(t, s2.exit + 0.05, s2.exit + 0.45, easeIn)} />
      {t > s2.stack[0] - 0.1 &&
        t < s2.exit + 1 &&
        STACK.map((b, i) => {
          const k = clamp01(sp(frame, s2.stack[i], HEAVY));
          const fall = prog(t, s2.exit + i * 0.06, s2.exit + 0.5 + i * 0.06, easeIn);
          const strike = prog(t, s2.strike + i * 0.12, s2.strike + i * 0.12 + 0.4, easeOut);
          const rot = (rand(i + 3) - 0.5) * 3.5 + Math.sin(t * 1.6 + i * 2) * 0.7 + fall * (i - 1) * 18;
          return (
            <div
              key={i}
              style={{
                position: 'absolute',
                left: 160,
                top: 540 + i * 140 + (1 - k) * 80 + fall * 500,
                width: 760,
                height: 108,
                borderRadius: 20,
                background: C.panel,
                border: `2px solid ${C.lineHi}`,
                opacity: k * (1 - fall),
                transform: `rotate(${rot}deg)`,
                display: 'flex',
                alignItems: 'center',
                padding: '0 34px',
                boxSizing: 'border-box',
                gap: 18,
              }}
            >
              <span style={{fontFamily: F.mono, fontSize: 32, color: C.ink, fontWeight: 600}}>{b.a}</span>
              <span style={{fontFamily: F.mono, fontSize: 26, color: C.dim}}>{b.b}</span>
              <span style={{flex: 1}} />
              <span style={{fontFamily: F.sans, fontSize: 24, color: C.muted}}>{b.c}</span>
              <div style={{position: 'absolute', left: 24, top: 50, height: 7, width: 712 * strike, borderRadius: 4, background: C.blue}} />
            </div>
          );
        })}

      {/* ---------- scene 3/4 ---------- */}
      <Headline top={210} words={[{w: 'Escreve'}, {w: 'na'}, {w: 'RAM.', accent: true}]} ks={wordKs(t, s3.line1, 3)} out={prog(t, s3.line2 - 0.25, s3.line2 + 0.1, easeIn)} size={96} />
      <Headline
        top={210}
        words={[{w: 'Move'}, {w: 'pro'}, {w: 'disco'}, {w: 'certo.', accent: true}]}
        ks={wordKs(t, s3.line2, 4)}
        out={prog(t, s3.exit, s3.exit + 0.35, easeIn)}
        size={96}
      />
      <Headline top={190} words={[{w: 'Disco'}, {w: 'caiu?'}]} ks={wordKs(t, s4.q1, 2)} out={prog(t, s4.q2 - 0.3, s4.q2, easeIn)} size={92} />
      <Headline top={300} words={[{w: 'O'}, {w: 'fluxo'}, {w: 'desvia.', accent: true}]} ks={wordKs(t, s4.a1, 3)} out={prog(t, s4.q2 - 0.25, s4.q2 + 0.05, easeIn)} size={92} />
      <Headline top={190} words={[{w: 'RAM'}, {w: 'no'}, {w: 'limite?'}]} ks={wordKs(t, s4.q2, 3)} out={prog(t, s4.exit, s4.exit + 0.35, easeIn)} size={92} />
      <Headline top={300} words={[{w: 'O'}, {w: 'thief'}, {w: 'protege.', accent: true}]} ks={wordKs(t, s4.a2, 3)} out={prog(t, s4.exit + 0.05, s4.exit + 0.4, easeIn)} size={92} />

      {sceneVis > 0.01 && (
        <RamBlock
          cx={RAM.cx}
          y={RAM.y + 40}
          k={sceneVis}
          fill={ramShown}
          ceiling={0.85}
          resume={0.7}
          markersK={markersK}
          hot={hot}
          evict={evicted ? 0 : evict}
          evictN={evicted ? 0 : EVICT_N}
          shift={evicted ? 0 : shift}
          flame={flame}
        />
      )}
      {inflow}
      {streams}
      {DISKS.map((d, i) => {
        const [st, stK] = diskState(i);
        return diskK[i] > 0.01 ? (
          <DiskCard
            key={d.name}
            cx={d.cx}
            y={DISK_Y}
            k={diskK[i]}
            name={d.name}
            cap={d.cap}
            use={d.use + 0.05 * prog(t, s3.streams, s4.exit)}
            state={st}
            stateK={stK}
            share={st === 'fail' ? '0%' : d.share}
            shareK={prog(t, s3.shares + i * 0.12, s3.shares + i * 0.12 + 0.5, easeOut)}
            shake={i === 1 ? shake : 0}
            remote={d.remote}
          />
        ) : null;
      })}

      {/* ---------- scene 5 ---------- */}
      <Headline top={200} words={[{w: 'Painel'}, {w: 'ao', accent: true}, {w: 'vivo.', accent: true}]} ks={wordKs(t, s5.line, 3)} out={prog(t, s5.exit, s5.exit + 0.35, easeIn)} size={96} />
      {panelK > 0.01 && <Panel k={panelK} chartK={chartK} t={t} />}
      {[
        {v: '156 mil', l: 'arquivos / min'},
        {v: '13/13', l: 'cenários de falha'},
        {v: '0', l: 'perdidos nas falhas'},
      ].map((s, i) =>
        statsK[i] > 0.01 ? (
          <div key={i} style={{position: 'absolute', top: 1010 + (1 - statsK[i]) * 30, left: 70 + i * 320, width: 300, textAlign: 'center', opacity: clamp01(statsK[i])}}>
            <div style={{fontFamily: F.serif, fontSize: 92, lineHeight: 1, color: i === 2 ? C.blue : C.ink, fontStyle: i === 2 ? 'italic' : 'normal'}}>{s.v}</div>
            <Label style={{marginTop: 12, fontSize: 18}}>{s.l}</Label>
          </div>
        ) : null,
      )}

      {/* ---------- scene 6 ---------- */}
      {merge > 0 &&
        merge < 1 &&
        Array.from({length: 14}, (_, i) => {
          const a = (i / 14) * Math.PI * 2 + 0.3;
          const r = lerp(600, 30, merge);
          return <Chip key={`m${i}`} x={540 + Math.cos(a) * r} y={520 + Math.sin(a) * r} s={lerp(1, 0.4, merge)} o={band(merge, 0, 0.15)} tone={i % 2 ? 'blue' : 'plain'} rot={merge * 180} />;
        })}
      {ring > 0 && ring < 1 && (
        <div
          style={{
            position: 'absolute',
            left: 540 - 700 * ring,
            top: 520 - 700 * ring,
            width: 1400 * ring,
            height: 1400 * ring,
            borderRadius: '50%',
            border: `${3 + 6 * (1 - ring)}px solid rgba(61,139,242,${0.6 * (1 - ring)})`,
          }}
        />
      )}

      {/* the mark: hero of scene 3, docked through 3–5, back as the finale */}
      {logoVisible && (
        <div style={{position: 'absolute', left: logoX - logoSize / 2, top: logoY - logoSize / 2, transform: `scale(${t < s6.hit ? clamp01(lockIn) : 1})`}}>
          <Logo size={logoSize} k={prog(t, s3.logo + 0.1, s3.logo + 0.8, easeOut)} glow={0.25 + flash} />
        </div>
      )}
      {wordK > 0 && wordOut < 1 && (
        <div style={{position: 'absolute', left: wordX, top: wordY - wordSize * 0.62, whiteSpace: 'nowrap'}}>
          <span style={{fontFamily: F.serif, fontSize: wordSize, lineHeight: 1, color: C.ink, letterSpacing: '-0.02em'}}>
            <Mask k={wordK} out={wordOut}>
              spillway
            </Mask>
          </span>
        </div>
      )}
      <Headline top={760} words={[{w: 'spillway'}]} ks={[prog(t, s6.word, s6.word + 0.8, easeOut)]} out={0} size={150} />
      <Headline top={950} words={[{w: 'RAM'}, {w: 'primeiro.'}, {w: 'Disco'}, {w: 'certo.'}]} ks={wordKs(t, s6.tag, 4)} out={0} size={64} />
      <Headline top={1035} words={[{w: 'Nada'}, {w: 'se'}, {w: 'perde.', accent: true}]} ks={wordKs(t, s6.tag + 0.4, 3)} out={0} size={64} />
      <div style={{position: 'absolute', top: 1190, left: 0, right: 0, textAlign: 'center', opacity: prog(t, s6.footer, s6.footer + 0.6, easeOut)}}>
        <Label>Rust · FUSE · master / nodes</Label>
      </div>

      {flash > 0 && <AbsoluteFill style={{background: `rgba(125,178,255,${0.18 * flash})`}} />}
      <Grain frame={frame} />
    </AbsoluteFill>
  );
};

// ---------------------------------------------------------------- the panel mock

const series = (seed: number, n: number, base: number, amp: number) =>
  Array.from({length: n}, (_, i) => base + amp * (0.55 * Math.sin(i * 0.33 + seed) + 0.3 * Math.sin(i * 0.11 + seed * 2) + 0.25 * (rand(i + seed * 50) - 0.5)));

const ING = series(1, 60, 0.6, 0.3);
const EGR = series(4, 60, 0.52, 0.28);

const Panel: React.FC<{k: number; chartK: number; t: number}> = ({k, chartK, t}) => {
  const w = 920;
  const left = (W - w) / 2;
  const roll = (v: number, from: number) => Math.round(v * prog(t, from, from + 1.2, easeOut));
  const path = (vals: number[]) => vals.map((v, i) => `${i ? 'L' : 'M'}${(i / (vals.length - 1)) * 820},${200 - v * 180}`).join(' ');
  const L = 2200;
  return (
    <div
      style={{
        position: 'absolute',
        left,
        top: 330 + (1 - k) * 60,
        width: w,
        height: 600,
        borderRadius: 28,
        background: C.panel,
        border: `2px solid ${C.lineHi}`,
        opacity: k,
        padding: 40,
        boxSizing: 'border-box',
        boxShadow: '0 30px 80px rgba(0,0,0,0.45)',
      }}
    >
      <div style={{display: 'flex', alignItems: 'center', gap: 14}}>
        <Logo size={34} />
        <span style={{fontFamily: F.sans, fontWeight: 600, fontSize: 26, color: C.ink}}>Visão geral</span>
        <span style={{flex: 1}} />
        <span style={{width: 12, height: 12, borderRadius: 6, background: C.green, boxShadow: `0 0 0 5px rgba(47,179,107,0.25)`}} />
        <span style={{fontFamily: F.mono, fontSize: 18, color: C.dim, letterSpacing: '0.12em'}}>AO VIVO</span>
      </div>
      <div style={{display: 'flex', gap: 20, marginTop: 28}}>
        {[
          {l: 'RAM EM USO', v: `${roll(418, 21.0) / 10}`.replace('.', ','), u: 'GiB'},
          {l: 'ENTRADA', v: `${roll(412, 21.1)}`, u: 'MiB/s'},
          {l: 'SAÍDA', v: `${roll(398, 21.2)}`, u: 'MiB/s'},
        ].map((s) => (
          <div key={s.l} style={{flex: 1, borderRadius: 18, background: C.panelHi, padding: '18px 22px', border: `1.5px solid ${C.line}`}}>
            <div style={{fontFamily: F.mono, fontSize: 15, letterSpacing: '0.16em', color: C.muted}}>{s.l}</div>
            <div style={{fontFamily: F.sans, fontWeight: 600, fontSize: 44, color: C.ink, marginTop: 6, fontVariantNumeric: 'tabular-nums'}}>
              {s.v}
              <span style={{fontSize: 20, color: C.dim, fontWeight: 500, marginLeft: 6}}>{s.u}</span>
            </div>
          </div>
        ))}
      </div>
      <svg width={820} height={230} style={{marginTop: 34, overflow: 'visible'}}>
        {[0, 1, 2, 3].map((i) => (
          <line key={i} x1={0} x2={820} y1={20 + i * 60} y2={20 + i * 60} stroke={C.line} strokeWidth={1} />
        ))}
        <path d={`${path(ING)} L820,200 L0,200 Z`} fill="rgba(61,139,242,0.10)" opacity={chartK} />
        <path d={path(EGR)} fill="none" stroke={C.dim} strokeWidth={3} strokeLinejoin="round" strokeDasharray={L} strokeDashoffset={L * (1 - chartK)} />
        <path d={path(ING)} fill="none" stroke={C.blue} strokeWidth={3.5} strokeLinejoin="round" strokeDasharray={L} strokeDashoffset={L * (1 - chartK)} />
      </svg>
      <div style={{display: 'flex', gap: 24, marginTop: 14, fontFamily: F.mono, fontSize: 16, letterSpacing: '0.12em', color: C.muted}}>
        <span style={{display: 'inline-flex', alignItems: 'center', gap: 8}}>
          <i style={{width: 18, height: 4, borderRadius: 2, background: C.blue, display: 'inline-block'}} />
          ENTRADA
        </span>
        <span style={{display: 'inline-flex', alignItems: 'center', gap: 8}}>
          <i style={{width: 18, height: 4, borderRadius: 2, background: C.dim, display: 'inline-block'}} />
          SAÍDA PARA OS DISCOS
        </span>
      </div>
    </div>
  );
};
