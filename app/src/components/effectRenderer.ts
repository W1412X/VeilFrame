// 预览的实时打码绘制。每一帧都从真实画面像素计算效果，与导出的 vf-render 使用相同的参数：
// - 马赛克：整帧先按块大小缩小（取块平均），再最近邻放大，只保留打码区域；
// - 模糊：对区域及周围一圈画面做高斯模糊；
// - 色块：区域填充黑色；
// - 羽化：掩膜做高斯模糊后作为 alpha 合成。
import type { Quad, Style } from '../domain/types';
import { blurSigma, expandQuad, motionPadding, mosaicBlock } from '../domain/render';

export interface RenderItem {
  style: Style;
  /** 当前时刻的四边形（视频像素坐标）。 */
  quad: Quad;
  /** 运动速度（像素/帧）。 */
  speed: number;
}

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 羽化与外扩留出的边距（像素），避免模糊在框边被截断。 */
const EDGE_MARGIN = 4;

function makeCanvas(w: number, h: number): HTMLCanvasElement {
  const c = document.createElement('canvas');
  c.width = Math.max(1, Math.round(w));
  c.height = Math.max(1, Math.round(h));
  return c;
}

function ctxOf(c: HTMLCanvasElement): CanvasRenderingContext2D {
  const ctx = c.getContext('2d');
  if (!ctx) throw new Error('无法创建 2D 绘图上下文');
  return ctx;
}

/** 旧版 WebKit 不支持 `ctx.filter`，此时模糊与羽化退化为缩放近似。 */
function supportsFilter(ctx: CanvasRenderingContext2D): boolean {
  return typeof (ctx as { filter?: unknown }).filter === 'string';
}

