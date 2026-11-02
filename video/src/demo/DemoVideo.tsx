import React from 'react';
import {AbsoluteFill, OffthreadVideo, Sequence, staticFile, useCurrentFrame} from 'remotion';
import {C, F, POP, band, clamp01, easeIn, easeInOut, easeOut, lerp, prog, sp} from '../lib';
import {Background, Grain, Label, Line, Logo, Mask, useFonts} from '../components/primitives';
import TL from '../../demo-timeline.json';
import TERM from '../../public/term.json';

export const DW = 1920;
export const DH = 1080;
export const DFPS = TL.fps;

// ------------------------------------------------------------------ layout
/** The panel "screen": a browser-ish frame holding the 1920×1080 footage. */
const SCR = {x: 216, y: 196, w: 1488, h: 837, bar: 36};
const K = SCR.w / 1920; // footage px → screen px at zoom 1

type Cut = {clip: string; from: number; to: number; rate?: number; at: number};
type Move = {at: number; dur: number; cx: number; cy: number; s: number};
type Callout = {from: number; to: number; box: [number, number, number, number]; text: string; tone?: 'blue' | 'red' | 'amber' | 'green'; side?: 'top' | 'bottom' | 'left' | 'right'};
type Head = {from: number; to: number; words: {w: string; accent?: boolean}[]};

/** Lays cuts back to back starting at `t0` (scene-relative seconds). */
const lay = (t0: number, cuts: Omit<Cut, 'at'>[]): Cut[] => {
  let at = t0;
  return cuts.map((c) => {
    const cut = {...c, at};
    at += (c.to - c.from) / (c.rate ?? 1);
    return cut;
  });
};
const cutEnd = (c: Cut) => c.at + (c.to - c.from) / (c.rate ?? 1);

/** Words of a headline; `*a span*` marks the accent (italic blue) words. */
const W = (s: string) => {
  let on = false;
  return s.split(' ').map((w) => {
    if (w.startsWith('*')) {
      on = true;
      w = w.slice(1);
    }
    const accent = on;
    if (w.endsWith('*')) {
      on = false;
      w = w.slice(0, -1);
    }
    return accent ? {w, accent} : {w};
  });
};

// ------------------------------------------------------------------ scenes
type Scene = {key: keyof typeof TL; n: string; label: string; cuts: Cut[]; moves: Move[]; callouts: Callout[]; heads: Head[]; chip?: (t: number) => React.ReactNode};

const b = TL.burst;
const f = TL.fail;
const k = TL.cluster;
const th = TL.thief;
const st = TL.settings;

const burstRate = (() => {
  const m = /(\d+) files in ([\d.]+)s/.exec(TERM.burst);
  return m ? {n: +m[1], s: +m[2]} : {n: 3000, s: 8};
})();
const perMin = Math.round(((burstRate.n / burstRate.s) * 60) / 100) / 10; // thousands per minute

