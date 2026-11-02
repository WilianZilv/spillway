import React from 'react';
import {Composition} from 'remotion';
import {SpillwayVideo} from './SpillwayVideo';
import {DFPS, DH, DW, DemoVideo} from './demo/DemoVideo';
import DTL from '../demo-timeline.json';
import {FPS, H, TL, W} from './lib';

export const Root: React.FC = () => (
  <>
    <Composition id="SpillwayVideo" component={SpillwayVideo} durationInFrames={Math.round(TL.duration * FPS)} fps={FPS} width={W} height={H} />
    <Composition id="DemoVideo" component={DemoVideo} durationInFrames={Math.round(DTL.duration * DFPS)} fps={DFPS} width={DW} height={DH} />
  </>
);
