// 示例项目（界面原型数据）。Tauri 接入后由后端读取 .maskproj 替换。
import type { Project, Quad, Segment, Target } from './types';

const W = 1920;
const H = 1080;

/** 把百分比矩形（相对画面）转为像素四边形。 */
function rect(x: number, y: number, w: number, h: number): Quad {
  const x0 = (x / 100) * W;
  const y0 = (y / 100) * H;
  const x1 = ((x + w) / 100) * W;
  const y1 = ((y + h) / 100) * H;
  return [
    [x0, y0],
    [x1, y0],
    [x1, y1],
    [x0, y1],
  ];
}

const targets: Target[] = [
  {
    id: 't1',
    name: '机密',
    kind: 'text',
    color: '#d1495b',
    engine: 'ocr',
    mode: 'standard',
    threshold: 0.6,
    style: { kind: 'mosaic', strength: 0.7, padding: 8, feather: true, motion_pad: true },
    enabled: true,
  },
  {
    id: 't2',
    name: 'logo',
    kind: 'image',
    color: '#2a9d8f',
    engine: 'sift_ransac',
    mode: 'standard',
    threshold: 0.6,
    style: { kind: 'blur', strength: 0.5, padding: 6, feather: true, motion_pad: false },
    enabled: true,
  },
  {
    id: 't3',
    name: '张三',
    kind: 'text',
    color: '#8e6bbf',
    engine: 'ocr',
    mode: 'precise',
    threshold: 0.6,
    style: { kind: 'solid', strength: 1, padding: 4, feather: false, motion_pad: false },
    enabled: false,
  },
];

function seg(
  id: string,
  target: string,
  t0: number,
  t1: number,
  status: Segment['status'],
  conf: number,
  box: [number, number, number, number],
  engine: Segment['engine'] = 'ocr',
): Segment {
  const quad = rect(...box);
  return {
    id,
    target,
    t0,
    t1,
    status,
    source: 'auto',
    engine,
    conf,
    keyframes: [{ t: t0, quad, conf }],
    negatives: [],
  };
}

const segments: Segment[] = [
  seg('s1', 't1', 12.3, 18.7, 'confirmed', 0.91, [9, 14, 13, 6]),
  seg('s2', 't1', 40.0, 44.5, 'pending', 0.78, [52, 70, 14, 6]),
  seg('s3', 't1', 130.2, 133.8, 'low', 0.52, [30, 40, 14, 6]),
  seg('s4', 't1', 190.0, 196.0, 'pending', 0.83, [9, 14, 13, 6]),
  seg('s5', 't2', 0, 120, 'confirmed', 0.95, [80, 5, 12, 12], 'sift_ransac'),
  seg('s6', 't2', 150, 210, 'confirmed', 0.93, [80, 5, 12, 12], 'sift_ransac'),
];

export const sampleProject: Project = {
  format_version: 1,
  video: {
    path: 'sample.mp4',
    content_hash: 'blake3-sampled:demo',
    duration: 225,
    fps: 30,
    width: W,
    height: H,
  },
  targets,
  segments,
};

export const SAMPLE_VIDEO_W = W;
export const SAMPLE_VIDEO_H = H;