const SCENES: Scene[] = [
  {
    key: 'burst',
    n: '02',
    label: 'Rajada',
    cuts: lay(0.2, [{clip: 'overview', from: 0.5, to: 12.3}]),
    moves: [
      {at: 2.0, dur: 0.9, cx: 525, cy: 168, s: 1.85},
      {at: 6.1, dur: 0.9, cx: 960, cy: 380, s: 1.12},
      {at: 9.9, dur: 0.9, cx: 960, cy: 540, s: 1},
    ],
    callouts: [
      {from: b.c1, to: 5.9, box: [389, 90, 661, 247], text: 'entrada na RAM', side: 'bottom'},
      {from: 4.0, to: 5.9, box: [100, 90, 372, 247], text: 'RAM absorve a rajada', side: 'bottom'},
      {from: b.c2, to: 9.7, box: [1532, 320, 1818, 432], text: 'gravações já nos discos', side: 'bottom'},
    ],
    heads: [{from: 0, to: 99, words: W(`${burstRate.n.toLocaleString('pt-BR')} gravações em *${Math.round(burstRate.s)} segundos.*`)}],
    chip: (t) => (
      <Stat k={band(t, b.burst + 0.4, b.burst + 1.0) * (1 - band(t, 11.2, 11.7))} big={`${perMin.toLocaleString('pt-BR')} mil`} small="arquivos MP4 por minuto" />
    ),
  },
  {
    key: 'fail',
    n: '03',
    label: 'Falha de disco',
    cuts: lay(0.2, [
      {clip: 'failover', from: 0.5, to: 8.5, rate: 4},
      {clip: 'failover', from: 8.5, to: 14.0},
      {clip: 'failover', from: 14.0, to: 21.5, rate: 4},
      {clip: 'failover', from: 21.5, to: 24.5},
    ]),
    moves: [
      {at: 2.3, dur: 0.8, cx: 1394, cy: 400, s: 1.8},
      {at: 5.5, dur: 0.8, cx: 420, cy: 330, s: 1.85},
      {at: 7.6, dur: 0.6, cx: 960, cy: 540, s: 1},
      {at: 10.6, dur: 0.7, cx: 1394, cy: 300, s: 1.75},
    ],
    callouts: [
      {from: f.down + 0.5, to: 5.4, box: [989, 250, 1800, 285], text: 'disco virou só leitura', tone: 'red', side: 'bottom'},
      {from: f.reroute + 0.6, to: 7.5, box: [110, 300, 330, 345], text: 'disk1 assume 80% do tráfego', side: 'bottom'},
      {from: f.back + 0.3, to: 12.6, box: [1686, 198, 1802, 228], text: 'voltou sozinho', tone: 'green', side: 'bottom'},
    ],
    heads: [
      {from: 0, to: 5.4, words: W('Disco virou *só leitura.*')},
      {from: 5.4, to: 9.6, words: W('O tráfego *desvia sozinho.*')},
      {from: 9.6, to: 99, words: W('Consertou? *Volta sozinho.*')},
    ],
  },
  {
    key: 'cluster',
    n: '04',
    label: 'Cluster',
    cuts: lay(0.2, [
      {clip: 'cluster', from: 3.8, to: 9.0},
      {clip: 'cluster', from: 9.0, to: 14.0, rate: 4},
      {clip: 'cluster', from: 14.4, to: 20.0},
    ]),
    moves: [
      {at: 0.0, dur: 0.01, cx: 960, cy: 500, s: 1.1},
      {at: 2.2, dur: 0.8, cx: 700, cy: 300, s: 1.5},
      {at: 6.4, dur: 0.5, cx: 960, cy: 540, s: 1},
      {at: 7.6, dur: 0.9, cx: 525, cy: 800, s: 1.7},
    ],
    callouts: [
      {from: 0.5, to: 2.2, box: [120, 482, 1800, 636], text: 'o mesmo binário, no outro servidor', side: 'top'},
      {from: k.join + 0.6, to: 5.6, box: [120, 290, 1340, 333], text: 'node-b entrou no cluster', tone: 'green', side: 'bottom'},
      {from: 8.6, to: 12.1, box: [100, 640, 950, 1010], text: 'disco do outro servidor recebendo', tone: 'green', side: 'top'},
    ],
    heads: [{from: 0, to: 99, words: W('Outro servidor? *Um comando.*')}],
  },
  {
    key: 'hooks',
    n: '05',
    label: 'Hooks',
    cuts: lay(0.2, [
      {clip: 'hooks', from: 1.0, to: 5.0},
      {clip: 'files', from: 4.6, to: 9.0},
      {clip: 'rec', from: 5.0, to: 9.5},
    ]),
    moves: [
      {at: 0.0, dur: 0.01, cx: 960, cy: 320, s: 1.07},
      {at: 4.15, dur: 0.01, cx: 1150, cy: 700, s: 1.5},
      {at: 8.55, dur: 0.01, cx: 1300, cy: 640, s: 1.6},
      {at: 10.4, dur: 0.9, cx: 960, cy: 780, s: 1.3},
    ],
    callouts: [
      {from: 0.6, to: 4.0, box: [100, 105, 950, 302], text: 'validate: mover, recusar ou segurar', side: 'bottom'},
      {from: 1.6, to: 4.0, box: [968, 105, 1820, 302], text: 'moved: outbox em disco', side: 'bottom'},
    ],
    heads: [
      {from: 0, to: 8.6, words: W('Suas regras, *num hook.*')},
      {from: 8.6, to: 99, words: W('Agrupado *por label.*')},
    ],
    chip: (t) => <Pill k={band(t, 4.6, 5.0) * (1 - band(t, 8.2, 8.5))} text="recusado pelo hook · motivo · labels" tone="red" />,
  },
  {
    key: 'thief',
    n: '06',
    label: 'Proteção',
    cuts: lay(0.2, [
      {clip: 'thief', from: 1.0, to: 3.5},
      {clip: 'thief', from: 3.5, to: 10.0, rate: 2.5},
      {clip: 'thief', from: 10.0, to: 14.5},
      {clip: 'thief', from: 14.5, to: 19.5, rate: 4},
      {clip: 'thief', from: 19.5, to: 22.5, rate: 1.5},
    ]),
    moves: [
      {at: 1.0, dur: 0.7, cx: 700, cy: 230, s: 1.45},
      {at: 5.2, dur: 0.8, cx: 1000, cy: 262, s: 1.3},
      {at: 10.9, dur: 0.6, cx: 960, cy: 400, s: 1.1},
    ],
    callouts: [
      {from: th.banner + 0.4, to: 3.4, box: [100, 92, 1820, 162], text: 'todos os destinos fora', tone: 'red', side: 'bottom'},
      {from: 3.0, to: 6.2, box: [100, 182, 372, 339], text: 'RAM subindo até o teto', tone: 'amber', side: 'bottom'},
      {from: th.thief + 0.3, to: 10.0, box: [1258, 182, 1530, 339], text: 'thief descarta os mais antigos', tone: 'amber', side: 'bottom'},
      {from: th.drain + 0.3, to: 13.1, box: [679, 90, 950, 247], text: 'discos de volta: a RAM esvazia', tone: 'green', side: 'bottom'},
    ],
    heads: [
      {from: 0, to: 6.0, words: W('Discos fora? *A RAM segura.*')},
      {from: 6.0, to: 10.8, words: W('No teto, *o thief decide.*')},
      {from: 10.8, to: 99, words: W('Voltaram? *Esvazia sozinho.*')},
    ],
  },
  {
    key: 'settings',
    n: '07',
    label: 'Ao vivo',
    cuts: lay(0.2, [{clip: 'settings', from: 0.6, to: 7.6}]),
    moves: [
      {at: 0.0, dur: 0.01, cx: 700, cy: 300, s: 1.45},
      {at: 3.6, dur: 0.8, cx: 1450, cy: 230, s: 1.55},
      {at: 5.4, dur: 0.8, cx: 800, cy: 320, s: 1.35},
    ],
    callouts: [],
    heads: [{from: 0, to: 99, words: W('Teto de RAM? *Arrasta e salva.*')}],
    chip: (t) => <ResizeChip k={band(t, st.save + 0.7, st.save + 1.2)} />,
  },
];