/** 四边形在画面内的整数外接框（已加边距），完全在画面外时返回 null。 */
function boundsOf(q: Quad, width: number, height: number): Box | null {
  const xs = q.map((p) => p[0]);
  const ys = q.map((p) => p[1]);
  const x0 = Math.max(0, Math.floor(Math.min(...xs)) - EDGE_MARGIN);
  const y0 = Math.max(0, Math.floor(Math.min(...ys)) - EDGE_MARGIN);
  const x1 = Math.min(width, Math.ceil(Math.max(...xs)) + EDGE_MARGIN);
  const y1 = Math.min(height, Math.ceil(Math.max(...ys)) + EDGE_MARGIN);
  if (x1 <= x0 || y1 <= y0) return null;
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

export class EffectRenderer {
  private readonly width: number;
  private readonly height: number;
  /** 当前帧的原始画面（视频像素分辨率）。 */
  private readonly frame: HTMLCanvasElement;
  /** 可复用的离屏画布，按名字存放。 */
  private readonly pool = new Map<string, HTMLCanvasElement>();

  constructor(width: number, height: number) {
    this.width = width;
    this.height = height;
    this.frame = makeCanvas(width, height);
  }

  /** 绘制一帧：先画原始画面，再把每个打码项的效果覆盖到输出上。 */
  render(out: CanvasRenderingContext2D, source: CanvasImageSource, items: RenderItem[]): void {
    const frameCtx = ctxOf(this.frame);
    frameCtx.clearRect(0, 0, this.width, this.height);
    frameCtx.drawImage(source, 0, 0, this.width, this.height);
    out.drawImage(this.frame, 0, 0);

    // 同一帧内，相同块大小的马赛克共用一张缩小图
    const smallByBlock = new Map<number, HTMLCanvasElement>();
    items.forEach((item, index) => this.applyItem(out, item, index, smallByBlock));
  }

  private applyItem(
    out: CanvasRenderingContext2D,
    item: RenderItem,
    index: number,
    smallByBlock: Map<number, HTMLCanvasElement>,
  ): void {
    const { style } = item;
    if (style.kind === 'image') return; // 与后端一致：图像遮罩尚未实现

    const pad = style.motion_pad ? motionPadding(style.padding, item.speed) : style.padding;
    const area = expandQuad(item.quad, pad);
    const box = boundsOf(area, this.width, this.height);
    if (!box) return;

    const fx = this.canvas(`fx${index}`, box.w, box.h);
    const fctx = ctxOf(fx);
    fctx.clearRect(0, 0, box.w, box.h);
    this.paintEffect(fctx, style, box, index, smallByBlock);

    // 掩膜：多边形在框内的位置。羽化时先对掩膜做模糊，再以其 alpha 合成。
    const mk = this.canvas(`mk${index}`, box.w, box.h);
    const mctx = ctxOf(mk);
    mctx.clearRect(0, 0, box.w, box.h);
    const feather = style.feather && supportsFilter(mctx);
    if (feather) mctx.filter = 'blur(3px)';
    mctx.fillStyle = '#fff';
    mctx.beginPath();
    area.forEach(([x, y], i) => {
      if (i === 0) mctx.moveTo(x - box.x, y - box.y);
      else mctx.lineTo(x - box.x, y - box.y);
    });
    mctx.closePath();
    mctx.fill();
    if (feather) mctx.filter = 'none';

    fctx.globalCompositeOperation = 'destination-in';
    fctx.drawImage(mk, 0, 0);
    fctx.globalCompositeOperation = 'source-over';

    out.drawImage(fx, box.x, box.y);
  }

  private paintEffect(
    fctx: CanvasRenderingContext2D,
    style: Style,
    box: Box,
    index: number,
    smallByBlock: Map<number, HTMLCanvasElement>,
  ): void {
    switch (style.kind) {
      case 'mosaic': {
        const block = mosaicBlock(style.strength);
        let small = smallByBlock.get(block);
        if (!small) {
          const sw = Math.max(1, Math.floor(this.width / block));
          const sh = Math.max(1, Math.floor(this.height / block));
          small = this.canvas(`small${block}`, sw, sh);
          const sctx = ctxOf(small);
          sctx.clearRect(0, 0, sw, sh);
          sctx.imageSmoothingEnabled = true;
          sctx.imageSmoothingQuality = 'high';
          sctx.drawImage(this.frame, 0, 0, sw, sh);
          smallByBlock.set(block, small);
        }
        // 把框映射到缩小图上，再最近邻放大，得到块状像素
        const sx = (box.x * small.width) / this.width;
        const sy = (box.y * small.height) / this.height;
        const sw = Math.max(1, (box.w * small.width) / this.width);
        const sh = Math.max(1, (box.h * small.height) / this.height);
        fctx.imageSmoothingEnabled = false;
        fctx.drawImage(small, sx, sy, sw, sh, 0, 0, box.w, box.h);
        fctx.imageSmoothingEnabled = true;
        return;
      }
      case 'blur': {
        const sigma = blurSigma(style.strength);
        // 卷积核半径约 3 个 sigma：在框外多取这么宽的一圈，框内结果与整帧模糊一致
        const m = Math.ceil(sigma * 3) + 1;
        const sx0 = Math.max(0, box.x - m);
        const sy0 = Math.max(0, box.y - m);
        const sx1 = Math.min(this.width, box.x + box.w + m);
        const sy1 = Math.min(this.height, box.y + box.h + m);
        const src = this.blurredSource(sx0, sy0, sx1 - sx0, sy1 - sy0, sigma, index);
        fctx.drawImage(src, box.x - sx0, box.y - sy0, box.w, box.h, 0, 0, box.w, box.h);
        return;
      }
      case 'solid':
        fctx.fillStyle = '#000';
        fctx.fillRect(0, 0, box.w, box.h);
        return;
      default:
        return;
    }
  }

  /** 取原始画面的一个子矩形并做高斯模糊。 */
  private blurredSource(x: number, y: number, w: number, h: number, sigma: number, index: number): HTMLCanvasElement {
    const out = this.canvas(`blur${index}`, w, h);
    const bctx = ctxOf(out);
    bctx.clearRect(0, 0, w, h);
    if (supportsFilter(bctx)) {
      bctx.filter = `blur(${sigma}px)`;
      bctx.drawImage(this.frame, x, y, w, h, 0, 0, w, h);
      bctx.filter = 'none';
    } else {
      // 旧内核：先缩小再放大，得到近似的模糊
      const k = Math.max(2, Math.round(sigma));
      const tiny = this.canvas(`tiny${index}`, Math.max(1, Math.round(w / k)), Math.max(1, Math.round(h / k)));
      const tctx = ctxOf(tiny);
      tctx.imageSmoothingQuality = 'high';
      tctx.drawImage(this.frame, x, y, w, h, 0, 0, tiny.width, tiny.height);
      bctx.imageSmoothingQuality = 'high';
      bctx.drawImage(tiny, 0, 0, tiny.width, tiny.height, 0, 0, w, h);
    }
    return out;
  }

  /** 按名字取一张可复用画布，尺寸变化时调整。 */
  private canvas(name: string, w: number, h: number): HTMLCanvasElement {
    const tw = Math.max(1, Math.round(w));
    const th = Math.max(1, Math.round(h));
    let c = this.pool.get(name);
    if (!c) {
      c = makeCanvas(tw, th);
      this.pool.set(name, c);
    } else if (c.width !== tw || c.height !== th) {
      c.width = tw;
      c.height = th;
    }
    return c;
  }
}
