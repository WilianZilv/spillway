import React, {useEffect, useState} from 'react';
import {AbsoluteFill, continueRender, delayRender, useVideoConfig} from 'remotion';
import {C, F, clamp01} from '../lib';

/** Word reveal: k 0→1 slides the word up out of a clip; out 0→1 sends it on up and away. */
export const Mask: React.FC<{k: number; out?: number; children: React.ReactNode; style?: React.CSSProperties}> = ({
  k,
  out = 0,
  children,
  style,
}) => (
  <span style={{display: 'inline-block', overflow: 'hidden', verticalAlign: 'bottom', padding: '0.05em 0.08em 0.18em', margin: '-0.05em -0.08em -0.18em'}}>
    <span
      style={{
        display: 'inline-block',
        transform: `translateY(${(1 - k - out) * 115}%) rotate(${(1 - k) * 5}deg)`,
        transformOrigin: '0% 100%',
        ...style,
      }}
    >
      {children}
    </span>
  </span>
);

/** A display line made of words, each with its own reveal value; accent words are italic blue. */
export const Line: React.FC<{
  words: {w: string; accent?: boolean}[];
  ks: number[];
  out?: number;
  size?: number;
  color?: string;
}> = ({words, ks, out = 0, size = 112, color = C.ink}) => (
  <div style={{fontFamily: F.serif, fontSize: size, lineHeight: 1, letterSpacing: '-0.02em', color, whiteSpace: 'nowrap'}}>
    {words.map((w, i) => (
      <React.Fragment key={i}>
        <Mask k={ks[i] ?? 1} out={out} style={w.accent ? {fontStyle: 'italic', color: C.blue} : undefined}>
          {w.w}
        </Mask>
        {i < words.length - 1 ? ' ' : null}
      </React.Fragment>
    ))}
  </div>
);

/** Mono uppercase label. */
export const Label: React.FC<{children: React.ReactNode; style?: React.CSSProperties}> = ({children, style}) => (
  <div style={{fontFamily: F.mono, fontSize: 22, letterSpacing: '0.2em', textTransform: 'uppercase', color: C.muted, ...style}}>{children}</div>
);

/** Film grain overlay, reseeded on twos. */
export const Grain: React.FC<{frame: number; opacity?: number}> = ({frame, opacity = 0.08}) => {
  const {width, height} = useVideoConfig();
  return (
    <AbsoluteFill style={{pointerEvents: 'none', mixBlendMode: 'overlay', opacity}}>
      <svg width={width} height={height}>
        <filter id="grain" x="0" y="0" width="100%" height="100%">
          <feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves={2} seed={Math.floor(frame / 2) % 97} stitchTiles="stitch" />
          <feColorMatrix type="saturate" values="0" />
        </filter>
        <rect width={width} height={height} filter="url(#grain)" />
      </svg>
    </AbsoluteFill>
  );
};

/** Dark field: drifting dot grid, a glow that follows the story, vignette. */
export const Background: React.FC<{t: number; glow: number; glowX?: number; glowY?: number; hue?: string}> = ({
  t,
  glow,
  glowX = 50,
  glowY = 55,
  hue = '61,139,242',
}) => {
  const {width, height} = useVideoConfig();
  return (
    <AbsoluteFill style={{background: C.bg}}>
      <AbsoluteFill
        style={{
          background: `radial-gradient(ellipse 65% 50% at ${glowX}% ${glowY}%, rgba(${hue},${0.03 + 0.17 * glow}) 0%, rgba(${hue},0) 70%)`,
        }}
      />
      <svg width={width} height={height} style={{position: 'absolute', inset: 0}}>
        <defs>
          <pattern id="dots" width="40" height="40" patternUnits="userSpaceOnUse" patternTransform={`translate(${-t * 5} ${-t * 9})`}>
            <circle cx="20" cy="20" r="1.2" fill="rgba(238,241,245,0.12)" />
          </pattern>
          <radialGradient id="fade" cx="50%" cy="50%" r="62%">
            <stop offset="0%" stopColor="#fff" stopOpacity="1" />
            <stop offset="78%" stopColor="#fff" stopOpacity="0.2" />
            <stop offset="100%" stopColor="#fff" stopOpacity="0" />
          </radialGradient>
          <mask id="dotMask">
            <rect width={width} height={height} fill="url(#fade)" />
          </mask>
        </defs>
        <rect width={width} height={height} fill="url(#dots)" mask="url(#dotMask)" />
      </svg>
      <AbsoluteFill style={{background: 'radial-gradient(ellipse 90% 80% at 50% 50%, rgba(0,0,0,0) 55%, rgba(0,0,0,0.6) 100%)'}} />
    </AbsoluteFill>
  );
};

/** The spillway mark: a rounded tile with three bars spilling to the left. `k` draws the bars on. */
export const Logo: React.FC<{size: number; k?: number; glow?: number}> = ({size, k = 1, glow = 0}) => {
  const bars = [
    {y: 11, x2: 25},
    {y: 16, x2: 19},
    {y: 21, x2: 14},
  ];
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" style={{display: 'block', overflow: 'visible', filter: glow ? `drop-shadow(0 0 ${glow * 40}px rgba(61,139,242,${0.6 * glow}))` : undefined}}>
      <rect width="32" height="32" rx="8" fill={C.blue} />
      {bars.map((b, i) => {
        const p = clamp01(k * 1.6 - i * 0.22);
        return <line key={i} x1="7" y1={b.y} x2={7 + (b.x2 - 7) * p} y2={b.y} stroke="#fff" strokeWidth="3" strokeLinecap="round" opacity={p > 0.01 ? 1 : 0} />;
      })}
    </svg>
  );
};

/** A recording segment: a small file chip with an mp4 tag. */
export const Chip: React.FC<{x: number; y: number; s?: number; o?: number; tone?: 'blue' | 'plain' | 'red'; rot?: number}> = ({
  x,
  y,
  s = 1,
  o = 1,
  tone = 'plain',
  rot = 0,
}) => {
  const fill = tone === 'blue' ? C.blue : tone === 'red' ? C.red : C.panelHi;
  const text = tone === 'plain' ? C.dim : '#fff';
  return (
    <div
      style={{
        position: 'absolute',
        left: x - 30 * s,
        top: y - 17 * s,
        width: 60 * s,
        height: 34 * s,
        borderRadius: 8 * s,
        background: fill,
        border: `${1.5 * s}px solid ${tone === 'plain' ? C.lineHi : 'rgba(255,255,255,0.25)'}`,
        opacity: o,
        transform: `rotate(${rot}deg)`,
        display: 'grid',
        placeItems: 'center',
        fontFamily: F.mono,
        fontSize: 13 * s,
        fontWeight: 600,
        letterSpacing: '0.06em',
        color: text,
      }}
    >
      MP4
    </div>
  );
};

export const useFonts = () => {
  const [handle] = useState(() => delayRender('fonts'));
  useEffect(() => {
    Promise.all([
      document.fonts.load('400 100px "Instrument Serif"'),
      document.fonts.load('italic 400 100px "Instrument Serif"'),
      document.fonts.load('400 20px "JetBrains Mono"'),
      document.fonts.load('600 20px "JetBrains Mono"'),
      document.fonts.load('400 20px "Inter Variable"'),
      document.fonts.load('600 20px "Inter Variable"'),
    ]).then(() => continueRender(handle));
  }, [handle]);
};