// ------------------------------------------------------------------ pieces

const camera = (t: number, moves: Move[]) => {
  let cx = 960;
  let cy = 540;
  let s = 1;
  for (const m of moves) {
    const q = prog(t, m.at, m.at + m.dur, easeInOut);
    cx = lerp(cx, m.cx, q);
    cy = lerp(cy, m.cy, q);
    s = lerp(s, m.s, q);
  }
  const hw = 960 / s;
  const hh = 540 / s;
  return {cx: Math.min(1920 - hw, Math.max(hw, cx)), cy: Math.min(1080 - hh, Math.max(hh, cy)), s};
};

const TONE = {blue: C.blue, red: C.red, amber: '#F2A93D', green: C.green};

const Stat: React.FC<{k: number; big: string; small: string}> = ({k, big, small}) =>
  k <= 0.01 ? null : (
    <div style={{position: 'absolute', right: SCR.x, top: 34, textAlign: 'right', opacity: k, transform: `translateY(${(1 - k) * 14}px)`}}>
      <div style={{fontFamily: F.serif, fontSize: 70, lineHeight: 1, color: C.ink, letterSpacing: '-0.02em'}}>{big}</div>
      <Label style={{fontSize: 16, marginTop: 6}}>{small}</Label>
    </div>
  );

const Pill: React.FC<{k: number; text: string; tone?: keyof typeof TONE}> = ({k, text, tone = 'blue'}) =>
  k <= 0.01 ? null : (
    <div style={{position: 'absolute', right: SCR.x, top: 100, opacity: k, transform: `translateY(${(1 - k) * 14}px)`}}>
      <span style={{fontFamily: F.mono, fontSize: 19, letterSpacing: '0.08em', color: '#fff', background: TONE[tone], padding: '9px 16px', borderRadius: 10}}>{text}</span>
    </div>
  );

