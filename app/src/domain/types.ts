// 与 Rust 端 vf-project 的 JSON 结构保持一致（字段名一致，便于 Tauri 直接传递）。

export type TargetKind = 'text' | 'image' | 'region';
export type Mode = 'fast' | 'standard' | 'precise';
export type Engine =
  | 'auto'
  | 'ncc'
  | 'sift_ransac'
  | 'akaze'
  | 'super_point_light_glue'
  | 'shape_edge'
  | 'ocr';
export type MaskKind = 'mosaic' | 'blur' | 'solid' | 'image';
export type SegmentStatus = 'confirmed' | 'pending' | 'low' | 'rejected';
export type Source = 'auto' | 'manual';

/** 四边形：左上、右上、右下、左下，视频像素坐标。 */
export type Quad = [[number, number], [number, number], [number, number], [number, number]];

export interface Style {
  kind: MaskKind;
  /** 0.0 ~ 1.0 */
  strength: number;
  /** 外扩像素 */
  padding: number;
  feather: boolean;
  motion_pad: boolean;
}

export interface Target {
  id: string;
  name: string;
  kind: TargetKind;
  color: string;
  engine: Engine;
  mode: Mode;
  /** 0.0 ~ 1.0 */
  threshold: number;
  style: Style;
  enabled: boolean;
}

export interface Keyframe {
  t: number;
  quad: Quad;
  conf: number;
}

export interface Segment {
  id: string;
  target: string;
  t0: number;
  t1: number;
  status: SegmentStatus;
  source: Source;
  engine: Engine;
  conf: number;
  keyframes: Keyframe[];
  negatives: Keyframe[];
}

export interface Video {
  path: string;
  content_hash: string;
  duration: number;
  fps: number;
  width: number;
  height: number;
}

export interface Project {
  format_version: number;
  video: Video | null;
  targets: Target[];
  segments: Segment[];
}