const ResizeChip: React.FC<{k: number}> = ({k}) => {
  if (k <= 0.01) return null;
  const size = (line: string) => line.split(/\s+/)[1] ?? '';
  return (
    <div style={{position: 'absolute', right: SCR.x, top: 22, opacity: k, transform: `translateY(${(1 - k) * 14}px)`, textAlign: 'right'}}>
      <div style={{fontFamily: F.mono, fontSize: 18, color: C.muted, letterSpacing: '0.1em'}}>$ df -h /ram</div>
      <div style={{fontFamily: F.serif, fontSize: 62, lineHeight: 1.1, color: C.ink}}>
        {size(TERM.ram_before).replace('.', ',')} <span style={{color: C.muted}}>→</span> <span style={{color: C.blue, fontStyle: 'italic'}}>{size(TERM.ram_after).replace('.', ',')}</span>
      </div>
      <Label style={{fontSize: 15}}>tmpfs redimensionado na hora</Label>
    </div>
  );
};

/** A callout box drawn over the footage, following the camera. */
const CalloutView: React.FC<{c: Callout; t: number; cam: {cx: number; cy: number; s: number}}> = ({c, t, cam}) => {
  const k = prog(t, c.from, c.from + 0.35, easeOut) * (1 - prog(t, c.to - 0.25, c.to, easeIn));
  if (k <= 0.01) return null;
  const map = (x: number, y: number) => [(x - cam.cx) * cam.s * K + SCR.w / 2, (y - cam.cy) * cam.s * K + SCR.h / 2];
  const pad = 8;
  const [x0, y0] = map(c.box[0], c.box[1]);
  const [x1, y1] = map(c.box[2], c.box[3]);
  const color = TONE[c.tone ?? 'blue'];
  const grow = lerp(14, 0, k);
  const side = c.side ?? 'bottom';
  const lx = Math.min(Math.max((x0 + x1) / 2, 160), SCR.w - 160);
  return (
    <>
      <div
        style={{
          position: 'absolute',
          left: x0 - pad - grow,
          top: y0 - pad - grow,
          width: x1 - x0 + 2 * (pad + grow),
          height: y1 - y0 + 2 * (pad + grow),
          border: `3px solid ${color}`,
          borderRadius: 14,
          opacity: k,
          boxShadow: `0 0 0 6px ${color}22, 0 0 40px ${color}55`,
        }}
      />
      <div
        style={{
          position: 'absolute',
          left: lx,
          top: side === 'top' ? y0 - pad - 54 : y1 + pad + 14,
          transform: `translateX(-50%) translateY(${(1 - k) * (side === 'top' ? 10 : -10)}px)`,
          opacity: k,
          whiteSpace: 'nowrap',
          fontFamily: F.sans,
          fontWeight: 600,
          fontSize: 22,
          color: '#fff',
          background: color,
          padding: '8px 16px',
          borderRadius: 10,
          boxShadow: '0 10px 30px rgba(0,0,0,0.45)',
        }}
      >
        {c.text}
      </div>
    </>
  );
};

const FastForward: React.FC<{rate: number; k: number}> = ({rate, k}) =>
  k <= 0.01 ? null : (
    <div
      style={{
        position: 'absolute',
        right: 18,
        top: SCR.bar + 16,
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '7px 13px',
        borderRadius: 10,
        background: 'rgba(14,15,17,0.82)',
        border: `1.5px solid ${C.lineHi}`,
        opacity: k,
        fontFamily: F.mono,
        fontSize: 18,
        color: C.ink,
        letterSpacing: '0.06em',
      }}
    >
      <svg width="22" height="16" viewBox="0 0 22 16">
        <path d="M1 1l9 7-9 7zM11 1l9 7-9 7z" fill={C.ink} />
      </svg>
      {rate}×
    </div>
  );

/** The panel screen for one scene: footage cuts, camera, callouts. */
const Screen: React.FC<{scene: Scene; t: number; startFrame: number; vis: number}> = ({scene, t, startFrame, vis}) => {
  const cam = camera(t, scene.moves);
  const active = scene.cuts.find((c) => t >= c.at && t < cutEnd(c));
  const rate = active?.rate ?? 1;
  const ffK = rate > 1 && active ? band(t, active.at, active.at + 0.15) * (1 - band(t, cutEnd(active) - 0.12, cutEnd(active))) : 0;
  const scale = cam.s;
  return (
    <div
      style={{
        position: 'absolute',
        left: SCR.x,
        top: SCR.y - SCR.bar,
        width: SCR.w,
        height: SCR.h + SCR.bar,
        borderRadius: 18,
        overflow: 'hidden',
        background: '#0b0c0e',
        border: `1.5px solid ${C.lineHi}`,
        boxShadow: '0 40px 120px rgba(0,0,0,0.6)',
        opacity: vis,
        transform: `translateY(${(1 - vis) * 40}px) scale(${lerp(0.96, 1, vis)})`,
      }}
    >
      <div style={{height: SCR.bar, display: 'flex', alignItems: 'center', gap: 8, padding: '0 16px', borderBottom: `1px solid ${C.line}`, background: C.panel}}>
        {[0, 1, 2].map((i) => (
          <i key={i} style={{width: 11, height: 11, borderRadius: 6, background: C.faint, display: 'inline-block'}} />
        ))}
        <span style={{marginLeft: 14, fontFamily: F.mono, fontSize: 15, color: C.muted, letterSpacing: '0.04em'}}>localhost:7070 · painel do spillway</span>
        <span style={{flex: 1}} />
        <i style={{width: 8, height: 8, borderRadius: 4, background: C.green, display: 'inline-block', boxShadow: `0 0 0 4px rgba(47,179,107,0.22)`}} />
        <span style={{fontFamily: F.mono, fontSize: 14, color: C.dim, letterSpacing: '0.14em'}}>AO VIVO</span>
      </div>
      <div style={{position: 'relative', width: SCR.w, height: SCR.h, overflow: 'hidden'}}>
        <div
          style={{
            position: 'absolute',
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
            transformOrigin: '0 0',
            transform: `translate(${SCR.w / 2}px, ${SCR.h / 2}px) scale(${K * scale}) translate(${-cam.cx}px, ${-cam.cy}px)`,
          }}
        >
          {scene.cuts.map((c, i) => (
            <Sequence key={i} from={startFrame + Math.round(c.at * DFPS)} durationInFrames={Math.round(((c.to - c.from) / (c.rate ?? 1)) * DFPS)} layout="none">
              <OffthreadVideo
                src={staticFile(`clips/${c.clip}.mp4`)}
                trimBefore={Math.round(c.from * DFPS)}
                playbackRate={c.rate ?? 1}
                muted
                style={{position: 'absolute', left: 0, top: 0, width: 1920, height: 1080}}
              />
            </Sequence>
          ))}
        </div>
        {scene.callouts.map((c, i) => (
          <CalloutView key={i} c={c} t={t} cam={cam} />
        ))}
        <FastForward rate={rate} k={ffK} />
      </div>
    </div>
  );
};

/** Chapter label + headline over the screen (one line, swapped with a mask). */
const Header: React.FC<{scene: Scene; t: number; vis: number}> = ({scene, t, vis}) => (
  <>
    <div style={{position: 'absolute', left: SCR.x, top: 44, opacity: vis, display: 'flex', gap: 14, alignItems: 'baseline'}}>
      <span style={{fontFamily: F.mono, fontSize: 18, color: C.blue, letterSpacing: '0.2em'}}>{scene.n}</span>
      <Label style={{fontSize: 18}}>{scene.label}</Label>
    </div>
    {scene.heads.map((hd, i) => {
      const ks = hd.words.map((_, j) => prog(t, hd.from + 0.15 + j * 0.07, hd.from + 0.15 + j * 0.07 + 0.7, easeOut));
      const out = Math.max(prog(t, hd.to - 0.3, hd.to, easeIn), 1 - vis);
      if (out >= 1 || ks.every((q) => q <= 0)) return null;
      return (
        <div key={i} style={{position: 'absolute', left: SCR.x, top: 78}}>
          <Line words={hd.words} ks={ks} out={out} size={76} />
        </div>
      );
    })}
  </>
);

// ------------------------------------------------------------------ terminal

type TermLine = {at: number; text: string; cmd?: boolean; dim?: boolean; accent?: boolean};

const termLines = (): TermLine[] => {
  const [c1, c2, c3] = TL.term.cmds;
  const short = (p: string) => p.replace(/^([0-9a-f]{8})[0-9a-f-]+/, '$1…');
  const lsDir = short(TERM.ls_path.replace('/videos/', ''));
  const file = short(TERM.inspect_file);
  const insp = JSON.parse(TERM.inspect.slice(0, TERM.inspect.lastIndexOf('}') + 1));
  const tr = insp.tracks[0];
  const took = TERM.inspect.split('\n').pop() ?? '';
  const out: TermLine[] = [];
  out.push({at: c1, text: 'mount | grep spillway', cmd: true});
  TERM.mount.split('\n').forEach((l: string, i: number) => out.push({at: c1 + 0.75 + i * 0.12, text: l.trim()}));
  out.push({at: c2, text: `ls -l /videos/${lsDir}`, cmd: true});
  TERM.ls.split('\n').forEach((l: string, i: number) => {
    const [size, name] = l.split(' ');
    out.push({at: c2 + 0.75 + i * 0.1, text: `${size.padStart(8)}  ${name}`});
  });
  out.push({at: c3, text: `spillway inspect /videos/${file}`, cmd: true});
  out.push({at: c3 + 0.95, text: `duração ${insp.duration_sec.toFixed(1)} s · ${insp.fragmented ? 'fMP4' : 'MP4 progressivo'} · ${insp.major_brand}`});
  out.push({at: c3 + 1.07, text: `vídeo   ${tr.codec} ${tr.width}×${tr.height}`});
  out.push({at: c3 + 1.19, text: took.replace('parsed in', 'analisado em'), accent: true});
  return out;
};

const Terminal: React.FC<{t: number; vis: number}> = ({t, vis}) => {
  const lines = termLines();
  return (
    <div
      style={{
        position: 'absolute',
        left: 300,
        top: 236,
        width: 1320,
        height: 640,
        borderRadius: 18,
        background: '#0b0c0e',
        border: `1.5px solid ${C.lineHi}`,
        boxShadow: '0 40px 120px rgba(0,0,0,0.6)',
        opacity: vis,
        transform: `translateY(${(1 - vis) * 40}px) scale(${lerp(0.96, 1, vis)})`,
        overflow: 'hidden',
      }}
    >
      <div style={{height: 40, display: 'flex', alignItems: 'center', gap: 8, padding: '0 18px', background: C.panel, borderBottom: `1px solid ${C.line}`}}>
        {[0, 1, 2].map((i) => (
          <i key={i} style={{width: 12, height: 12, borderRadius: 6, background: C.faint, display: 'inline-block'}} />
        ))}
        <span style={{marginLeft: 14, fontFamily: F.mono, fontSize: 16, color: C.muted}}>root@lab · bash</span>
      </div>
      <div style={{padding: '22px 30px', fontFamily: F.mono, fontSize: 25, lineHeight: 1.62}}>
        {lines.map((l, i) => {
          if (t < l.at) return null;
          if (l.cmd) {
            const n = Math.floor((t - l.at) * 46);
            const typed = l.text.slice(0, n);
            const caret = n < l.text.length || (lines[i + 1] && t < lines[i + 1].at && Math.floor(t * 2.2) % 2 === 0);
            return (
              <div key={i} style={{color: C.ink, whiteSpace: 'pre'}}>
                <span style={{color: C.blue}}>$ </span>
                {typed}
                {caret && <span style={{background: C.ink, color: C.bg}}> </span>}
              </div>
            );
          }
          return (
            <div key={i} style={{color: l.accent ? C.blueHi : C.dim, whiteSpace: 'pre', opacity: band(t, l.at, l.at + 0.12)}}>
              {l.text}
            </div>
          );
        })}
      </div>
    </div>
  );
};

// ------------------------------------------------------------------ the film

const sceneVis = (t: number, at: number, end: number) => prog(t, at, at + 0.5, easeOut) * (1 - prog(t, end - 0.35, end, easeIn));

export const DemoVideo: React.FC = () => {
  useFonts();
  const frame = useCurrentFrame();
  const t = frame / DFPS;
  const I = TL.intro;
  const T = TL.term;
  const O = TL.outro;

  // intro
  const introOut = prog(t, I.end - 0.45, I.end, easeIn);
  const logoK = sp(frame, 0.15, POP);

  // terminal
  const termVis = sceneVis(t, T.at, T.end);
  const tt = t - T.at;

  // outro
  const ot = t - O.at;
  const outroK = prog(t, O.at, O.at + 0.6, easeOut);
  const heroK = sp(frame, O.at + O.logo, POP);
  const fade = prog(t, TL.duration - 0.6, TL.duration, easeIn);

  // the logo docks top-right through the demo
  const dockK = prog(t, I.end - 0.5, I.end + 0.2, easeInOut) * (1 - prog(t, O.at - 0.2, O.at + 0.5, easeInOut));
  const glow = 0.25 + 0.2 * band(t, O.at, O.at + 1);

  return (
    <AbsoluteFill style={{background: C.bg, overflow: 'hidden'}}>
      <Background t={t} glow={glow} glowY={45} />

      {/* ---------------- intro ---------------- */}
      {t < I.end + 0.2 && (
        <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center', opacity: 1 - introOut}}>
          <div style={{transform: `scale(${clamp01(logoK)})`, marginBottom: 46}}>
            <Logo size={120} k={prog(t, 0.25, 0.9, easeOut)} glow={0.5} />
          </div>
          <Label style={{opacity: prog(t, I.label, I.label + 0.5, easeOut), marginBottom: 26}}>demonstração · lab real, sem montagem</Label>
          <Line words={W('Chega de slide.')} ks={[0, 1, 2].map((j) => prog(t, I.l1 + j * 0.09, I.l1 + j * 0.09 + 0.8, easeOut))} out={introOut} size={110} />
          <div style={{height: 18}} />
          <Line words={W('O spillway *rodando de verdade.*')} ks={[0, 1, 2, 3, 4].map((j) => prog(t, I.l2 + j * 0.09, I.l2 + j * 0.09 + 0.8, easeOut))} out={introOut} size={110} />
        </AbsoluteFill>
      )}

      {/* ---------------- terminal ---------------- */}
      {termVis > 0.01 && (
        <>
          <div style={{position: 'absolute', left: 300, top: 60, opacity: termVis, display: 'flex', gap: 14, alignItems: 'baseline'}}>
            <span style={{fontFamily: F.mono, fontSize: 18, color: C.blue, letterSpacing: '0.2em'}}>01</span>
            <Label style={{fontSize: 18}}>O mount</Label>
          </div>
          <div style={{position: 'absolute', left: 300, top: 94}}>
            <Line words={W('Um mount comum. *RAM por baixo.*')} ks={[0, 1, 2, 3, 4, 5].map((j) => prog(tt, 0.15 + j * 0.07, 0.85 + j * 0.07, easeOut))} out={1 - termVis} size={76} />
          </div>
          <Terminal t={tt} vis={termVis} />
        </>
      )}

      {/* ---------------- panel scenes ---------------- */}
      {SCENES.map((s) => {
        const tl = TL[s.key] as {at: number; end: number};
        if (t < tl.at - 0.1 || t > tl.end + 0.1) return null;
        const vis = sceneVis(t, tl.at, tl.end);
        const lt = t - tl.at;
        return (
          <React.Fragment key={s.key}>
            <Screen scene={s} t={lt} startFrame={Math.round(tl.at * DFPS)} vis={vis} />
            <Header scene={s} t={lt} vis={vis} />
            {s.chip?.(lt)}
          </React.Fragment>
        );
      })}

      {/* docked mark */}
      {dockK > 0.01 && (
        <div style={{position: 'absolute', right: 40, bottom: 34, display: 'flex', alignItems: 'center', gap: 12, opacity: dockK * 0.9}}>
          <Logo size={34} />
          <span style={{fontFamily: F.serif, fontSize: 34, color: C.ink, letterSpacing: '-0.01em'}}>spillway</span>
        </div>
      )}

      {/* ---------------- outro ---------------- */}
      {t > O.at - 0.1 && (
        <AbsoluteFill style={{opacity: outroK}}>
          <Sequence from={Math.round(O.at * DFPS)} layout="none">
            <AbsoluteFill style={{filter: 'blur(14px) saturate(0.6)', opacity: 0.22, transform: `scale(${1.08 + ot * 0.01})`}}>
              <OffthreadVideo src={staticFile('clips/hero.mp4')} muted style={{width: '100%', height: '100%'}} />
            </AbsoluteFill>
          </Sequence>
          <AbsoluteFill style={{background: 'radial-gradient(ellipse 70% 60% at 50% 45%, rgba(14,15,17,0.55), rgba(14,15,17,0.95))'}} />
          <AbsoluteFill style={{alignItems: 'center', paddingTop: 170}}>
            <div style={{transform: `scale(${clamp01(heroK)})`}}>
              <Logo size={128} k={prog(ot, O.logo, O.logo + 0.7, easeOut)} glow={0.7} />
            </div>
            <div style={{marginTop: 26, fontFamily: F.serif, fontSize: 150, lineHeight: 1, color: C.ink, letterSpacing: '-0.02em'}}>
              <Mask k={prog(ot, O.word, O.word + 0.8, easeOut)}>spillway</Mask>
            </div>
            <div style={{marginTop: 22}}>
              <Line words={W('RAM primeiro. Disco certo. *Nada se perde.*')} ks={[0, 1, 2, 3, 4, 5, 6].map((j) => prog(ot, O.tag + j * 0.08, O.tag + j * 0.08 + 0.8, easeOut))} out={0} size={60} />
            </div>
            <div style={{display: 'flex', gap: 90, marginTop: 70}}>
              {[
                {v: '20/20', l: 'cenários de falha testados'},
                {v: `${perMin.toLocaleString('pt-BR')} mil`, l: 'MP4 por minuto na rajada'},
                {v: '1', l: 'binário Rust, painel embutido', accent: true},
              ].map((s, i) => {
                const q = clamp01(sp(frame, O.at + O.stats[i], POP));
                return (
                  <div key={i} style={{textAlign: 'center', opacity: q, transform: `translateY(${(1 - q) * 24}px)`}}>
                    <div style={{fontFamily: F.serif, fontSize: 84, lineHeight: 1, color: s.accent ? C.blue : C.ink, fontStyle: s.accent ? 'italic' : 'normal'}}>{s.v}</div>
                    <Label style={{marginTop: 12, fontSize: 16}}>{s.l}</Label>
                  </div>
                );
              })}
            </div>
            <Label style={{marginTop: 64, opacity: prog(ot, O.footer, O.footer + 0.6, easeOut)}}>Rust · FUSE · master / nodes · hooks</Label>
          </AbsoluteFill>
        </AbsoluteFill>
      )}

      <Grain frame={frame} opacity={0.06} />
      {fade > 0 && <AbsoluteFill style={{background: `rgba(0,0,0,${fade})`}} />}
    </AbsoluteFill>
  );
};
